//! transcript를 읽어 원본 저장소에 반영하는 수집 파이프라인. (SRS 3.2.1~3.2.3)

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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub iterations: usize,
    pub completions: usize,
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

pub fn ingest(
    records: &mut RecordStore,
    redactor: &Redactor,
    options: IngestOptions,
    req: &IngestRequest,
) -> Result<IngestReport, IngestError> {
    let bytes = fs::read(&req.transcript_path).map_err(|source| IngestError::Transcript {
        path: req.transcript_path.clone(),
        source,
    })?;
    let mut state = records.session_state(&req.session_id)?.unwrap_or_default();
    if state.safe_offset > bytes.len() as u64 {
        state = ExtractorState::default();
    }
    let mut extractor = Extractor::new(state.clone());
    let events = read_events(
        &mut extractor,
        &bytes,
        state.safe_offset as usize,
        req.event,
    );

    let mut iterations = Vec::new();
    let mut notifications = Vec::new();
    for event in events {
        match event {
            Event::Iteration(mut it) => {
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
    let batch_kinds: HashMap<String, BackgroundKind> = iterations
        .iter()
        .flat_map(|n| n.iteration.tool_calls.iter())
        .filter_map(background_start)
        .map(|b| (b.tool_use_id, b.kind))
        .collect();
    let mut completions = Vec::new();
    for (session_id, line_uuid, ts, mut notification) in notifications {
        let kind = match batch_kinds.get(&notification.tool_use_id) {
            Some(kind) => Some(*kind),
            None => records.bg_task(&notification.tool_use_id)?.map(|t| t.kind),
        };
        let result_text = read_output(kind, &notification).map(|t| redactor.redact(&t));
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

    let report = IngestReport {
        iterations: iterations.len(),
        completions: completions.len(),
    };
    records.apply(&Batch {
        session_id: req.session_id.clone(),
        transcript_path: req.transcript_path.to_string_lossy().into_owned(),
        iterations,
        completions,
        state: extractor.state(),
    })?;
    Ok(report)
}

/// 개행으로 끝난 라인까지만 읽는다. 기록 중인 마지막 라인은 다음 처리로 미룬다.
fn read_events(
    extractor: &mut Extractor,
    bytes: &[u8],
    start: usize,
    event: HookEvent,
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
    events.extend(match event {
        HookEvent::PostToolBatch => extractor.finish_batch(),
        HookEvent::Stop | HookEvent::StopFailure => extractor.finish_turn(),
        HookEvent::SessionEnd => extractor.finish_session(),
    });
    events
}

/// 백그라운드 결과를 읽는다. Agent는 서브에이전트 transcript에서 보고를 추출한다. (FR-BG-4, 5)
fn read_output(kind: Option<BackgroundKind>, notification: &TaskNotification) -> Option<String> {
    let path = Path::new(notification.output_file.as_deref()?);
    match kind {
        Some(BackgroundKind::Agent) => extract_agent_report(fs::read_to_string(path).ok()?.lines()),
        Some(BackgroundKind::Bash) | None => {
            Some(String::from_utf8_lossy(&fs::read(path).ok()?).into_owned())
        }
    }
}
