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
use hippo::install::{Action, Scope, apply, target_file};
use hippo::protocol::{Request, Response};
use hippo::search::{SearchKind, SearchRequest};
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
        /// cwd, all, 또는 프로젝트 경로
        #[arg(long, default_value = "cwd")]
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
    },
    /// 데몬을 포그라운드로 실행한다.
    Daemon,
    /// 데몬 상태
    Status,
    /// 현재 설정의 임베더와 벡터 저장소로 전체를 다시 색인한다.
    Reindex,
    /// settings.json에 넣을 hook 설정
    HooksConfig,
    /// launchd plist
    LaunchdPlist,
    /// Claude Code 설정 파일에 hook을 병합한다.
    Install(InstallArgs),
    /// Claude Code 설정 파일에서 hippo hook을 제거한다.
    Uninstall(InstallArgs),
}

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
            let cwd = match scope.as_str() {
                "all" => None,
                "cwd" => match std::env::current_dir() {
                    Ok(dir) => Some(dir.to_string_lossy().into_owned()),
                    Err(e) => return fail(&e.to_string()),
                },
                path => Some(path.to_string()),
            };
            let kind = match kind {
                KindArg::Iteration => SearchKind::Iteration,
                KindArg::Result => SearchKind::Result,
                KindArg::Both => SearchKind::Both,
            };
            query_daemon(
                &paths,
                &Request::Search(SearchRequest {
                    query,
                    kind,
                    cwd,
                    exclude_session: None,
                    limit,
                }),
            )
        }
        Command::Show {
            prompt_id,
            with_results,
        } => query_daemon(
            &paths,
            &Request::Show {
                prompt_id,
                with_results,
            },
        ),
        Command::Status => query_daemon(&paths, &Request::Status),
        Command::Reindex => query_daemon(&paths, &Request::Reindex),
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

fn run_daemon(paths: Paths) -> ExitCode {
    let config = match Config::load(&paths.config()) {
        Ok(c) => c,
        Err(e) => return fail(&e.to_string()),
    };
    let embedder = match build_embedder(&config, &paths) {
        Ok(e) => e,
        Err(e) => return fail(&e.to_string()),
    };
    let source = match vector_source(&config, &paths) {
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
