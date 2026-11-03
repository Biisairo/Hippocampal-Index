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
    assert_eq!(chunk("짧은 글", nz(100), 20), vec!["짧은 글".to_string()]);
    let exact = "가".repeat(10);
    assert_eq!(
        chunk(&exact, nz(10), 2),
        vec![exact.clone()],
        "길이가 L과 같으면 1청크"
    );
}

#[test]
fn empty_text_has_no_chunks() {
    assert!(chunk("", nz(10), 2).is_empty());
}

#[test]
fn neighbors_share_overlap_and_windows_advance_by_l_minus_o() {
    let text: String = "0123456789abcdefghij".into();
    let chunks = chunk(&text, nz(10), 2);
    assert_eq!(chunks, vec!["0123456789", "89abcdefgh", "ghij"]);
}

#[test]
fn multibyte_safe_with_overlap() {
    let chunks = chunk("가나다라마바사", nz(3), 1);
    assert_eq!(chunks, vec!["가나다", "다라마", "마바사"]);
}

#[test]
fn removing_overlap_restores_original_and_respects_limit() {
    let text = (0..500)
        .map(|i| format!("줄 {i} {}\n", "x".repeat(i % 37)))
        .collect::<String>();
    let (limit, overlap) = (nz(64), 16);
    let chunks = chunk(&text, limit, overlap);
    assert!(chunks.len() > 1);
    assert!(
        chunks
            .iter()
            .all(|c| !c.is_empty() && c.chars().count() <= limit.get())
    );
    let mut restored = chunks[0].clone();
    for pair in chunks.windows(2) {
        let prev_tail: String = pair[0].chars().skip(limit.get() - overlap).collect();
        let head: String = pair[1].chars().take(overlap).collect();
        assert_eq!(prev_tail, head, "이웃 청크는 O자를 공유");
        restored.extend(pair[1].chars().skip(overlap));
    }
    assert_eq!(restored, text);
}

#[test]
fn last_chunk_ends_at_text_end_without_tiny_duplicate() {
    let chunks = chunk(&"a".repeat(18), nz(10), 2);
    assert_eq!(chunks.len(), 2, "8자 진행: [0,10) [8,18)");
    assert_eq!(chunks[1].len(), 10);
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
    assert!(is_hippo_call(&bash("cd /x && hippo status")));
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

fn structured_answer(input: serde_json::Value, structured: serde_json::Value) -> ToolCall {
    ToolCall {
        result: Some(ToolResultData {
            text: "The user answered: \"long question\"=\"skip\". You can now continue.".into(),
            is_error: false,
            structured: Some(structured),
        }),
        ..call("AskUserQuestion", input)
    }
}

#[test]
fn answer_uses_structured_answers_in_question_order_with_notes() {
    let it = iteration(
        vec![],
        vec![structured_answer(
            json!({"q": 2}),
            json!({
                "questions": [{"question": "B?"}, {"question": "A?"}],
                "answers": {"A?": "yes", "B?": "skip copies"},
                "annotations": {"A?": {"notes": "only for import"}}
            }),
        )],
        vec![],
    );
    assert_eq!(
        build_embed_text(&it, 500, &answer_tools()),
        "[tool] AskUserQuestion({\"q\":2})\n[answer] skip copies / yes (note: only for import)"
    );
    let no_answers = iteration(
        vec![],
        vec![structured_answer(json!({}), json!({"plan": "x"}))],
        vec![],
    );
    assert!(
        build_embed_text(&no_answers, 500, &answer_tools()).ends_with("You can now continue."),
        "answers가 없으면 결과 텍스트"
    );
}

#[test]
fn hippo_calls_are_found_by_parsing_the_shell_command() {
    let bash = |cmd: &str| call("Bash", json!({"command": cmd}));
    for hit in [
        "cd /x && /p/hippo search q",
        "cargo build; hippo status",
        "echo q | /p/hippo search --kind result",
        "H=/p/target/release/hippo; $H show p1",
        "H=/p/hippo\n\"$H\" import --wait",
        "export H=/p/hippo && ${H} status",
        "out=$(/p/hippo search fork); echo \"$out\"",
        "echo `hippo status`",
        "env RUST_LOG=1 hippo status",
        "sudo -n /p/hippo status",
        "X=1 Y=2 /p/hippo search q",
        "(cd /x && hippo search q)",
        "for i in 1 2; do hippo status; done",
    ] {
        assert!(is_hippo_call(&bash(hit)), "잡아야 함: {hit}");
    }
    for miss in [
        "cargo test -p hippo",
        "ls target/release/hippo",
        "grep -rn hippo src",
        "H=/p/hippo; echo $H",
        "git commit -m 'hippo search fix'",
        "echo \"hippo status\"",
    ] {
        assert!(!is_hippo_call(&bash(miss)), "잡지 않아야 함: {miss}");
    }
    assert!(
        is_hippo_call(&bash("hippo search 'unterminated")),
        "파싱 실패 시 첫 단어 규칙"
    );
}
