mod common;

use std::collections::BTreeSet;

use common::*;
use hippo::transcript::{Event, Extractor, ExtractorState, IterationInput};
use serde_json::json;

// ---- 실측 fixture 기반 ----

#[test]
fn s1_text_only_iteration_completes_on_finish_turn() {
    let (mut ex, events) = run_fixture("s1_notool.jsonl");
    assert!(
        iterations(&events).is_empty(),
        "다음 라인이 없으므로 아직 완료가 아님"
    );

    let its = iterations(&ex.finish_turn());
    assert_eq!(its.len(), 1);
    let it = &its[0];
    assert_eq!(it.message_id, "msg_011CgdDVj6iVzqktydy4MNuE");
    assert_eq!(
        it.prompt_id.as_deref(),
        Some("2ca92f21-3f9e-4079-ba71-69e7ef08d365")
    );
    assert_eq!(it.iteration_index, 0);
    assert_eq!(
        it.inputs,
        vec![IterationInput::Prompt {
            text: "1+1은? 도구를 쓰지 말고 숫자만 답해.".into()
        }],
        "isMeta 안내 메시지는 입력에서 제외"
    );
    assert_eq!(it.texts, vec!["2".to_string()], "thinking은 제외");
    assert!(it.tool_calls.is_empty());
    assert_eq!(it.cwd, fixture_cwd("s1_notool.jsonl"));
    assert!(!it.cwd.is_empty());
    assert!(!it.ts.is_empty());
}

#[test]
fn s2_interleaved_parallel_calls_stay_in_one_iteration() {
    let (mut ex, events) = run_fixture("s2_tools.jsonl");
    let its = iterations(&events);
    assert_eq!(its.len(), 1, "다른 message.id가 나타날 때 첫 반복이 완료");
    let first = &its[0];
    assert!(first.message_id.ends_with("HkUAv6"));
    assert_eq!(first.tool_calls.len(), 2);
    assert!(
        first
            .tool_calls
            .iter()
            .all(|c| c.name == "Read" && c.result.is_some())
    );
    assert!(
        first.tool_calls[0]
            .result
            .as_ref()
            .unwrap()
            .text
            .contains("import json")
    );
    assert!(matches!(
        first.inputs.as_slice(),
        [IterationInput::Prompt { .. }]
    ));

    let last = iterations(&ex.finish_turn());
    assert_eq!(last.len(), 1);
    assert!(last[0].message_id.ends_with("TKNZ8A"));
    assert_eq!(last[0].iteration_index, 1);
    assert!(last[0].inputs.is_empty());
    assert!(last[0].texts[0].starts_with("두 파일은"));
}

#[test]
fn s2_finish_batch_completes_iteration_when_all_results_present() {
    let content = read_fixture("s2_tools.jsonl");
    let lines = lines_with_offsets(&content);
    let mut ex = Extractor::new(ExtractorState::default());
    for (l, s, e) in &lines[..=6] {
        assert!(iterations(&ex.feed_line(l, *s, *e)).is_empty());
    }
    let its = iterations(&ex.finish_batch());
    assert_eq!(its.len(), 1);
    assert_eq!(its[0].tool_calls.len(), 2);
}

#[test]
fn finish_batch_waits_when_results_missing() {
    let content = read_fixture("s2_tools.jsonl");
    let lines = lines_with_offsets(&content);
    let mut ex = Extractor::new(ExtractorState::default());
    for (l, s, e) in &lines[..=5] {
        ex.feed_line(l, *s, *e);
    }
    assert!(
        iterations(&ex.finish_batch()).is_empty(),
        "두 번째 Read의 결과가 아직 없음"
    );
}

#[test]
fn s3_foreground_agent_result_is_stored_as_tool_result() {
    let (mut ex, events) = run_fixture("s3_fg_agent.jsonl");
    let mut its = iterations(&events);
    its.extend(iterations(&ex.finish_turn()));
    assert_eq!(its.len(), 2);
    let call = &its[0].tool_calls[0];
    assert_eq!(call.name, "Agent");
    let result = call.result.as_ref().unwrap();
    assert!(result.text.contains("log_hook.py"));
    assert_eq!(result.structured.as_ref().unwrap()["status"], "completed");
}

#[test]
fn s4_background_agent_message_and_notification() {
    let (mut ex, events) = run_fixture("s4_bg_agent.jsonl");
    let notifications: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Notification {
                line_uuid,
                notification,
                session_id,
                ts,
            } => {
                assert!(ts.starts_with("2026-"), "알림 라인의 timestamp");
                Some((line_uuid.clone(), notification.clone(), session_id.clone()))
            }
            Event::Iteration(_) => None,
        })
        .collect();
    assert_eq!(
        notifications.len(),
        1,
        "알림은 반복 완료를 기다리지 않고 즉시 내보냄"
    );
    assert_eq!(notifications[0].1.task_id, "a65370863753f6d78");
    assert_eq!(
        notifications[0].1.tool_use_id,
        "toolu_011MdRAe8c4q2Ua6xuusj2hf"
    );
    assert!(!notifications[0].0.is_empty());
    assert_eq!(notifications[0].2, "40c046b1-c6a7-4e0c-8d41-f78b1f948059");

    let mut its = iterations(&events);
    its.extend(iterations(&ex.finish_turn()));
    assert_eq!(its.len(), 3);
    let report = &its[2];
    assert!(report.prompt_id.as_deref().unwrap().starts_with("5fbbafee"));
    assert_eq!(report.iteration_index, 0, "새 prompt_id에서 순번 초기화");
    assert!(
        matches!(report.inputs.as_slice(), [IterationInput::AgentMessage { text }] if text.contains("24")),
        "isMeta여도 agent-message는 입력으로 유지, system-reminder 전용 라인은 제외"
    );
    assert_eq!(report.texts, vec!["24".to_string()]);
}

#[test]
fn s4_finish_session_drops_unattached_inputs_and_advances_offset() {
    let content = read_fixture("s4_bg_agent.jsonl");
    let last_line_start = lines_with_offsets(&content).last().unwrap().1;
    let (mut ex, _) = run_fixture("s4_bg_agent.jsonl");
    ex.finish_turn();
    assert_eq!(
        ex.state().safe_offset,
        last_line_start,
        "붙지 않은 알림 라인 앞에서 대기"
    );
    assert!(iterations(&ex.finish_session()).is_empty());
    assert_eq!(ex.state().safe_offset, content.len() as u64);
}

#[test]
fn s5_notification_becomes_input_of_next_iteration() {
    let (mut ex, events) = run_fixture("s5_bg_bash.jsonl");
    let mut its = iterations(&events);
    its.extend(iterations(&ex.finish_turn()));
    assert_eq!(its.len(), 4);
    let bash = &its[0].tool_calls[0];
    assert_eq!(bash.input["run_in_background"], true);
    assert_eq!(
        bash.result.as_ref().unwrap().structured.as_ref().unwrap()["backgroundTaskId"],
        "bm0rqrvzr"
    );

    let read = &its[2];
    assert!(read.prompt_id.as_deref().unwrap().starts_with("d5e737c4"));
    assert!(matches!(
        read.inputs.as_slice(),
        [IterationInput::TaskNotification(n)] if n.task_id == "bm0rqrvzr" && n.status == "completed"
    ));
}

// ---- 안전 offset과 재개 (FR-EXT-1, FR-EXT-10) ----

#[test]
fn safe_offset_points_to_earliest_pending_line() {
    let content = read_fixture("s2_tools.jsonl");
    let lines = lines_with_offsets(&content);
    let mut ex = Extractor::new(ExtractorState::default());
    for (l, s, e) in &lines[..=5] {
        ex.feed_line(l, *s, *e);
    }
    assert_eq!(
        ex.state().safe_offset,
        lines[1].1,
        "프롬프트 라인부터 아직 반복에 확정되지 않음"
    );
}

#[test]
fn resume_from_any_cut_point_yields_same_iterations() {
    for name in [
        "s1_notool.jsonl",
        "s2_tools.jsonl",
        "s3_fg_agent.jsonl",
        "s4_bg_agent.jsonl",
        "s5_bg_bash.jsonl",
    ] {
        let content = read_fixture(name);
        let lines = lines_with_offsets(&content);

        let (mut full, full_events) = run_fixture(name);
        let mut expected = iterations(&full_events);
        expected.extend(iterations(&full.finish_turn()));

        for cut in 0..lines.len() {
            let mut a = Extractor::new(ExtractorState::default());
            let mut got = Vec::new();
            for (l, s, e) in &lines[..cut] {
                got.extend(iterations(&a.feed_line(l, *s, *e)));
            }
            let saved = serde_json::to_string(&a.state()).unwrap();
            let state: ExtractorState = serde_json::from_str(&saved).unwrap();
            let mut b = Extractor::new(state.clone());
            for (l, s, e) in lines.iter().filter(|(_, s, _)| *s >= state.safe_offset) {
                got.extend(iterations(&b.feed_line(l, *s, *e)));
            }
            got.extend(iterations(&b.finish_turn()));
            assert_eq!(got, expected, "{name} cut={cut}");
        }
    }
}

// ---- 합성 라인 기반 경계 사례 ----

#[test]
fn ignores_malformed_and_non_conversation_lines() {
    let content = joined(&[
        "not json".into(),
        r#"{"type":"queue-operation","operation":"enqueue"}"#.into(),
        r#"{"type":"system","subtype":"stop_hook_summary"}"#.into(),
        user_line("u1", "p1", json!("hello")),
        assistant_line("a1", "m1", json!([{"type": "text", "text": "hi"}])),
    ]);
    let mut ex = Extractor::new(ExtractorState::default());
    feed_all(&mut ex, &content);
    let its = iterations(&ex.finish_turn());
    assert_eq!(its.len(), 1);
    assert_eq!(
        its[0].inputs,
        vec![IterationInput::Prompt {
            text: "hello".into()
        }]
    );
}

#[test]
fn excludes_system_reminder_only_and_empty_user_lines() {
    let content = joined(&[
        user_line(
            "u1",
            "p1",
            json!("<system-reminder>\nnoise\n</system-reminder>"),
        ),
        user_line("u2", "p1", json!("   ")),
        user_line(
            "u3",
            "p1",
            json!([{"type": "text", "text": "블록 프롬프트"}, {"type": "image"}]),
        ),
        assistant_line("a1", "m1", json!([{"type": "text", "text": "ok"}])),
    ]);
    let mut ex = Extractor::new(ExtractorState::default());
    feed_all(&mut ex, &content);
    let its = iterations(&ex.finish_turn());
    assert_eq!(
        its[0].inputs,
        vec![IterationInput::Prompt {
            text: "블록 프롬프트".into()
        }]
    );
}

#[test]
fn text_only_iteration_completes_when_new_message_follows() {
    let content = joined(&[
        user_line("u1", "p1", json!("q")),
        assistant_line("a1", "m1", json!([{"type": "text", "text": "first"}])),
        assistant_line("a2", "m2", json!([{"type": "text", "text": "continued"}])),
    ]);
    let mut ex = Extractor::new(ExtractorState::default());
    let its = iterations(&feed_all(&mut ex, &content));
    assert_eq!(its.len(), 1);
    assert_eq!(its[0].texts, vec!["first".to_string()]);
    let rest = iterations(&ex.finish_turn());
    assert_eq!(rest[0].iteration_index, 1);
    assert!(rest[0].inputs.is_empty());
}

#[test]
fn missing_tool_result_is_stored_as_none_on_finish_turn() {
    let content = joined(&[
        user_line("u1", "p1", json!("q")),
        assistant_line(
            "a1",
            "m1",
            json!([{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "x"}}]),
        ),
    ]);
    let mut ex = Extractor::new(ExtractorState::default());
    feed_all(&mut ex, &content);
    let its = iterations(&ex.finish_turn());
    assert_eq!(its.len(), 1);
    assert!(its[0].tool_calls[0].result.is_none());
}

#[test]
fn tool_result_blocks_are_flattened_and_error_flag_kept() {
    let content = joined(&[
        user_line("u1", "p1", json!("q")),
        assistant_line(
            "a1",
            "m1",
            json!([{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]),
        ),
        user_line(
            "u2",
            "p1",
            json!([{"type": "tool_result", "tool_use_id": "t1", "is_error": true,
            "content": [{"type": "text", "text": "line1"}, {"type": "text", "text": "line2"}]}]),
        ),
    ]);
    let mut ex = Extractor::new(ExtractorState::default());
    feed_all(&mut ex, &content);
    let its = iterations(&ex.finish_batch());
    let r = its[0].tool_calls[0].result.as_ref().unwrap();
    assert_eq!(r.text, "line1\nline2");
    assert!(r.is_error);
}

#[test]
fn replayed_notification_has_same_line_uuid() {
    let (_, first) = run_fixture("s5_bg_bash.jsonl");
    let (_, second) = run_fixture("s5_bg_bash.jsonl");
    let uuids = |ev: &[Event]| -> BTreeSet<String> {
        ev.iter()
            .filter_map(|e| match e {
                Event::Notification { line_uuid, .. } => Some(line_uuid.clone()),
                Event::Iteration(_) => None,
            })
            .collect()
    };
    assert_eq!(uuids(&first), uuids(&second));
    assert_eq!(uuids(&first).len(), 1);
}

// ---- FR-EXT-3: API 오류 안내문은 응답이 아니다 ----

#[test]
fn api_error_message_text_is_not_stored_as_response() {
    let mut error_line: serde_json::Value = serde_json::from_str(&assistant_line(
        "a1",
        "m1",
        json!([{"type": "text", "text": "API Error: safeguards flagged this message"}]),
    ))
    .unwrap();
    error_line["isApiErrorMessage"] = json!(true);
    error_line["message"]["model"] = json!("<synthetic>");
    let content = joined(&[user_line("u1", "p1", json!("질문")), error_line.to_string()]);
    let mut ex = Extractor::new(ExtractorState::default());
    feed_all(&mut ex, &content);
    let its = iterations(&ex.finish_turn());
    assert_eq!(its.len(), 1, "반복 자체는 만든다");
    assert!(its[0].texts.is_empty(), "{:?}", its[0].texts);
    assert_eq!(
        its[0].inputs,
        vec![IterationInput::Prompt {
            text: "질문".into()
        }]
    );
}

// ---- 턴 종료 표시 라인 (FR-EXT-7 d, FR-EXT-9) ----

fn system_line(subtype: &str) -> String {
    json!({"type": "system", "subtype": subtype, "sessionId": "S", "timestamp": "2026-11-02T00:00:02Z"})
        .to_string()
}

fn text_turn(marker: &str) -> String {
    joined(&[
        user_line("u1", "P1", json!("질문")),
        assistant_line("a1", "M1", json!([{"type": "text", "text": "답"}])),
        system_line(marker),
    ])
}

#[test]
fn turn_end_markers_complete_the_open_iteration() {
    for marker in ["turn_duration", "stop_hook_summary"] {
        let mut ex = Extractor::new(ExtractorState::default());
        let its = iterations(&feed_all(&mut ex, &text_turn(marker)));
        assert_eq!(its.len(), 1, "{marker}");
        assert_eq!(its[0].texts, vec!["답".to_string()]);
        assert_eq!(its[0].prompt_id.as_deref(), Some("P1"));
        assert!(iterations(&ex.finish_turn()).is_empty(), "이미 확정됨");
    }
}

#[test]
fn other_system_lines_do_not_complete_the_iteration() {
    let mut ex = Extractor::new(ExtractorState::default());
    let its = iterations(&feed_all(&mut ex, &text_turn("away_summary")));
    assert!(its.is_empty());
    assert_eq!(iterations(&ex.finish_turn()).len(), 1);
}

#[test]
fn turn_end_marker_keeps_unattached_inputs() {
    let content = joined(&[
        user_line("u1", "P1", json!("첫 질문")),
        assistant_line("a1", "M1", json!([{"type": "text", "text": "첫 답"}])),
        user_line("u2", "P2", json!("둘째 질문")),
        system_line("turn_duration"),
        assistant_line("a2", "M2", json!([{"type": "text", "text": "둘째 답"}])),
        system_line("turn_duration"),
    ]);
    let mut ex = Extractor::new(ExtractorState::default());
    let its = iterations(&feed_all(&mut ex, &content));
    assert_eq!(its.len(), 2);
    assert_eq!(
        its[1].inputs,
        vec![IterationInput::Prompt {
            text: "둘째 질문".into()
        }]
    );
}
