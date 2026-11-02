use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use hippo::backends::{build_embedder, vector_source};
use hippo::client::{
    Launcher, STARTUP_WAIT, ensure_daemon, launch_disabled, request, run_hook, spawn_daemon_process,
};
use hippo::config::{Config, Paths};
use hippo::daemon;
use hippo::ingest::ImportOutcome;
use hippo::install::{Action, Scope, apply, target_file};
use hippo::protocol::{ImportSessionReport, Request, Response, Scope as QueryScope};
use hippo::registry::is_project_id;
use hippo::search::{SearchKind, escape_control};
use hippo::setup::{hooks_config, launchd_plist};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// UserPromptSubmit hook timeout(초)은 시간 예산보다 이만큼 여유를 둔다.
const INJECT_TIMEOUT_MARGIN_SECS: u64 = 2;

#[derive(Parser)]
#[command(
    name = "hippo",
    about = "Hippocampal Index: Claude Code 세션을 위한 장기 기억"
)]
struct Cli {
    /// 데이터 루트 (기본: ~/.hippo)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Claude Code hook 진입점. stdin으로 hook 입력을 받는다.
    Hook { event: String },
    /// 기억 검색
    Search {
        query: String,
        /// project(현재 디렉터리의 프로젝트), all, 또는 경로
        #[arg(long, default_value = SCOPE_PROJECT)]
        scope: String,
        #[arg(long, value_enum, default_value = "iteration")]
        kind: KindArg,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// 한 턴의 반복을 순서대로 보여준다.
    Show {
        prompt_id: String,
        #[arg(long)]
        with_results: bool,
        /// project 또는 경로. 없으면 현재 프로젝트를 먼저 찾고 나머지 프로젝트를 찾는다.
        #[arg(long)]
        scope: Option<String>,
    },
    /// 데몬을 포그라운드로 실행한다.
    Daemon,
    /// 데몬 상태
    Status,
    /// 현재 설정의 임베더와 벡터 저장소로 다시 색인한다.
    Reindex {
        /// all, project, 또는 경로
        #[arg(long, default_value = SCOPE_ALL)]
        scope: String,
    },
    /// 프로젝트 목록
    #[command(subcommand)]
    Project(ProjectCommand),
    /// 과거 세션 기록을 프로젝트로 가져온다. 생략한 인자는 현재 디렉터리이다.
    Import {
        /// 가져올 기록의 경로(이전 경로 가능) 또는 프로젝트 id
        #[arg(long)]
        from: Option<String>,
        /// 기록을 넣을 프로젝트의 경로
        #[arg(long)]
        to: Option<String>,
    },
    /// settings.json에 넣을 hook 설정
    HooksConfig,
    /// launchd plist
    LaunchdPlist,
    /// Claude Code 설정 파일에 hook을 병합한다.
    Install(InstallArgs),
    /// Claude Code 설정 파일에서 hippo hook을 제거한다.
    Uninstall(InstallArgs),
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// 프로젝트 목록
    List,
}

const SCOPE_PROJECT: &str = "project";
const SCOPE_ALL: &str = "all";

#[derive(clap::Args)]
struct InstallArgs {
    #[arg(long, value_enum)]
    scope: ScopeArg,
    /// 프로젝트 경로 (기본: 현재 디렉터리)
    #[arg(long)]
    project: Option<PathBuf>,
    /// 변경 내용만 출력하고 쓰지 않는다.
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
        Command::Import { from, to } => match current_dir() {
            Ok(cwd) => run_import(&paths, &cwd, from, to),
            Err(e) => fail(&e),
        },
        Command::Daemon => run_daemon(paths),
        Command::HooksConfig => match hook_settings(&paths) {
            Ok(hooks) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&hooks).unwrap_or_default()
                );
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
        Command::Install(args) => match hook_settings(&paths) {
            Ok(hooks) => run_install(&args, Action::Install(hooks)),
            Err(e) => fail(&e),
        },
        Command::Uninstall(args) => run_install(&args, Action::Uninstall),
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
fn hook_settings(paths: &Paths) -> Result<serde_json::Value, String> {
    let budget_ms = Config::load(&paths.config())
        .map_err(|e| e.to_string())?
        .inject
        .budget_ms;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let timeout = budget_ms.div_ceil(1_000) + INJECT_TIMEOUT_MARGIN_SECS;
    Ok(hooks_config(&exe, timeout))
}

fn run_install(args: &InstallArgs, action: Action) -> ExitCode {
    let scope = match args.scope {
        ScopeArg::User => Scope::User,
        ScopeArg::Project => match args.project.clone().map_or_else(std::env::current_dir, Ok) {
            Ok(dir) => Scope::Project(dir),
            Err(e) => return fail(&e.to_string()),
        },
    };
    let outcome = match target_file(&scope).and_then(|path| apply(&path, &action, args.dry_run)) {
        Ok(o) => o,
        Err(e) => return fail(&e.to_string()),
    };
    if !outcome.changed {
        println!("변경 없음: {}", outcome.path.display());
        return ExitCode::SUCCESS;
    }
    print!("{}", outcome.diff);
    if args.dry_run {
        println!(
            "(dry-run) 파일을 쓰지 않았습니다: {}",
            outcome.path.display()
        );
    } else {
        println!("적용했습니다: {}", outcome.path.display());
        if let Some(backup) = outcome.backup {
            println!("백업: {}", backup.display());
        }
    }
    ExitCode::SUCCESS
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
            print!("{}", s.render());
            ExitCode::SUCCESS
        }
        Ok(Response::Done) => {
            println!("재색인을 시작했습니다. 진행 상태는 `hippo status`로 확인할 수 있습니다.");
            ExitCode::SUCCESS
        }
        Ok(Response::Error { message }) => fail(&message),
        Ok(other) => fail(&format!("unexpected response: {other:?}")),
        Err(e) => fail(&e.to_string()),
    }
}

/// 준비 요청으로 후보를 받고, 세션마다 요청을 하나씩 보내며 진행률을 출력한다. (FR-IMP-1)
fn run_import(paths: &Paths, cwd: &str, from: Option<String>, to: Option<String>) -> ExitCode {
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
    println!("후보 세션 {total}개");
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
                format!(
                    "{} 반복 {}  남은 임베딩 {}",
                    outcome_label(r.outcome),
                    r.iterations,
                    r.pending_embeds
                )
            }
            Ok(Response::Error { message }) => {
                tally.errors += 1;
                format!("오류: {message}")
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
        "합계: 세션 {total}개 중 가져옴 {}, 변경 없음 {}, 다른 프로젝트 {}, 오류 {}. 반복 {}개 저장, fork 복사본 {}개 건너뜀.",
        tally.imported,
        tally.unchanged,
        tally.other,
        tally.errors,
        tally.iterations,
        tally.fork_skipped
    );
    if tally.pending_embeds > 0 {
        println!(
            "남은 임베딩 작업 {}개는 데몬이 계속 처리합니다. 진행 상태는 `hippo status`로 확인할 수 있습니다.",
            tally.pending_embeds
        );
    }
    ExitCode::SUCCESS
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

fn outcome_label(outcome: ImportOutcome) -> &'static str {
    match outcome {
        ImportOutcome::Imported => "가져옴",
        ImportOutcome::Unchanged => "변경 없음",
        ImportOutcome::OtherProject => "다른 프로젝트",
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
