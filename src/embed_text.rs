//! 임베딩 대상 텍스트 생성과 청크 분할. (FR-STO-2, FR-STO-5)

use std::num::NonZeroUsize;

use serde_json::Value;

use crate::shell::runs_hippo;
use crate::transcript::{Iteration, IterationInput, ToolCall, ToolResultData};

const TRUNCATION_MARK: char = '…';
const BASH_TOOL: &str = "Bash";
const ANSWER_SEPARATOR: &str = " / ";
pub const INPUT_PREFIX: &str = "[input] ";
pub const TOOL_PREFIX: &str = "[tool] ";
pub const ANSWER_PREFIX: &str = "[answer] ";
pub const RESPONSE_PREFIX: &str = "[response] ";
/// 자동 주입에서만 쓴다. (FR-RET-4)
pub const QUESTION_PREFIX: &str = "[question] ";

/// 반복 입력의 표시 텍스트. 완료 알림은 `<status>: <summary>`.
pub fn input_text(input: &IterationInput) -> String {
    match input {
        IterationInput::Prompt { text } | IterationInput::AgentMessage { text } => text.clone(),
        IterationInput::TaskNotification(n) => format!("{}: {}", n.status, n.summary),
    }
}

/// 실행되는 명령 중 하나라도 hippo인 Bash 호출. (FR-STO-11)
pub fn is_hippo_call(call: &ToolCall) -> bool {
    is_hippo_tool(&call.name, &call.input)
}

pub fn is_hippo_tool(name: &str, input: &Value) -> bool {
    name == BASH_TOOL
        && input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(runs_hippo)
}

/// 사용자 응답 도구의 답. 구조화 결과의 `answers`가 있으면 질문 순서대로 답(과 메모)만 쓴다. (FR-STO-2)
fn answer_text(result: &ToolResultData) -> String {
    let Some(structured) = &result.structured else {
        return result.text.clone();
    };
    let Some(answers) = structured.get("answers").and_then(Value::as_object) else {
        return result.text.clone();
    };
    let ordered: Vec<&String> = structured
        .get("questions")
        .and_then(Value::as_array)
        .map(|qs| {
            qs.iter()
                .filter_map(|q| q.get("question").and_then(Value::as_str))
                .filter_map(|q| answers.get_key_value(q).map(|(k, _)| k))
                .collect()
        })
        .filter(|v: &Vec<&String>| !v.is_empty())
        .unwrap_or_else(|| answers.keys().collect());
    ordered
        .into_iter()
        .filter_map(|question| {
            let answer = answers.get(question)?.as_str()?;
            let note = structured
                .get("annotations")
                .and_then(|a| a.get(question))
                .and_then(|a| a.get("notes"))
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty());
            Some(match note {
                Some(note) => format!("{answer} (note: {note})"),
                None => answer.to_string(),
            })
        })
        .collect::<Vec<_>>()
        .join(ANSWER_SEPARATOR)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    cut.push(TRUNCATION_MARK);
    cut
}

/// 반복의 임베딩 텍스트. hippo 호출은 넣지 않고, 사용자 응답 도구는 결과를 `[answer]`로 넣는다. (FR-STO-2, FR-STO-11)
pub fn build_embed_text(
    it: &Iteration,
    max_tool_input_chars: usize,
    answer_tools: &[String],
) -> String {
    let inputs = it
        .inputs
        .iter()
        .map(|input| format!("{INPUT_PREFIX}{}", input_text(input)));
    let tools = it
        .tool_calls
        .iter()
        .filter(|call| !is_hippo_call(call))
        .flat_map(|call| {
            let line = format!(
                "{TOOL_PREFIX}{}({})",
                call.name,
                truncate(&call.input.to_string(), max_tool_input_chars)
            );
            let answer = call
                .result
                .as_ref()
                .filter(|_| answer_tools.contains(&call.name))
                .map(|r| format!("{ANSWER_PREFIX}{}", answer_text(r)));
            std::iter::once(line).chain(answer)
        });
    let response =
        (!it.texts.is_empty()).then(|| format!("{RESPONSE_PREFIX}{}", it.texts.join("\n")));
    inputs
        .chain(tools)
        .chain(response)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn chunk_limit(
    model_max_tokens: usize,
    config_max_tokens: usize,
    chars_per_token: f64,
) -> NonZeroUsize {
    let tokens = model_max_tokens.min(config_max_tokens);
    let chars = (tokens as f64 * chars_per_token).floor() as usize;
    NonZeroUsize::new(chars).unwrap_or(NonZeroUsize::MIN)
}

/// `max_chars`자 창을 `max_chars - overlap`자씩 옮기며 나눈다. 이웃 청크는 `overlap`자를 공유한다. (FR-STO-5)
pub fn chunk(text: &str, max_chars: NonZeroUsize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let size = max_chars.get();
    let step = size - overlap.min(size - 1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + size).min(chars.len());
        chunks.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
    }
    chunks
}
