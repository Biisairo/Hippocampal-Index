//! hook 클라이언트와 소켓 요청. (IF-HK-1~3, IF-HK-5, FR-CAP-1, FR-CAP-5)

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::Paths;
use crate::fsutil::{ensure_private_dir, open_private_append};
use crate::ingest::{HookEvent, IngestRequest};
use crate::protocol::{Request, Response};
use crate::spool;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("daemon not reachable: {0}")]
    Connect(std::io::Error),
    #[error("daemon request failed: {0}")]
    Io(std::io::Error),
    #[error("invalid daemon response: {0}")]
    Protocol(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookAction {
    Skip,
    Ingest(IngestRequest),
    Inject {
        prompt: String,
        cwd: String,
        session_id: String,
    },
}

pub const INGEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 데몬을 띄운 뒤 소켓이 열릴 때까지 기다리는 최대 시간. (FR-DMN-7)
pub const STARTUP_WAIT: Duration = Duration::from_secs(5);
const CONNECT_POLL: Duration = Duration::from_millis(20);
const DAEMON_STDERR: &str = "daemon.err";

/// 데몬을 띄우는 방법. 실제로는 `spawn_daemon_process`, 자동 시작을 끄면 `launch_disabled`.
pub type Launcher<'a> = &'a dyn Fn(&Paths) -> std::io::Result<()>;

/// 같은 실행 파일로 `daemon`을 분리된 프로세스로 띄운다. stderr는 `logs/daemon.err`에 덧붙인다.
pub fn spawn_daemon_process(paths: &Paths) -> std::io::Result<()> {
    ensure_private_dir(&paths.logs())?;
    let stderr = open_private_append(&paths.logs().join(DAEMON_STDERR))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--root")
        .arg(&paths.root)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(|_| ())
}

pub fn launch_disabled(_paths: &Paths) -> std::io::Result<()> {
    Err(std::io::Error::other("daemon autostart is disabled"))
}

/// 데몬에 연결되면 true. 연결되지 않으면 띄운 뒤 `wait` 동안 연결을 기다린다.
pub fn ensure_daemon(paths: &Paths, launcher: Launcher, wait: Duration) -> bool {
    let socket = paths.socket();
    if UnixStream::connect(&socket).is_ok() {
        return true;
    }
    if launcher(paths).is_err() {
        return false;
    }
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if UnixStream::connect(&socket).is_ok() {
            return true;
        }
        std::thread::sleep(CONNECT_POLL);
    }
    false
}

pub fn request(socket: &Path, req: &Request, timeout: Duration) -> Result<Response, ClientError> {
    exchange(socket, req, Some(timeout))
}

/// 응답이 올 때까지 시간 제한 없이 기다린다. 응답 전에 연결이 끊기면 오류. (FR-DEL-4)
pub fn request_until_reply(socket: &Path, req: &Request) -> Result<Response, ClientError> {
    exchange(socket, req, None)
}

fn exchange(
    socket: &Path,
    req: &Request,
    timeout: Option<Duration>,
) -> Result<Response, ClientError> {
    let mut stream = UnixStream::connect(socket).map_err(ClientError::Connect)?;
    stream.set_read_timeout(timeout).map_err(ClientError::Io)?;
    stream.set_write_timeout(timeout).map_err(ClientError::Io)?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(ClientError::Io)?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(ClientError::Io)?;
    Ok(serde_json::from_str(&reply)?)
}

/// hook 이름(`post-tool-batch` 등)과 stdin JSON으로 할 일을 정한다.
pub fn parse_hook(event_arg: &str, input: &str) -> HookAction {
    let Ok(v) = serde_json::from_str::<Value>(input) else {
        return HookAction::Skip;
    };
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let capture = match event_arg {
        "post-tool-batch" => Some(HookEvent::PostToolBatch),
        "stop" => Some(HookEvent::Stop),
        "stop-failure" => Some(HookEvent::StopFailure),
        "session-end" => Some(HookEvent::SessionEnd),
        _ => None,
    };
    if let Some(event) = capture {
        if v.get("agent_id").is_some() {
            return HookAction::Skip;
        }
        return match (field("session_id"), field("transcript_path")) {
            (Some(session_id), Some(path)) => HookAction::Ingest(IngestRequest {
                event,
                session_id,
                transcript_path: PathBuf::from(path),
                prompt_id: field("prompt_id"),
            }),
            _ => HookAction::Skip,
        };
    }
    if event_arg != "user-prompt-submit" {
        return HookAction::Skip;
    }
    match (field("prompt"), field("cwd"), field("session_id")) {
        (Some(prompt), Some(cwd), Some(session_id)) => HookAction::Inject {
            prompt,
            cwd,
            session_id,
        },
        _ => HookAction::Skip,
    }
}

/// UserPromptSubmit hook의 stdout JSON.
pub fn inject_output(context: &str) -> String {
    json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": context}}).to_string()
}

/// hook을 실행하고 stdout에 쓸 내용을 돌려준다. 어떤 경우에도 실패를 밖으로 내지 않는다. (QA-5)
/// 자동 주입은 데몬에 바로 연결되지 않아 새로 띄워야 하면 `cold_budget`을 쓴다. (FR-RET-5)
pub fn run_hook(
    paths: &Paths,
    budget: Duration,
    cold_budget: Duration,
    event_arg: &str,
    input: &str,
    launcher: Launcher,
) -> Option<String> {
    match parse_hook(event_arg, input) {
        HookAction::Skip => None,
        HookAction::Ingest(req) => {
            let message = Request::Ingest(req.clone());
            // 데몬이 응답했으면(오류 응답 포함) 전달된 것으로 본다. 오류 요청을 스풀에 넣으면 같은 오류가 반복된다.
            let delivered = request(&paths.socket(), &message, INGEST_TIMEOUT).is_ok()
                || (ensure_daemon(paths, launcher, STARTUP_WAIT)
                    && request(&paths.socket(), &message, INGEST_TIMEOUT).is_ok());
            if !delivered {
                let _ = spool::append(&paths.spool(), &req);
            }
            None
        }
        HookAction::Inject {
            prompt,
            cwd,
            session_id,
        } => {
            let started = Instant::now();
            let budget = if UnixStream::connect(paths.socket()).is_ok() {
                budget
            } else {
                cold_budget
            };
            if !ensure_daemon(paths, launcher, budget) {
                return None;
            }
            // 모델이 내려가 있으면 데몬이 콜드 예산까지 늘리므로 그만큼 기다린다. (FR-RET-5)
            let remaining = cold_budget.max(budget).saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return None;
            }
            match request(
                &paths.socket(),
                &Request::Inject {
                    prompt,
                    cwd,
                    session_id,
                    budget_ms: Some(budget.as_millis() as u64),
                },
                remaining,
            ) {
                Ok(Response::Inject {
                    context: Some(context),
                }) => Some(inject_output(&context)),
                _ => None,
            }
        }
    }
}
