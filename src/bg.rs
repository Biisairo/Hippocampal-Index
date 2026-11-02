//! 백그라운드 작업의 시작 호출과 완료 알림을 해석한다. (SRS 3.2.3)

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::transcript::ToolCall;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundKind {
    Bash,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundStart {
    pub tool_use_id: String,
    pub kind: BackgroundKind,
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNotification {
    pub task_id: String,
    pub tool_use_id: String,
    pub output_file: Option<String>,
    pub status: String,
    pub summary: String,
}

/// 백그라운드로 시작된 호출이면 대기 기록을 만들 정보를 반환한다. (FR-BG-1)
pub fn background_start(call: &ToolCall) -> Option<BackgroundStart> {
    if call.input.get("run_in_background").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let (kind, id_field) = match call.name.as_str() {
        "Bash" => (BackgroundKind::Bash, "backgroundTaskId"),
        "Agent" => (BackgroundKind::Agent, "agentId"),
        _ => return None,
    };
    let task_id = call
        .result
        .as_ref()?
        .structured
        .as_ref()?
        .get(id_field)?
        .as_str()?;
    Some(BackgroundStart {
        tool_use_id: call.id.clone(),
        kind,
        task_id: task_id.to_string(),
    })
}

/// `<task-notification>` 블록을 파싱한다. (FR-BG-2)
pub fn parse_task_notification(text: &str) -> Option<TaskNotification> {
    let block = tag_content(text, "task-notification")?;
    Some(TaskNotification {
        task_id: tag_content(block, "task-id")?.to_string(),
        tool_use_id: tag_content(block, "tool-use-id")?.to_string(),
        output_file: tag_content(block, "output-file").map(str::to_string),
        status: tag_content(block, "status").unwrap_or_default().to_string(),
        summary: tag_content(block, "summary")
            .unwrap_or_default()
            .to_string(),
    })
}

/// 서브에이전트 transcript에서 최종 보고를 추출한다. (FR-BG-5)
pub fn extract_agent_report<'a>(lines: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut handback = None;
    let mut last_text = None;
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_use")
                    if b.get("name").and_then(Value::as_str) == Some(HANDBACK_TOOL) =>
                {
                    if let Some(m) = b.pointer("/input/message").and_then(Value::as_str) {
                        handback = Some(m.to_string());
                    }
                }
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        last_text = Some(t.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    handback.or(last_text)
}

const HANDBACK_TOOL: &str = "SubagentHandback";

fn tag_content<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&close)?;
    let content = text[start..end].trim();
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}
