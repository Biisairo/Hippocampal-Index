use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use smem::backends::{build_embedder, vector_source};
use smem::client::{request, run_hook};
use smem::config::{Config, Paths};
use smem::daemon;
use smem::protocol::{Request, Response};
use smem::search::{SearchKind, SearchRequest};
use smem::setup::{hooks_config, launchd_plist};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// UserPromptSubmit hook timeout(초)은 시간 예산보다 이만큼 여유를 둔다.
const INJECT_TIMEOUT_MARGIN_SECS: u64 = 2;

#[derive(Parser)]
#[command(name = "smem", about = "Claude Code 세션 기억 저장소")]
struct Cli {
    /// 데이터 루트 (기본: ~/.session-memory)
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
            let budget_ms = Config::load(&paths.config())
                .map_or(Config::default().inject.budget_ms, |c| c.inject.budget_ms);
            if let Some(out) = run_hook(&paths, Duration::from_millis(budget_ms), &event, &input) {
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
        Command::HooksConfig => {
            let budget_ms = match Config::load(&paths.config()) {
                Ok(c) => c.inject.budget_ms,
                Err(e) => return fail(&e.to_string()),
            };
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(e) => return fail(&e.to_string()),
            };
            let timeout = budget_ms.div_ceil(1_000) + INJECT_TIMEOUT_MARGIN_SECS;
            println!(
                "{}",
                serde_json::to_string_pretty(&hooks_config(&exe, timeout)).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Command::LaunchdPlist => match std::env::current_exe() {
            Ok(exe) => {
                print!("{}", launchd_plist(&exe, &paths));
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e.to_string()),
        },
    }
}

fn query_daemon(paths: &Paths, req: &Request) -> ExitCode {
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
            println!("재색인을 시작했습니다. 진행 상태는 `smem status`로 확인할 수 있습니다.");
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
    let embedder = match build_embedder(&config) {
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
    eprintln!("smem: {message}");
    ExitCode::from(1)
}
