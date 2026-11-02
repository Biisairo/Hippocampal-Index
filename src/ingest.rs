//! transcript를 읽어 원본 저장소에 반영하는 수집 파이프라인. (SRS 3.2.1~3.2.3, 3.2.11)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bg::{BackgroundKind, TaskNotification, background_start, extract_agent_report};
use crate::embed_text::build_embed_text;
use crate::redact::Redactor;
use crate::store::{Batch, NewCompletion, NewIteration, RecordStore, StoreError};
use crate::transcript::{Event, Extractor, ExtractorState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEvent {
    PostToolBatch,
    Stop,
    StopFailure,
    SessionEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestRequest {
    pub event: HookEvent,
    pub session_id: String,
    pub transcript_path: PathBuf,
    /// hook 입력의 `prompt_id`. 처음 보는 세션의 기준 턴이다. (FR-CAP-7, FR-EXT-12)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub iterations: usize,
    pub completions: usize,
}

/// 세션 하나를 가져온 결과. (FR-IMP-7)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportOutcome {
    Imported,
    Unchanged,
    OtherProject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub outcome: ImportOutcome,
    pub iterations: usize,
    pub completions: usize,
    pub fork_skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestOptions {
    pub max_tool_input_chars: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("transcript read failed ({path}): {source}")]
    Transcript {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// 읽은 범위 끝에서 열린 반복을 어떻게 다룰지.
#[derive(Clone, Copy)]
enum Finish {
    Hook(HookEvent),
    /// 모든 도구 호출에 결과가 있는 반복만 확정한다. (FR-IMP-3)
    Complete,
}

/// 저장할 반복과 완료 기록. fork 복사본으로 건너뛴 반복 수를 함께 센다.
struct Collected {
    iterations: Vec<NewIteration>,
    completions: Vec<NewCompletion>,
    fork_skipped: usize,
}

/// 실시간 수집. 처음 보는 세션은 hook의 `prompt_id`가 가리키는 턴부터 읽는다. (FR-EXT-1, FR-EXT-12)
pub fn ingest(
    records: &mut RecordStore,
    redactor: &Redactor,
    options: IngestOptions,
    req: &IngestRequest,
) -> Result<IngestReport, IngestError> {
    let bytes = read_transcript(&req.transcript_path)?;
    let known = records
        .session_state(&req.session_id)?
        .filter(|s| s.safe_offset <= bytes.len() as u64);
    let state = match known {
        Some(state) => state,
        None => {
            let Some(anchor) = req
                .prompt_id
                .as_deref()
                .and_then(|p| prompt_start(&bytes, p))
            else {
                return Ok(IngestReport::default());
            };
            ExtractorState {
                safe_offset: anchor,
                start_offset: anchor,
                ..ExtractorState::default()
            }
        }
    };
    let mut extractor = Extractor::new(state.clone());
    let events = read_events(
        &mut extractor,
        &bytes,
        state.safe_offset as usize,
        Finish::Hook(req.event),
    );
    let collected = collect(records, redactor, options, events, req, false)?;
    let report = IngestReport {
        iterations: collected.iterations.len(),
        completions: collected.completions.len(),
    };
    save(records, req, collected, extractor.state(), false)?;
    Ok(report)
}

/// 가져오기. 처음 보는 세션은 처음부터, 실시간 수집이 중간부터 시작한 세션은 그 앞부분을 채운다.
/// fork 복사본은 건너뛴다. (FR-IMP-3, FR-IMP-4, FR-IMP-5)
pub fn import_session(
    records: &mut RecordStore,
    redactor: &Redactor,
    options: IngestOptions,
    session_id: &str,
    transcript_path: &Path,
) -> Result<ImportReport, IngestError> {
    let bytes = read_transcript(transcript_path)?;
    let req = IngestRequest {
        event: HookEvent::Stop,
        session_id: session_id.to_string(),
        transcript_path: transcript_path.to_path_buf(),
        prompt_id: None,
    };
    let known = records
        .session_state(session_id)?
        .filter(|s| s.safe_offset <= bytes.len() as u64);
    let (collected, state) = match known {
        Some(live) if live.start_offset > 0 => {
            let end = (live.start_offset as usize).min(bytes.len());
            let mut extractor = Extractor::new(ExtractorState::default());
            let events = read_events(
                &mut extractor,
                &bytes[..end],
                0,
                Finish::Hook(HookEvent::Stop),
            );
            let collected = collect(records, redactor, options, events, &req, true)?;
            (
                collected,
                ExtractorState {
                    start_offset: 0,
                    ..live
                },
            )
        }
        known => {
            let state = known.unwrap_or_default();
            let mut extractor = Extractor::new(state.clone());
            let events = read_events(
                &mut extractor,
                &bytes,
                state.safe_offset as usize,
                Finish::Complete,
            );
            let collected = collect(records, redactor, options, events, &req, true)?;
            (collected, extractor.state())
        }
    };
    let report = ImportReport {
        outcome: if collected.iterations.is_empty() && collected.completions.is_empty() {
            ImportOutcome::Unchanged
        } else {
            ImportOutcome::Imported
        },
        iterations: collected.iterations.len(),
        completions: collected.completions.len(),
        fork_skipped: collected.fork_skipped,
    };
    save(records, &req, collected, state, true)?;
    Ok(report)
}

fn read_transcript(path: &Path) -> Result<Vec<u8>, IngestError> {
    fs::read(path).map_err(|source| IngestError::Transcript {
        path: path.to_path_buf(),
        source,
    })
}

/// `promptId`가 같은 첫 user 라인의 시작 위치. 개행으로 끝난 라인만 본다. (FR-EXT-12)
fn prompt_start(bytes: &[u8], prompt_id: &str) -> Option<u64> {
    let end = bytes.iter().rposition(|b| *b == b'\n')? + 1;
    let mut pos = 0usize;
    for raw in bytes[..end].split_inclusive(|b| *b == b'\n') {
        let start = pos;
        pos += raw.len();
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(raw) else {
            continue;
        };
        let is_user = v.get("type").and_then(serde_json::Value::as_str) == Some("user");
        if is_user && v.get("promptId").and_then(serde_json::Value::as_str) == Some(prompt_id) {
            return Some(start as u64);
        }
    }
    None
}

fn collect(
    records: &RecordStore,
    redactor: &Redactor,
    options: IngestOptions,
    events: Vec<Event>,
    req: &IngestRequest,
    skip_forks: bool,
) -> Result<Collected, IngestError> {
    let mut iterations = Vec::new();
    let mut notifications = Vec::new();
    let mut fork_skipped = 0;
    for event in events {
        match event {
            Event::Iteration(mut it) => {
                if skip_forks && records.message_in_other_session(&it.message_id, &it.session_id)? {
                    fork_skipped += 1;
                    continue;
                }
                redactor.redact_iteration(&mut it);
                let embed_text = build_embed_text(&it, options.max_tool_input_chars);
                iterations.push(NewIteration {
                    iteration: it,
                    embed_text,
                });
            }
            Event::Notification {
                session_id,
                line_uuid,
                ts,
                notification,
            } => {
                notifications.push((session_id, line_uuid, ts, notification));
            }
        }
    }
    let batch_starts: HashMap<String, (BackgroundKind, String)> = iterations
        .iter()
        .flat_map(|n| n.iteration.tool_calls.iter())
        .filter_map(background_start)
        .map(|b| (b.tool_use_id, (b.kind, b.task_id)))
        .collect();
    let mut completions = Vec::new();
    for (session_id, line_uuid, ts, mut notification) in notifications {
        let start = match batch_starts.get(&notification.tool_use_id) {
            Some(start) => Some(start.clone()),
            None => records
                .bg_task(&notification.tool_use_id)?
                .map(|t| (t.kind, t.task_id)),
        };
        let result_text = start
            .and_then(|(kind, task_id)| {
                read_output(
                    kind,
                    &task_id,
                    &notification,
                    &req.session_id,
                    &req.transcript_path,
                )
            })
            .map(|t| redactor.redact(&t));
        notification.summary = redactor.redact(&notification.summary);
        completions.push(NewCompletion {
            session_id: if session_id.is_empty() {
                req.session_id.clone()
            } else {
                session_id
            },
            line_uuid,
            notification,
            result_text,
            ts,
        });
    }
    Ok(Collected {
        iterations,
        completions,
        fork_skipped,
    })
}

fn save(
    records: &mut RecordStore,
    req: &IngestRequest,
    collected: Collected,
    state: ExtractorState,
    bulk: bool,
) -> Result<(), IngestError> {
    records.apply(&Batch {
        session_id: req.session_id.clone(),
        transcript_path: req.transcript_path.to_string_lossy().into_owned(),
        iterations: collected.iterations,
        completions: collected.completions,
        state,
        bulk,
    })?;
    Ok(())
}

/// 세션 시작 경로: 개행으로 끝난 라인 중 `cwd` 필드를 가진 첫 라인의 값. (FR-PRJ-1)
pub fn session_start_cwd(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().rposition(|b| *b == b'\n')?;
    bytes[..end].split(|b| *b == b'\n').find_map(|raw| {
        serde_json::from_slice::<serde_json::Value>(raw)
            .ok()?
            .get("cwd")?
            .as_str()
            .map(str::to_string)
    })
}

/// 개행으로 끝난 라인까지만 읽는다. 기록 중인 마지막 라인은 다음 처리로 미룬다.
fn read_events(
    extractor: &mut Extractor,
    bytes: &[u8],
    start: usize,
    finish: Finish,
) -> Vec<Event> {
    let tail = &bytes[start..];
    let end = tail.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut events = Vec::new();
    let mut pos = start;
    for raw in tail[..end].split_inclusive(|b| *b == b'\n') {
        let next = pos + raw.len();
        let line = String::from_utf8_lossy(raw);
        events.extend(extractor.feed_line(
            line.trim_end_matches(['\n', '\r']),
            pos as u64,
            next as u64,
        ));
        pos = next;
    }
    events.extend(match finish {
        Finish::Hook(HookEvent::PostToolBatch) => extractor.finish_batch(),
        Finish::Hook(HookEvent::Stop | HookEvent::StopFailure) => extractor.finish_turn(),
        Finish::Hook(HookEvent::SessionEnd) => extractor.finish_session(),
        Finish::Complete => extractor.finish_complete(),
    });
    events
}

/// 백그라운드 결과를 읽는다. 알려진 시작 호출과 작업 id가 일치하고 경로가 실측 배치와 맞을 때만 읽는다.
/// 위조된 알림으로 임의 파일을 읽지 않기 위한 제한이다. (FR-BG-4, 5, 7)
fn read_output(
    kind: BackgroundKind,
    task_id: &str,
    notification: &TaskNotification,
    session_id: &str,
    transcript_path: &Path,
) -> Option<String> {
    if notification.task_id != task_id {
        return None;
    }
    let path = Path::new(notification.output_file.as_deref()?);
    if !is_task_output_path(path, session_id, task_id) {
        return None;
    }
    match kind {
        BackgroundKind::Bash => {
            let canonical = fs::canonicalize(path).ok()?;
            if !is_task_output_path(&canonical, session_id, task_id) {
                return None;
            }
            read_regular_file_nofollow(&canonical)
        }
        BackgroundKind::Agent => {
            let expected = fs::canonicalize(
                transcript_path
                    .parent()?
                    .join(session_id)
                    .join(SUBAGENTS_DIR),
            )
            .ok()?
            .join(format!("agent-{task_id}.jsonl"));
            if fs::canonicalize(path).ok()? != expected {
                return None;
            }
            extract_agent_report(fs::read_to_string(&expected).ok()?.lines())
        }
    }
}

const TASKS_DIR: &str = "tasks";
const SUBAGENTS_DIR: &str = "subagents";

/// 링크를 따르지 않고 연 뒤, 열린 핸들로 일반 파일인지 확인하고 읽는다. 검사와 읽기 사이의 경쟁을 없앤다.
fn read_regular_file_nofollow(path: &Path) -> Option<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// 절대 경로이고, `.`/`..` 없이 `…/<session_id>/tasks/<task_id>.output` 형태인지 확인한다.
fn is_task_output_path(path: &Path, session_id: &str, task_id: &str) -> bool {
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let file_name = format!("{task_id}.output");
    let mut parts = path.components().rev().map(|c| c.as_os_str());
    parts.next() == Some(file_name.as_ref())
        && parts.next() == Some(TASKS_DIR.as_ref())
        && parts.next() == Some(session_id.as_ref())
}
