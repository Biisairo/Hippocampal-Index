mod common;

use common::*;
use hippo::bg::{
    BackgroundKind, BackgroundStart, background_start, extract_agent_report,
    parse_task_notification,
};
use hippo::transcript::{ToolCall, ToolResultData};
use serde_json::json;

fn call(name: &str, input: serde_json::Value, structured: Option<serde_json::Value>) -> ToolCall {
    ToolCall {
        id: "toolu_X".into(),
        name: name.into(),
        input,
        result: Some(ToolResultData {
            text: String::new(),
            is_error: false,
            structured,
        }),
    }
}

// ---- FR-BG-1 ----

#[test]
fn background_bash_start() {
    let c = call(
        "Bash",
        json!({"command": "sleep 5", "run_in_background": true}),
        Some(json!({"backgroundTaskId": "bm0rqrvzr"})),
    );
    assert_eq!(
        background_start(&c),
        Some(BackgroundStart {
            tool_use_id: "toolu_X".into(),
            kind: BackgroundKind::Bash,
            task_id: "bm0rqrvzr".into()
        })
    );
}

#[test]
fn background_agent_start() {
    let c = call(
        "Agent",
        json!({"prompt": "p", "run_in_background": true}),
        Some(json!({"isAsync": true, "status": "async_launched", "agentId": "a6537"})),
    );
    let b = background_start(&c).unwrap();
    assert_eq!(b.kind, BackgroundKind::Agent);
    assert_eq!(b.task_id, "a6537");
}

#[test]
fn foreground_calls_are_not_background() {
    assert_eq!(
        background_start(&call(
            "Agent",
            json!({"prompt": "p"}),
            Some(json!({"status": "completed", "agentId": "x"}))
        )),
        None
    );
    assert_eq!(
        background_start(&call(
            "Bash",
            json!({"command": "ls", "run_in_background": false}),
            None
        )),
        None
    );
    assert_eq!(
        background_start(&call(
            "Read",
            json!({"run_in_background": true}),
            Some(json!({"backgroundTaskId": "x"}))
        )),
        None
    );
}

#[test]
fn background_without_task_id_is_ignored() {
    let c = call(
        "Bash",
        json!({"run_in_background": true}),
        Some(json!({"stdout": ""})),
    );
    assert_eq!(background_start(&c), None);
    let mut no_result = c.clone();
    no_result.result = None;
    assert_eq!(background_start(&no_result), None);
}

// ---- FR-BG-2 ----

const NOTE: &str = "<task-notification>\n<task-id>bm0rqrvzr</task-id>\n<tool-use-id>toolu_01D9</tool-use-id>\n<output-file>/private/tmp/x/tasks/bm0rqrvzr.output</output-file>\n<status>completed</status>\n<summary>Background command \"Run\" completed (exit code 0)</summary>\n</task-notification>";

#[test]
fn parses_task_notification() {
    let n = parse_task_notification(NOTE).unwrap();
    assert_eq!(n.task_id, "bm0rqrvzr");
    assert_eq!(n.tool_use_id, "toolu_01D9");
    assert_eq!(
        n.output_file.as_deref(),
        Some("/private/tmp/x/tasks/bm0rqrvzr.output")
    );
    assert_eq!(n.status, "completed");
    assert!(n.summary.contains("exit code 0"));
}

#[test]
fn notification_without_output_file_is_allowed() {
    let text = NOTE.replace(
        "<output-file>/private/tmp/x/tasks/bm0rqrvzr.output</output-file>\n",
        "",
    );
    assert_eq!(parse_task_notification(&text).unwrap().output_file, None);
}

#[test]
fn notification_missing_required_fields_is_rejected() {
    assert_eq!(parse_task_notification("plain text"), None);
    assert_eq!(
        parse_task_notification(&NOTE.replace("<tool-use-id>toolu_01D9</tool-use-id>", "")),
        None
    );
    assert_eq!(
        parse_task_notification(&NOTE.replace("<task-id>bm0rqrvzr</task-id>", "")),
        None
    );
}

// ---- FR-BG-5 ----

#[test]
fn agent_report_from_handback_s4() {
    let content = read_fixture("s4_bg_agent.agent-a65370863753f6d78.jsonl");
    assert_eq!(extract_agent_report(content.lines()).as_deref(), Some("24"));
}

#[test]
fn agent_report_from_handback_s3() {
    let content = read_fixture("s3_fg_agent.agent-a9a4012efcf2e2824.jsonl");
    assert!(
        extract_agent_report(content.lines())
            .unwrap()
            .contains("log_hook.py")
    );
}

#[test]
fn agent_report_falls_back_to_last_text() {
    let content = joined(&[
        assistant_line("a1", "m1", json!([{"type": "text", "text": "first"}])),
        assistant_line(
            "a2",
            "m2",
            json!([{"type": "thinking", "thinking": ""}, {"type": "text", "text": "final"}]),
        ),
    ]);
    assert_eq!(
        extract_agent_report(content.lines()).as_deref(),
        Some("final")
    );
}

#[test]
fn agent_report_none_when_no_output() {
    let content = joined(&[user_line("u1", "p", json!("q")), "garbage".into()]);
    assert_eq!(extract_agent_report(content.lines()), None);
}
