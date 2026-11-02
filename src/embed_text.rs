//! 임베딩 대상 텍스트 생성과 청크 분할. (FR-STO-2, FR-STO-5)

use std::num::NonZeroUsize;

use std::path::Path;

use serde_json::Value;

use crate::transcript::{Iteration, IterationInput, ToolCall};

const TRUNCATION_MARK: char = '…';
const BASH_TOOL: &str = "Bash";
const HIPPO_EXE: &str = "hippo";

/// 반복 입력의 표시 텍스트. 완료 알림은 `<status>: <summary>`.
pub fn input_text(input: &IterationInput) -> String {
    match input {
        IterationInput::Prompt { text } | IterationInput::AgentMessage { text } => text.clone(),
        IterationInput::TaskNotification(n) => format!("{}: {}", n.status, n.summary),
    }
}

/// `command`의 첫 단어(따옴표를 벗긴 값)의 파일 이름이 `hippo`인 Bash 호출. (FR-STO-11)
pub fn is_hippo_call(call: &ToolCall) -> bool {
    if call.name != BASH_TOOL {
        return false;
    }
    let Some(command) = call.input.get("command").and_then(Value::as_str) else {
        return false;
    };
    let command = command.trim_start();
    let first = match command.chars().next() {
        Some(q @ ('\'' | '"')) => command[1..].split(q).next().unwrap_or(""),
        _ => command.split_whitespace().next().unwrap_or(""),
    };
    Path::new(first).file_name().is_some_and(|n| n == HIPPO_EXE)
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
        .map(|input| format!("[input] {}", input_text(input)));
    let tools = it
        .tool_calls
        .iter()
        .filter(|call| !is_hippo_call(call))
        .flat_map(|call| {
            let line = format!(
                "[tool] {}({})",
                call.name,
                truncate(&call.input.to_string(), max_tool_input_chars)
            );
            let answer = call
                .result
                .as_ref()
                .filter(|_| answer_tools.contains(&call.name))
                .map(|r| format!("[answer] {}", r.text));
            std::iter::once(line).chain(answer)
        });
    let response = (!it.texts.is_empty()).then(|| format!("[response] {}", it.texts.join("\n")));
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

pub fn chunk(text: &str, max_chars: NonZeroUsize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some((limit, _)) = rest.char_indices().nth(max_chars.get()) else {
            chunks.push(rest.to_string());
            break;
        };
        let cut = rest[..limit].rfind('\n').map_or(limit, |i| i + 1);
        chunks.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    chunks
}
