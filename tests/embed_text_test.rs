use std::num::NonZeroUsize;

use serde_json::json;
use smem::bg::TaskNotification;
use smem::embed_text::{build_embed_text, chunk, chunk_limit};
use smem::transcript::{Iteration, IterationInput, ToolCall};

fn iteration(inputs: Vec<IterationInput>, calls: Vec<ToolCall>, texts: Vec<&str>) -> Iteration {
    Iteration {
        session_id: "s".into(),
        prompt_id: Some("p".into()),
        message_id: "m".into(),
        iteration_index: 0,
        cwd: "/w".into(),
        ts: "t".into(),
        inputs,
        texts: texts.into_iter().map(str::to_string).collect(),
        tool_calls: calls,
    }
}

fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: name.into(),
        input,
        result: None,
    }
}

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

// ---- FR-STO-2 ----

#[test]
fn full_iteration_format() {
    let it = iteration(
        vec![IterationInput::Prompt {
            text: "로그인 버그 고쳐줘".into(),
        }],
        vec![
            call("Read", json!({"file_path": "auth.ts"})),
            call("Grep", json!({"pattern": "token"})),
        ],
        vec!["확인하겠습니다", "두 번째 문단"],
    );
    assert_eq!(
        build_embed_text(&it, 500),
        "[입력] 로그인 버그 고쳐줘\n[툴] Read({\"file_path\":\"auth.ts\"})\n[툴] Grep({\"pattern\":\"token\"})\n[응답] 확인하겠습니다\n두 번째 문단"
    );
}

#[test]
fn empty_sections_are_omitted() {
    let it = iteration(vec![], vec![], vec!["답만 있음"]);
    assert_eq!(build_embed_text(&it, 500), "[응답] 답만 있음");
    let tools_only = iteration(vec![], vec![call("Bash", json!({"command": "ls"}))], vec![]);
    assert_eq!(
        build_embed_text(&tools_only, 500),
        "[툴] Bash({\"command\":\"ls\"})"
    );
    assert_eq!(
        build_embed_text(&iteration(vec![], vec![], vec![]), 500),
        ""
    );
}

#[test]
fn notification_and_agent_message_inputs() {
    let it = iteration(
        vec![
            IterationInput::TaskNotification(TaskNotification {
                task_id: "t".into(),
                tool_use_id: "u".into(),
                output_file: None,
                status: "completed".into(),
                summary: "npm test completed (exit code 0)".into(),
            }),
            IterationInput::AgentMessage {
                text: "보고: 24".into(),
            },
        ],
        vec![],
        vec!["24"],
    );
    assert_eq!(
        build_embed_text(&it, 500),
        "[입력] completed: npm test completed (exit code 0)\n[입력] 보고: 24\n[응답] 24"
    );
}

#[test]
fn long_tool_input_is_truncated_in_embed_text_only() {
    let content = "가".repeat(1000);
    let it = iteration(
        vec![],
        vec![call("Write", json!({"content": content}))],
        vec![],
    );
    let text = build_embed_text(&it, 20);
    let json_part = text
        .strip_prefix("[툴] Write(")
        .unwrap()
        .strip_suffix(')')
        .unwrap();
    assert_eq!(json_part.chars().count(), 21, "20자 + …");
    assert!(json_part.ends_with('…'));
    assert_eq!(
        it.tool_calls[0].input["content"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1000
    );
}

// ---- FR-STO-5 ----

#[test]
fn chunk_limit_uses_smaller_token_limit() {
    assert_eq!(chunk_limit(32_000, 8_000, 1.0).get(), 8_000);
    assert_eq!(chunk_limit(2_000, 8_000, 1.0).get(), 2_000);
    assert_eq!(chunk_limit(8_000, 8_000, 2.5).get(), 20_000);
}

#[test]
fn chunk_limit_never_zero() {
    assert_eq!(chunk_limit(0, 8_000, 1.0).get(), 1);
    assert_eq!(chunk_limit(8_000, 8_000, 0.0).get(), 1);
}

#[test]
fn short_text_is_single_chunk() {
    assert_eq!(chunk("짧은 글", nz(100)), vec!["짧은 글".to_string()]);
}

#[test]
fn empty_text_has_no_chunks() {
    assert!(chunk("", nz(10)).is_empty());
}

#[test]
fn splits_at_last_newline_within_limit() {
    let text = "aaaa\nbbbb\ncccc";
    assert_eq!(
        chunk(text, nz(10)),
        vec!["aaaa\nbbbb\n".to_string(), "cccc".to_string()]
    );
}

#[test]
fn hard_split_when_no_newline_and_multibyte_safe() {
    let text = "가나다라마바사";
    let chunks = chunk(text, nz(3));
    assert_eq!(chunks, vec!["가나다", "라마바", "사"]);
}

#[test]
fn chunks_are_lossless_and_within_limit() {
    let text = (0..500)
        .map(|i| format!("줄 {i} {}\n", "x".repeat(i % 37)))
        .collect::<String>();
    let limit = nz(64);
    let chunks = chunk(&text, limit);
    assert_eq!(chunks.concat(), text);
    assert!(
        chunks
            .iter()
            .all(|c| !c.is_empty() && c.chars().count() <= limit.get())
    );
}
