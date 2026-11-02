//! 임베딩 대상 텍스트 생성과 청크 분할. (FR-STO-2, FR-STO-5)

use std::num::NonZeroUsize;

use crate::transcript::{Iteration, IterationInput};

const TRUNCATION_MARK: char = '…';

pub fn build_embed_text(it: &Iteration, max_tool_input_chars: usize) -> String {
    let inputs = it.inputs.iter().map(|input| {
        let text = match input {
            IterationInput::Prompt { text } | IterationInput::AgentMessage { text } => text.clone(),
            IterationInput::TaskNotification(n) => format!("{}: {}", n.status, n.summary),
        };
        format!("[입력] {text}")
    });
    let tools = it.tool_calls.iter().map(|call| {
        let json = call.input.to_string();
        let shown = if json.chars().count() > max_tool_input_chars {
            let mut cut: String = json.chars().take(max_tool_input_chars).collect();
            cut.push(TRUNCATION_MARK);
            cut
        } else {
            json
        };
        format!("[툴] {}({shown})", call.name)
    });
    let response = (!it.texts.is_empty()).then(|| format!("[응답] {}", it.texts.join("\n")));
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
