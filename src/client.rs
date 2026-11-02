//! hook 클라이언트와 소켓 요청. (IF-HK-1~3, IF-HK-5, FR-CAP-1, FR-CAP-5)

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Paths;
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

pub fn request(socket: &Path, req: &Request, timeout: Duration) -> Result<Response, ClientError> {
    let mut stream = UnixStream::connect(socket).map_err(ClientError::Connect)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(ClientError::Io)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(ClientError::Io)?;
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
pub fn run_hook(paths: &Paths, budget: Duration, event_arg: &str, input: &str) -> Option<String> {
    match parse_hook(event_arg, input) {
        HookAction::Skip => None,
        HookAction::Ingest(req) => {
            if let Err(ClientError::Connect(_)) = request(
                &paths.socket(),
                &Request::Ingest(req.clone()),
                INGEST_TIMEOUT,
            ) {
                let _ = spool::append(&paths.spool(), &req);
            }
            None
        }
        HookAction::Inject {
            prompt,
            cwd,
            session_id,
        } => {
            match request(
                &paths.socket(),
                &Request::Inject {
                    prompt,
                    cwd,
                    session_id,
                },
                budget,
            ) {
                Ok(Response::Inject {
                    context: Some(context),
                }) => Some(inject_output(&context)),
                _ => None,
            }
        }
    }
}
