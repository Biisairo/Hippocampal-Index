use std::num::NonZeroUsize;

use hippo::bg::TaskNotification;
use hippo::embed_text::{build_embed_text, chunk, chunk_limit};
use hippo::transcript::{Iteration, IterationInput, ToolCall};
use serde_json::json;

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
        build_embed_text(&it, 500, &[]),
        "[input] 로그인 버그 고쳐줘\n[tool] Read({\"file_path\":\"auth.ts\"})\n[tool] Grep({\"pattern\":\"token\"})\n[response] 확인하겠습니다\n두 번째 문단"
    );
}

#[test]
fn empty_sections_are_omitted() {
    let it = iteration(vec![], vec![], vec!["답만 있음"]);
    assert_eq!(build_embed_text(&it, 500, &[]), "[response] 답만 있음");
    let tools_only = iteration(vec![], vec![call("Bash", json!({"command": "ls"}))], vec![]);
    assert_eq!(
        build_embed_text(&tools_only, 500, &[]),
        "[tool] Bash({\"command\":\"ls\"})"
    );
    assert_eq!(
        build_embed_text(&iteration(vec![], vec![], vec![]), 500, &[]),
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
        build_embed_text(&it, 500, &[]),
        "[input] completed: npm test completed (exit code 0)\n[input] 보고: 24\n[response] 24"
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
    let text = build_embed_text(&it, 20, &[]);
    let json_part = text
        .strip_prefix("[tool] Write(")
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

// ---- FR-STO-2 [answer], FR-STO-11 ----

use hippo::embed_text::is_hippo_call;
use hippo::transcript::ToolResultData;

fn answered(name: &str, input: serde_json::Value, result: &str) -> ToolCall {
    ToolCall {
        result: Some(ToolResultData {
            text: result.into(),
            is_error: false,
            structured: None,
        }),
        ..call(name, input)
    }
}

fn answer_tools() -> Vec<String> {
    vec!["AskUserQuestion".into(), "ExitPlanMode".into()]
}

#[test]
fn user_answer_follows_its_tool_line() {
    let it = iteration(
        vec![],
        vec![
            answered(
                "AskUserQuestion",
                json!({"q": "fork?"}),
                "User chose: skip copies",
            ),
            answered("Read", json!({"file_path": "a.rs"}), "file body"),
        ],
        vec![],
    );
    assert_eq!(
        build_embed_text(&it, 500, &answer_tools()),
        "[tool] AskUserQuestion({\"q\":\"fork?\"})\n[answer] User chose: skip copies\n[tool] Read({\"file_path\":\"a.rs\"})",
        "응답 도구만 결과를 넣음"
    );
    let long = iteration(
        vec![],
        vec![answered("AskUserQuestion", json!({}), "0123456789abc")],
        vec![],
    );
    assert_eq!(
        build_embed_text(&long, 10, &answer_tools()),
        "[tool] AskUserQuestion({})\n[answer] 0123456789abc",
        "답은 결과 끝에 있으므로 자르지 않음"
    );
    let unanswered = iteration(vec![], vec![call("AskUserQuestion", json!({}))], vec![]);
    assert_eq!(
        build_embed_text(&unanswered, 500, &answer_tools()),
        "[tool] AskUserQuestion({})"
    );
}

#[test]
fn hippo_calls_are_recognized_by_first_word() {
    let bash = |cmd: &str| call("Bash", json!({"command": cmd}));
    assert!(is_hippo_call(&bash("/opt/bin/hippo search \"fork\"")));
    assert!(is_hippo_call(&bash("hippo status")));
    assert!(is_hippo_call(&bash("  '/a b/hippo' show p1")));
    assert!(is_hippo_call(&bash("\"/x/hippo\" import --wait")));
    assert!(!is_hippo_call(&bash("cd /x && hippo status")));
    assert!(!is_hippo_call(&bash("hippocampus run")));
    assert!(!is_hippo_call(&call("Read", json!({"file_path": "hippo"}))));
    assert!(!is_hippo_call(&call("Bash", json!({}))));
}

#[test]
fn hippo_calls_are_left_out_of_embed_text() {
    let hippo = call("Bash", json!({"command": "/opt/bin/hippo search fork"}));
    let only = iteration(vec![], vec![hippo.clone()], vec![]);
    assert_eq!(build_embed_text(&only, 500, &[]), "", "색인할 내용 없음");
    let with_prompt = iteration(
        vec![IterationInput::Prompt {
            text: "fork 중복 어떻게 했지".into(),
        }],
        vec![hippo, call("Read", json!({"file_path": "a.rs"}))],
        vec![],
    );
    assert_eq!(
        build_embed_text(&with_prompt, 500, &[]),
        "[input] fork 중복 어떻게 했지\n[tool] Read({\"file_path\":\"a.rs\"})",
        "사용자 프롬프트는 남김"
    );
}
