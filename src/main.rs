use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use hippo::backends::{build_embedder, vector_source};
use hippo::client::{STARTUP_WAIT, ensure_daemon, request, request_until_reply, run_hook};
use hippo::commands::{
    self, Confirm, ImportTally, SCOPE_ALL, SCOPE_PROJECT, absolute, confirm_delete, hook_settings,
    launcher, project_scope, render_delete_report, scope_arg, session_line,
};
use hippo::config::{Config, Paths};
use hippo::daemon;
use hippo::i18n::Lang;
use hippo::import::render_preview;
use hippo::install::Scope;
use hippo::protocol::{Request, Response, embed_progress};
use hippo::registry::is_project_id;
use hippo::search::{SearchKind, escape_control};
use hippo::setup::launchd_plist;
use hippo::store::ForgetTarget;
use hippo::tr;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// `import --wait`이 상태를 묻는 간격. (IF-CLI-9)
const WAIT_POLL: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(
    name = "hippo",
    about = "Hippocampal Index: long-term memory for Claude Code sessions"
)]
struct Cli {
    /// Data root (default: ~/.hippo)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Claude Code hook entry point. Reads the hook input from stdin.
    Hook { event: String },
    /// Search memory
    Search {
        query: String,
        /// project (the current directory's project), all, or a path
        #[arg(long, default_value = SCOPE_PROJECT)]
        scope: String,
        #[arg(long, value_enum, default_value = "iteration")]
        kind: KindArg,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Show the iterations of one turn in order
    Show {
        prompt_id: String,
        #[arg(long)]
        with_results: bool,
        /// project or a path. Without it, the current project is searched first, then the others
        #[arg(long)]
        scope: Option<String>,
    },
    /// Run the daemon in the foreground
    Daemon,
    /// Daemon status
    Status,
    /// Rebuild vector indexes with the configured embedder and vector store
    Reindex {
        /// all, project, or a path
        #[arg(long, default_value = SCOPE_ALL)]
        scope: String,
    },
    /// Project commands
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Import past session records into a project. Omitted arguments default to the current directory
    Import {
        /// Path the records came from (an old path is fine) or a project id
        #[arg(long)]
        from: Option<String>,
        /// Path of the project that receives the records
        #[arg(long)]
        to: Option<String>,
        /// Wait until the imported records are embedded, showing progress
        #[arg(long)]
        wait: bool,
        /// Only list the candidate sessions and what would happen; change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Delete one session or one turn (all iterations with the prompt id) from memory
    #[command(group(clap::ArgGroup::new("target").required(true).args(["session", "prompt"])))]
    Forget {
        /// Session id to delete
        #[arg(long)]
        session: Option<String>,
        /// Prompt id of the turn to delete
        #[arg(long)]
        prompt: Option<String>,
        /// project or a path. Without it, the current project is searched first, then the others
        #[arg(long)]
        scope: Option<String>,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        /// Only show what would be deleted
        #[arg(long)]
        dry_run: bool,
    },
    /// Open the GUI window (settings, status, import, search, Claude setup)
    Gui,
    /// Print the hook settings for settings.json
    HooksConfig,
    /// Print a launchd plist
    LaunchdPlist,
    /// Add hippo hooks and the memory guide to Claude Code
    Install(InstallArgs),
    /// Remove hippo hooks and the memory guide from Claude Code
    Uninstall(InstallArgs),
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// List projects
    List,
    /// Delete a project and all its records
    Remove {
        /// Project id or path
        project: String,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        /// Only show what would be deleted
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(clap::Args)]
struct InstallArgs {
    #[arg(long, value_enum)]
    scope: ScopeArg,
    /// Project path (default: current directory)
    #[arg(long)]
    project: Option<PathBuf>,
    /// Print the changes without writing
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ScopeArg {
    User,
    Project,
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Iteration,
    Result,
    Both,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = match cli.root.map_or_else(Paths::default_root, Ok) {
        Ok(root) => root,
        Err(e) => return fail(&e.to_string()),
    };
    let paths = Paths::new(root);
    match cli.command {
        Command::Hook { event } => {
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            let config = Config::load(&paths.config()).unwrap_or_default();
            let budget = Duration::from_millis(config.inject.budget_ms);
            let cold_budget = Duration::from_millis(config.inject.cold_budget_ms);
            if let Some(out) = run_hook(
                &paths,
                budget,
                cold_budget,
                &event,
                &input,
                launcher(&config),
            ) {
                println!("{out}");
            }
            ExitCode::SUCCESS
        }
        Command::Search {
            query,
            scope,
            kind,
            limit,
        } => {
            let cwd = match current_dir() {
                Ok(cwd) => cwd,
                Err(e) => return fail(&e),
            };
            let scope = scope_arg(&cwd, &scope);
            let kind = match kind {
                KindArg::Iteration => SearchKind::Iteration,
                KindArg::Result => SearchKind::Result,
                KindArg::Both => SearchKind::Both,
            };
            query_daemon(
                &paths,
                &Request::Search {
                    query,
                    kind,
                    limit,
                    scope,
                    cwd,
                    structured: false,
                },
            )
        }
        Command::Show {
            prompt_id,
            with_results,
            scope,
        } => match current_dir() {
            Ok(cwd) => query_daemon(
                &paths,
                &Request::Show {
                    prompt_id,
                    with_results,
                    scope: scope.map(|s| scope_arg(&cwd, &s)),
                    cwd,
                },
            ),
            Err(e) => fail(&e),
        },
        Command::Status => query_daemon(&paths, &Request::Status),
        Command::Reindex { scope } => match current_dir() {
            Ok(cwd) => query_daemon(
                &paths,
                &Request::Reindex {
                    scope: scope_arg(&cwd, &scope),
                    cwd,
                },
            ),
            Err(e) => fail(&e),
        },
        Command::Project(ProjectCommand::List) => query_daemon(&paths, &Request::ProjectList),
        Command::Project(ProjectCommand::Remove {
            project,
            yes,
            dry_run,
        }) => match current_dir() {
            Ok(cwd) => {
                let project = if is_project_id(&project) {
                    project
                } else {
                    absolute(&cwd, &project)
                };
                run_delete(
                    &paths,
                    |dry_run| Request::ProjectRemove {
                        project: project.clone(),
                        cwd: cwd.clone(),
                        dry_run,
                    },
                    yes,
                    dry_run,
                )
            }
            Err(e) => fail(&e),
        },
        Command::Forget {
            session,
            prompt,
            scope,
            yes,
            dry_run,
        } => match current_dir() {
            Ok(cwd) => {
                let target = match (session, prompt) {
                    (Some(id), _) => ForgetTarget::Session(id),
                    (None, Some(id)) => ForgetTarget::Prompt(id),
                    (None, None) => return fail("--session or --prompt is required"),
                };
                let scope = scope.map(|s| scope_arg(&cwd, &s));
                run_delete(
                    &paths,
                    |dry_run| Request::Forget {
                        target: target.clone(),
                        scope: scope.clone(),
                        cwd: cwd.clone(),
                        dry_run,
                    },
                    yes,
                    dry_run,
                )
            }
            Err(e) => fail(&e),
        },
        Command::Import {
            from,
            to,
            wait,
            dry_run,
        } => match current_dir() {
            Ok(cwd) => run_import(&paths, &cwd, from, to, wait, dry_run),
            Err(e) => fail(&e),
        },
        Command::Daemon => run_daemon(paths),
        Command::Gui => match current_dir().and_then(|cwd| hippo::gui::run(paths, cwd)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        },
        Command::HooksConfig => match std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| hook_settings(&paths, &exe))
        {
            Ok(hooks) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&hooks).unwrap_or_default()
                );
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
        Command::Install(args) => run_install(&paths, &args, true, ui_lang(&paths)),
        Command::Uninstall(args) => run_install(&paths, &args, false, ui_lang(&paths)),
        Command::LaunchdPlist => match std::env::current_exe() {
            Ok(exe) => {
                print!("{}", launchd_plist(&exe, &paths));
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e.to_string()),
        },
    }
}

fn current_dir() -> Result<String, String> {
    std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

/// hook 설정과 메모리 파일의 안내 블록을 함께 바꾼다. (IF-CLI-6, FR-INS-7)
fn run_install(paths: &Paths, args: &InstallArgs, install: bool, lang: Lang) -> ExitCode {
    let scope = match args.scope {
        ScopeArg::User => Scope::User,
        ScopeArg::Project => {
            let cwd = match std::env::current_dir() {
                Ok(dir) => dir,
                Err(e) => return fail(&e.to_string()),
            };
            match project_scope(args.project.clone(), &cwd) {
                Ok(scope) => scope,
                Err(e) => return fail(&e),
            }
        }
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return fail(&e.to_string()),
    };
    let mut out = String::new();
    let result = commands::run_install(paths, &scope, install, args.dry_run, &exe, lang, &mut out);
    print!("{out}");
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn ui_lang(paths: &Paths) -> Lang {
    Config::load(&paths.config()).map_or(Lang::En, |c| c.ui.language)
}

fn query_daemon(paths: &Paths, req: &Request) -> ExitCode {
    let config = match Config::load(&paths.config()) {
        Ok(c) => c,
        Err(e) => return fail(&e.to_string()),
    };
    ensure_daemon(paths, launcher(&config), STARTUP_WAIT);
    match request(&paths.socket(), req, QUERY_TIMEOUT) {
        Ok(Response::Text { text }) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Response::Status(s)) => {
            print!("{}", s.render(config.ui.language));
            ExitCode::SUCCESS
        }
        Ok(Response::Done) => {
            println!(
                "{}",
                tr!(
                    config.ui.language,
                    "Reindex started. Check progress with `hippo status`.",
                    "재색인을 시작했습니다. 진행 상태는 `hippo status`로 확인할 수 있습니다."
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Response::Error { message }) => fail(&message),
        Ok(other) => fail(&format!("unexpected response: {other:?}")),
        Err(e) => fail(&e.to_string()),
    }
}

/// `dry_run` 요청으로 미리 보기를 출력하고, 확인을 거쳐 삭제를 요청한다. (FR-DEL-4)
fn run_delete(paths: &Paths, make: impl Fn(bool) -> Request, yes: bool, dry_run: bool) -> ExitCode {
    let config = match Config::load(&paths.config()) {
        Ok(c) => c,
        Err(e) => return fail(&e.to_string()),
    };
    let lang = config.ui.language;
    ensure_daemon(paths, launcher(&config), STARTUP_WAIT);
    // 실제 삭제는 끝날 때까지 기다린다. 응답이 곧 삭제 결과이다. (FR-DEL-4)
    let send = |dry_run: bool| {
        let reply = if dry_run {
            request(&paths.socket(), &make(true), QUERY_TIMEOUT)
        } else {
            request_until_reply(&paths.socket(), &make(false))
        };
        match reply {
            Ok(Response::Delete { report }) => Ok(report),
            Ok(Response::Error { message }) => Err(message),
            Ok(other) => Err(format!("unexpected response: {other:?}")),
            Err(e) => Err(e.to_string()),
        }
    };
    let report = match send(true) {
        Ok(Some(report)) => report,
        Ok(None) => {
            println!("{}", tr!(lang, "Nothing to delete.", "대상 없음"));
            return ExitCode::SUCCESS;
        }
        Err(message) => return fail(&message),
    };
    print!("{}", render_delete_report(&report, lang));
    if dry_run {
        return ExitCode::SUCCESS;
    }
    let ask = || {
        print!("{}", tr!(lang, "Delete? [y/N] ", "지울까요? [y/N] "));
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok().map(|_| line)
    };
    match confirm_delete(yes, std::io::stdin().is_terminal(), ask) {
        Confirm::Proceed => {}
        Confirm::Cancel => {
            println!("{}", tr!(lang, "Cancelled.", "취소했습니다."));
            return ExitCode::SUCCESS;
        }
        Confirm::NeedYes => {
            return fail(&tr!(
                lang,
                "stdin is not a terminal; pass --yes to delete",
                "표준 입력이 터미널이 아닙니다. 지우려면 --yes를 주십시오"
            ));
        }
    }
    match send(false) {
        Ok(Some(_)) => {
            println!("{}", tr!(lang, "Deleted.", "지웠습니다."));
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("{}", tr!(lang, "Nothing to delete.", "대상 없음"));
            ExitCode::SUCCESS
        }
        Err(message) => fail(&message),
    }
}

/// 준비 요청으로 후보를 받고, 세션마다 요청을 하나씩 보내며 진행률을 출력한다. (FR-IMP-1)
fn run_import(
    paths: &Paths,
    cwd: &str,
    from: Option<String>,
    to: Option<String>,
    wait: bool,
    dry_run: bool,
) -> ExitCode {
    let from = match from {
        Some(f) if is_project_id(&f) => f,
        Some(f) => absolute(cwd, &f),
        None => cwd.to_string(),
    };
    let to = to.map_or_else(|| cwd.to_string(), |t| absolute(cwd, &t));
    let config = match Config::load(&paths.config()) {
        Ok(c) => c,
        Err(e) => return fail(&e.to_string()),
    };
    let lang = config.ui.language;
    ensure_daemon(paths, launcher(&config), STARTUP_WAIT);
    // 실제 준비와 세션 가져오기는 끝날 때까지 기다린다. (FR-IMP-1)
    let prepare = Request::ImportPrepare { from, to, dry_run };
    let reply = if dry_run {
        request(&paths.socket(), &prepare, QUERY_TIMEOUT)
    } else {
        request_until_reply(&paths.socket(), &prepare)
    };
    let (project_id, summary, sessions) = match reply {
        Ok(Response::ImportPlan {
            project_id,
            summary,
            sessions,
            ..
        }) => (project_id, summary, sessions),
        Ok(Response::Error { message }) => return fail(&message),
        Ok(other) => return fail(&format!("unexpected response: {other:?}")),
        Err(e) => return fail(&e.to_string()),
    };
    if dry_run {
        print!("{}", render_preview(&summary, &sessions, lang));
        return ExitCode::SUCCESS;
    }
    print!("{}", escape_control(&summary));
    let total = sessions.len();
    println!(
        "{}",
        tr!(lang, "Candidate sessions: {total}", "후보 세션 {total}개")
    );
    let mut tally = ImportTally::default();
    for (n, c) in sessions.into_iter().enumerate() {
        let req = Request::ImportSession {
            project_id: project_id.clone(),
            session_id: c.session_id.clone(),
            transcript_path: c.transcript_path,
        };
        let line = match request_until_reply(&paths.socket(), &req) {
            Ok(Response::ImportSession(r)) => {
                session_line(n + 1, total, &c.session_id, Ok(&r), &mut tally, lang)
            }
            Ok(Response::Error { message }) => {
                session_line(n + 1, total, &c.session_id, Err(&message), &mut tally, lang)
            }
            Ok(other) => return fail(&format!("unexpected response: {other:?}")),
            Err(e) => return fail(&e.to_string()),
        };
        println!("{line}");
    }
    print!("{}", tally.summary(total, lang));
    if wait {
        return wait_for_embedding(paths, &project_id, lang);
    }
    ExitCode::SUCCESS
}

/// 대상 프로젝트의 임베딩 대기 건수가 0이 될 때까지 진행률 한 줄을 갱신한다. (IF-CLI-9)
fn wait_for_embedding(paths: &Paths, project_id: &str, lang: Lang) -> ExitCode {
    loop {
        let project = match request(&paths.socket(), &Request::Status, QUERY_TIMEOUT) {
            Ok(Response::Status(s)) => s.projects.into_iter().find(|p| p.id == project_id),
            Ok(Response::Error { message }) => return fail(&message),
            Ok(other) => return fail(&format!("unexpected response: {other:?}")),
            Err(e) => return fail(&e.to_string()),
        };
        let Some(project) = project else {
            return fail(&tr!(
                lang,
                "Project not found: {project_id}",
                "프로젝트가 없습니다: {project_id}"
            ));
        };
        if project.embed_jobs == 0 {
            println!(
                "\r{}",
                tr!(lang, "Embedding complete.", "임베딩을 마쳤습니다.")
            );
            return ExitCode::SUCCESS;
        }
        let pending = project.embed_jobs;
        let progress = embed_progress(pending, project.embed_per_min, lang);
        print!(
            "\r{}",
            tr!(
                lang,
                "Embedding: {pending} pending{progress}   ",
                "임베딩 중: {pending}건 남음{progress}   "
            )
        );
        let _ = std::io::stdout().flush();
        std::thread::sleep(WAIT_POLL);
    }
}

fn run_daemon(paths: Paths) -> ExitCode {
    let config = match Config::load(&paths.config()) {
        Ok(c) => c,
        Err(e) => return fail(&e.to_string()),
    };
    let embedder = match build_embedder(&config, &paths) {
        Ok(e) => e,
        Err(e) => return fail(&e.to_string()),
    };
    let source = match vector_source(&config) {
        Ok(s) => s,
        Err(e) => return fail(&e.to_string()),
    };
    match daemon::start(paths, config, embedder, source) {
        Ok(handle) => {
            handle.wait();
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e.to_string()),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("hippo: {message}");
    ExitCode::from(1)
}
