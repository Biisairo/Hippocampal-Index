use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use hippo::backends::{build_embedder, vector_source};
use hippo::client::{
    Launcher, STARTUP_WAIT, ensure_daemon, launch_disabled, request, run_hook, spawn_daemon_process,
};
use hippo::config::{Config, Paths};
use hippo::daemon;
use hippo::i18n::Lang;
use hippo::ingest::ImportOutcome;
use hippo::install::{
    Action, Outcome, Scope, apply, apply_memory, forget_install, guide_block, install_binary,
    installed_exe, memory_file, record_install, target_file, warn_untracked,
};
use hippo::protocol::{
    ImportSessionReport, Request, Response, Scope as QueryScope, embed_progress,
};
use hippo::registry::is_project_id;
use hippo::search::{SearchKind, escape_control};
use hippo::setup::{hooks_config, launchd_plist};
use hippo::tr;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// `import --wait`이 상태를 묻는 간격. (IF-CLI-9)
const WAIT_POLL: Duration = Duration::from_secs(2);
/// UserPromptSubmit hook timeout(초)은 시간 예산보다 이만큼 여유를 둔다.
const INJECT_TIMEOUT_MARGIN_SECS: u64 = 2;

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
    },
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
}

const SCOPE_PROJECT: &str = "project";
const SCOPE_ALL: &str = "all";

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
            if let Some(out) = run_hook(&paths, budget, &event, &input, launcher(&config)) {
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
        Command::Import { from, to, wait } => match current_dir() {
            Ok(cwd) => run_import(&paths, &cwd, from, to, wait),
            Err(e) => fail(&e),
        },
        Command::Daemon => run_daemon(paths),
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

/// 상대 경로는 클라이언트의 현재 디렉터리 기준으로 바꾼다. 데몬의 현재 디렉터리와 다르기 때문이다.
fn absolute(cwd: &str, path: &str) -> String {
    PathBuf::from(cwd).join(path).to_string_lossy().into_owned()
}

fn scope_arg(cwd: &str, scope: &str) -> QueryScope {
    match scope {
        SCOPE_PROJECT => QueryScope::Project,
        SCOPE_ALL => QueryScope::All,
        path => QueryScope::Path(absolute(cwd, path)),
    }
}

/// 실행 중인 hippo 경로로 hook 설정을 만든다. UserPromptSubmit timeout은 시간 예산보다 여유를 둔다.
fn hook_settings(paths: &Paths, exe: &Path) -> Result<serde_json::Value, String> {
    let budget_ms = Config::load(&paths.config())
        .map_err(|e| e.to_string())?
        .inject
        .budget_ms;
    let timeout = budget_ms.div_ceil(1_000) + INJECT_TIMEOUT_MARGIN_SECS;
    Ok(hooks_config(exe, timeout))
}

/// hook 설정과 메모리 파일의 안내 블록을 함께 바꾼다. (IF-CLI-6, FR-INS-7)
fn run_install(paths: &Paths, args: &InstallArgs, install: bool, lang: Lang) -> ExitCode {
    let scope = match args.scope {
        ScopeArg::User => Scope::User,
        ScopeArg::Project => match args
            .project
            .clone()
            .map_or_else(std::env::current_dir, Ok)
            .and_then(|dir| fs::canonicalize(&dir).or_else(|_| std::path::absolute(&dir)))
        {
            Ok(dir) => Scope::Project(dir),
            Err(e) => return fail(&e.to_string()),
        },
    };
    let root = match std::path::absolute(&paths.root) {
        Ok(root) => root,
        Err(e) => return fail(&e.to_string()),
    };
    let settings_path = match target_file(&scope) {
        Ok(path) => path,
        Err(e) => return fail(&e.to_string()),
    };
    let (action, block) = if install {
        let binary = match std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| install_binary(&exe, &root, args.dry_run).map_err(|e| e.to_string()))
        {
            Ok(b) => b,
            Err(e) => return fail(&e),
        };
        let path = binary.path.display();
        println!(
            "{}",
            match (binary.copied, args.dry_run) {
                (true, false) => tr!(
                    lang,
                    "Installed binary: {path}",
                    "실행 파일을 설치했습니다: {path}"
                ),
                (true, true) => tr!(
                    lang,
                    "(dry-run) Would install binary: {path}",
                    "(dry-run) 실행 파일을 설치할 위치: {path}"
                ),
                (false, _) => tr!(
                    lang,
                    "Binary up to date: {path}",
                    "실행 파일이 최신입니다: {path}"
                ),
            }
        );
        match hook_settings(paths, &binary.path) {
            Ok(hooks) => (Action::Install(hooks), Some(guide_block(&binary.path))),
            Err(e) => return fail(&e),
        }
    } else {
        (Action::Uninstall, None)
    };
    let settings = apply(&settings_path, &action, args.dry_run);
    let memory = settings.and_then(|settings| {
        memory_file(&scope)
            .and_then(|path| apply_memory(&path, block.as_deref(), args.dry_run))
            .map(|memory| (settings, memory))
    });
    let (settings, memory) = match memory {
        Ok(pair) => pair,
        Err(e) => return fail(&e.to_string()),
    };
    print_outcome(&settings, args.dry_run, lang);
    print_outcome(&memory, args.dry_run, lang);
    let recorded = if install {
        if args.dry_run {
            Ok(false)
        } else {
            record_install(&root, &settings_path).map(|()| false)
        }
    } else {
        forget_install(&root, &settings_path, args.dry_run)
    };
    match recorded {
        Ok(true) => {
            let path = installed_exe(&root);
            let path = path.display();
            let line = if args.dry_run {
                tr!(
                    lang,
                    "(dry-run) Would remove binary: {path}",
                    "(dry-run) 지울 실행 파일: {path}"
                )
            } else {
                tr!(
                    lang,
                    "Removed binary: {path}",
                    "실행 파일을 지웠습니다: {path}"
                )
            };
            println!("{line}");
        }
        Ok(false) => {}
        Err(e) => return fail(&e.to_string()),
    }
    if let Scope::Project(dir) = &scope
        && install
        && warn_untracked(dir)
    {
        let path = memory.path.display();
        println!(
            "{}",
            tr!(
                lang,
                "Warning: {path} is not ignored by git and may be committed. Add it to .gitignore if it should stay private.",
                "경고: {path}이(가) git에서 무시되지 않아 커밋될 수 있습니다. 개인 파일로 두려면 .gitignore에 추가하십시오."
            )
        );
    }
    ExitCode::SUCCESS
}

fn print_outcome(outcome: &Outcome, dry_run: bool, lang: Lang) {
    let path = outcome.path.display();
    if !outcome.changed {
        println!("{}", tr!(lang, "No change: {path}", "변경 없음: {path}"));
        return;
    }
    print!("{}", outcome.diff);
    if dry_run {
        println!(
            "{}",
            tr!(
                lang,
                "(dry-run) Nothing written: {path}",
                "(dry-run) 파일을 쓰지 않았습니다: {path}"
            )
        );
        return;
    }
    println!("{}", tr!(lang, "Applied: {path}", "적용했습니다: {path}"));
    if let Some(backup) = &outcome.backup {
        let backup = backup.display();
        println!("{}", tr!(lang, "Backup: {backup}", "백업: {backup}"));
    }
}

fn ui_lang(paths: &Paths) -> Lang {
    Config::load(&paths.config()).map_or(Lang::En, |c| c.ui.language)
}

/// 설정에 따라 데몬을 띄우는 방법을 고른다. (FR-DMN-7)
fn launcher(config: &Config) -> Launcher<'static> {
    if config.daemon.autostart {
        &spawn_daemon_process
    } else {
        &launch_disabled
    }
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

/// 준비 요청으로 후보를 받고, 세션마다 요청을 하나씩 보내며 진행률을 출력한다. (FR-IMP-1)
fn run_import(
    paths: &Paths,
    cwd: &str,
    from: Option<String>,
    to: Option<String>,
    wait: bool,
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
    let (project_id, summary, sessions) = match request(
        &paths.socket(),
        &Request::ImportPrepare { from, to },
        QUERY_TIMEOUT,
    ) {
        Ok(Response::ImportPlan {
            project_id,
            summary,
            sessions,
        }) => (project_id, summary, sessions),
        Ok(Response::Error { message }) => return fail(&message),
        Ok(other) => return fail(&format!("unexpected response: {other:?}")),
        Err(e) => return fail(&e.to_string()),
    };
    print!("{}", escape_control(&summary));
    let total = sessions.len();
    println!(
        "{}",
        tr!(lang, "Candidate sessions: {total}", "후보 세션 {total}개")
    );
    let mut tally = ImportTally::default();
    for (n, c) in sessions.into_iter().enumerate() {
        let short: String = c.session_id.chars().take(SESSION_ID_SHORT).collect();
        let req = Request::ImportSession {
            project_id: project_id.clone(),
            session_id: c.session_id,
            transcript_path: c.transcript_path,
        };
        let line = match request(&paths.socket(), &req, QUERY_TIMEOUT) {
            Ok(Response::ImportSession(r)) => {
                tally.add(&r);
                tr!(
                    lang,
                    "{}  iterations {}  pending embeddings {}",
                    "{} 반복 {}  남은 임베딩 {}",
                    outcome_label(r.outcome, lang),
                    r.iterations,
                    r.pending_embeds
                )
            }
            Ok(Response::Error { message }) => {
                tally.errors += 1;
                tr!(lang, "error: {message}", "오류: {message}")
            }
            Ok(other) => return fail(&format!("unexpected response: {other:?}")),
            Err(e) => return fail(&e.to_string()),
        };
        println!(
            "{}",
            escape_control(&format!("[{}/{total}] {short} {line}", n + 1))
        );
    }
    println!(
        "{}",
        tr!(
            lang,
            "Total: {total} sessions; imported {}, unchanged {}, other project {}, errors {}. Stored {} iterations, skipped {} fork copies.",
            "합계: 세션 {total}개 중 가져옴 {}, 변경 없음 {}, 다른 프로젝트 {}, 오류 {}. 반복 {}개 저장, fork 복사본 {}개 건너뜀.",
            tally.imported,
            tally.unchanged,
            tally.other,
            tally.errors,
            tally.iterations,
            tally.fork_skipped
        )
    );
    if tally.pending_embeds > 0 {
        println!(
            "{}",
            tr!(
                lang,
                "The daemon keeps working on the {} pending embedding jobs. Check progress with `hippo status`.",
                "남은 임베딩 작업 {}개는 데몬이 계속 처리합니다. 진행 상태는 `hippo status`로 확인할 수 있습니다.",
                tally.pending_embeds
            )
        );
    }
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

const SESSION_ID_SHORT: usize = 8;

#[derive(Default)]
struct ImportTally {
    imported: usize,
    unchanged: usize,
    other: usize,
    errors: usize,
    iterations: usize,
    fork_skipped: usize,
    pending_embeds: u64,
}

impl ImportTally {
    fn add(&mut self, r: &ImportSessionReport) {
        match r.outcome {
            ImportOutcome::Imported => self.imported += 1,
            ImportOutcome::Unchanged => self.unchanged += 1,
            ImportOutcome::OtherProject => self.other += 1,
        }
        self.iterations += r.iterations;
        self.fork_skipped += r.fork_skipped;
        self.pending_embeds = r.pending_embeds;
    }
}

fn outcome_label(outcome: ImportOutcome, lang: Lang) -> &'static str {
    match (outcome, lang) {
        (ImportOutcome::Imported, Lang::En) => "imported",
        (ImportOutcome::Unchanged, Lang::En) => "unchanged",
        (ImportOutcome::OtherProject, Lang::En) => "other project",
        (ImportOutcome::Imported, Lang::Ko) => "가져옴",
        (ImportOutcome::Unchanged, Lang::Ko) => "변경 없음",
        (ImportOutcome::OtherProject, Lang::Ko) => "다른 프로젝트",
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
