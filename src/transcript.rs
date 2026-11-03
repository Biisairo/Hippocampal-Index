//! Claude Code transcript JSONL을 agentic loop 반복 단위로 해석한다. (SRS 3.2.2)

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bg::{TaskNotification, parse_task_notification};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IterationInput {
    Prompt { text: String },
    TaskNotification(TaskNotification),
    AgentMessage { text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultData {
    pub text: String,
    pub is_error: bool,
    pub structured: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub result: Option<ToolResultData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Iteration {
    pub session_id: String,
    pub prompt_id: Option<String>,
    pub message_id: String,
    pub iteration_index: u32,
    pub cwd: String,
    pub ts: String,
    pub inputs: Vec<IterationInput>,
    pub texts: Vec<String>,
    pub tool_calls: Vec<ToolCall>,
}

/// 해석 결과. 완료 알림은 반복 완료를 기다리지 않고 즉시 내보낸다. (FR-BG-2)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Iteration(Iteration),
    Notification {
        session_id: String,
        line_uuid: String,
        ts: String,
        notification: TaskNotification,
    },
}

/// 안전 offset에서 해석을 재개하는 데 필요한 상태. (FR-EXT-1)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractorState {
    pub safe_offset: u64,
    /// 안전 offset 시점에 가장 최근 user 라인의 `promptId`. (FR-EXT-11)
    pub current_prompt_id: Option<String>,
    /// `next_index`가 기준으로 삼는 턴.
    pub indexed_prompt_id: Option<String>,
    pub next_index: u32,
    /// 기록을 저장하기 시작한 위치. 이 앞은 가져오기로만 채운다. (FR-EXT-12, FR-IMP-3)
    #[serde(default)]
    pub start_offset: u64,
}

/// 아직 확정되지 않은 항목. 시작 offset과 그 시점의 `promptId`를 함께 기억해 재개에 쓴다.
struct Pending<T> {
    start: u64,
    prompt_before: Option<String>,
    value: T,
}

pub struct Extractor {
    state: ExtractorState,
    last_end: u64,
    inputs: Vec<Pending<IterationInput>>,
    open: Option<Pending<Iteration>>,
}

impl Extractor {
    pub fn new(state: ExtractorState) -> Self {
        Self {
            last_end: state.safe_offset,
            state,
            inputs: Vec::new(),
            open: None,
        }
    }

    /// `start..end`는 이 라인의 transcript 내 바이트 범위다.
    pub fn feed_line(&mut self, line: &str, start: u64, end: u64) -> Vec<Event> {
        let mut events = Vec::new();
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            match v.get("type").and_then(Value::as_str) {
                Some("user") => self.on_user(&v, start, &mut events),
                Some("assistant") => self.on_assistant(&v, start, &mut events),
                _ => {}
            }
        }
        self.last_end = end;
        events
    }

    /// PostToolBatch 처리 요청. (FR-EXT-7 b)
    pub fn finish_batch(&mut self) -> Vec<Event> {
        let ready = self.open.as_ref().is_some_and(|p| {
            let calls = &p.value.tool_calls;
            !calls.is_empty() && calls.iter().all(|c| c.result.is_some())
        });
        if ready { self.close_open() } else { Vec::new() }
    }

    /// 가져오기에서 읽은 범위 끝. 모든 도구 호출에 결과가 있는 반복만 확정한다. (FR-IMP-3)
    pub fn finish_complete(&mut self) -> Vec<Event> {
        let ready = self
            .open
            .as_ref()
            .is_some_and(|p| p.value.tool_calls.iter().all(|c| c.result.is_some()));
        if ready { self.close_open() } else { Vec::new() }
    }

    /// Stop / StopFailure 처리 요청. (FR-EXT-7 c)
    pub fn finish_turn(&mut self) -> Vec<Event> {
        self.close_open()
    }

    /// SessionEnd 처리 요청. 반복에 붙지 않은 입력을 버린다. (FR-CAP-4)
    pub fn finish_session(&mut self) -> Vec<Event> {
        let events = self.close_open();
        self.inputs.clear();
        events
    }

    pub fn state(&self) -> ExtractorState {
        let earliest = self
            .open
            .iter()
            .map(|p| (p.start, &p.prompt_before))
            .chain(self.inputs.iter().map(|p| (p.start, &p.prompt_before)))
            .min_by_key(|(start, _)| *start);
        let mut state = self.state.clone();
        match earliest {
            Some((start, prompt)) => {
                state.safe_offset = start;
                state.current_prompt_id = prompt.clone();
            }
            None => state.safe_offset = self.last_end,
        }
        state
    }

    fn on_user(&mut self, v: &Value, start: u64, events: &mut Vec<Event>) {
        let prompt_before = self.state.current_prompt_id.clone();
        if let Some(pid) = str_field(v, "promptId") {
            self.state.current_prompt_id = Some(pid.to_string());
        }
        let content = v.pointer("/message/content");
        if let Some(blocks) = content.and_then(Value::as_array) {
            let results: Vec<&Value> = blocks
                .iter()
                .filter(|b| str_field(b, "type") == Some("tool_result"))
                .collect();
            if !results.is_empty() {
                self.attach_results(&results, v.get("toolUseResult"));
                return;
            }
        }
        let text = match content {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter(|b| str_field(b, "type") == Some("text"))
                .filter_map(|b| str_field(b, "text"))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => return,
        };
        let is_meta = v.get("isMeta").and_then(Value::as_bool) == Some(true);
        if (is_meta && !text.contains(AGENT_MESSAGE_TAG))
            || strip_system_reminders(&text).trim().is_empty()
        {
            return;
        }
        let input = if let Some(n) = text
            .contains(TASK_NOTIFICATION_TAG)
            .then(|| parse_task_notification(&text))
            .flatten()
        {
            events.push(Event::Notification {
                session_id: str_field(v, "sessionId").unwrap_or_default().to_string(),
                line_uuid: str_field(v, "uuid").unwrap_or_default().to_string(),
                ts: str_field(v, "timestamp").unwrap_or_default().to_string(),
                notification: n.clone(),
            });
            IterationInput::TaskNotification(n)
        } else if text.contains(AGENT_MESSAGE_TAG) {
            IterationInput::AgentMessage { text }
        } else {
            IterationInput::Prompt { text }
        };
        self.inputs.push(Pending {
            start,
            prompt_before,
            value: input,
        });
    }

    fn attach_results(&mut self, results: &[&Value], structured: Option<&Value>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        for r in results {
            let Some(id) = str_field(r, "tool_use_id") else {
                continue;
            };
            if let Some(call) = open.value.tool_calls.iter_mut().find(|c| c.id == id) {
                let handback = structured
                    .and_then(|s| s.pointer("/handbackReport/text"))
                    .and_then(Value::as_str);
                call.result = Some(ToolResultData {
                    text: handback.map_or_else(|| flatten_result(r.get("content")), str::to_string),
                    is_error: r.get("is_error").and_then(Value::as_bool) == Some(true),
                    structured: structured.filter(|s| s.is_object()).cloned(),
                });
            }
        }
    }

    fn on_assistant(&mut self, v: &Value, start: u64, events: &mut Vec<Event>) {
        let Some(mid) = v.pointer("/message/id").and_then(Value::as_str) else {
            return;
        };
        if self
            .open
            .as_ref()
            .is_some_and(|p| p.value.message_id != mid)
        {
            events.extend(self.close_open());
        }
        if self.open.is_none() {
            let inputs = std::mem::take(&mut self.inputs);
            let (start, prompt_before) = match inputs.first() {
                Some(first) => (first.start, first.prompt_before.clone()),
                None => (start, self.state.current_prompt_id.clone()),
            };
            self.open = Some(Pending {
                start,
                prompt_before,
                value: Iteration {
                    session_id: str_field(v, "sessionId").unwrap_or_default().to_string(),
                    prompt_id: self.state.current_prompt_id.clone(),
                    message_id: mid.to_string(),
                    iteration_index: 0,
                    cwd: str_field(v, "cwd").unwrap_or_default().to_string(),
                    ts: str_field(v, "timestamp").unwrap_or_default().to_string(),
                    inputs: inputs.into_iter().map(|p| p.value).collect(),
                    texts: Vec::new(),
                    tool_calls: Vec::new(),
                },
            });
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let api_error = v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true);
        let blocks = v.pointer("/message/content").and_then(Value::as_array);
        for b in blocks.into_iter().flatten() {
            match str_field(b, "type") {
                Some("text") if api_error => {}
                Some("text") => {
                    if let Some(t) = str_field(b, "text") {
                        open.value.texts.push(t.to_string());
                    }
                }
                Some("tool_use") => open.value.tool_calls.push(ToolCall {
                    id: str_field(b, "id").unwrap_or_default().to_string(),
                    name: str_field(b, "name").unwrap_or_default().to_string(),
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                    result: None,
                }),
                _ => {}
            }
        }
    }

    fn close_open(&mut self) -> Vec<Event> {
        let Some(pending) = self.open.take() else {
            return Vec::new();
        };
        let mut it = pending.value;
        if it.prompt_id != self.state.indexed_prompt_id {
            self.state.indexed_prompt_id = it.prompt_id.clone();
            self.state.next_index = 0;
        }
        it.iteration_index = self.state.next_index;
        self.state.next_index += 1;
        vec![Event::Iteration(it)]
    }
}

pub const TASK_NOTIFICATION_TAG: &str = "<task-notification>";
const AGENT_MESSAGE_TAG: &str = "<agent-message";
const SYSTEM_REMINDER_OPEN: &str = "<system-reminder>";
const SYSTEM_REMINDER_CLOSE: &str = "</system-reminder>";

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn strip_system_reminders(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(SYSTEM_REMINDER_OPEN) {
        out.push_str(&rest[..i]);
        match rest[i..].find(SYSTEM_REMINDER_CLOSE) {
            Some(j) => rest = &rest[i + j + SYSTEM_REMINDER_CLOSE.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn flatten_result(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| match str_field(b, "type") {
                Some("text") => str_field(b, "text").map(str::to_string),
                Some(other) => Some(format!("[{other}]")),
                None => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
