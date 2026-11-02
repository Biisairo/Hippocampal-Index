mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use common::*;
use serde_json::{Value, json};
use smem::ingest::{HookEvent, IngestError, IngestOptions, IngestRequest, ingest};
use smem::redact::Redactor;
use smem::store::RecordStore;

const OPTS: IngestOptions = IngestOptions {
    max_tool_input_chars: 500,
};

fn req(event: HookEvent, session: &str, path: &Path) -> IngestRequest {
    IngestRequest {
        event,
        session_id: session.into(),
        transcript_path: path.to_path_buf(),
    }
}

fn run(
    store: &mut RecordStore,
    event: HookEvent,
    session: &str,
    path: &Path,
) -> smem::ingest::IngestReport {
    ingest(
        store,
        &Redactor::new(&[]).unwrap(),
        OPTS,
        &req(event, session, path),
    )
    .unwrap()
}

fn copy_fixture(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, read_fixture(name)).unwrap();
    path
}

const S2: &str = "afd070e6-4625-4b31-8c43-f1f553949222";

#[test]
fn ingests_whole_turn_on_stop() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_fixture(dir.path(), "s2_tools.jsonl");
    let mut store = RecordStore::open_in_memory().unwrap();
    let report = run(&mut store, HookEvent::Stop, S2, &path);
    assert_eq!(report.iterations, 2);
    assert_eq!(store.counts().unwrap().iterations, 2);
    assert_eq!(
        store.session_state(S2).unwrap().unwrap().safe_offset,
        fs::metadata(&path).unwrap().len()
    );
}

#[test]
fn incremental_ingest_has_no_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let content = read_fixture("s2_tools.jsonl");
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let path = dir.path().join("t.jsonl");
    fs::write(&path, lines[..7].concat()).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    assert_eq!(
        run(&mut store, HookEvent::PostToolBatch, S2, &path).iterations,
        1
    );
    fs::write(&path, content.clone()).unwrap();
    assert_eq!(run(&mut store, HookEvent::Stop, S2, &path).iterations, 1);
    assert_eq!(
        run(&mut store, HookEvent::Stop, S2, &path).iterations,
        0,
        "같은 요청 반복은 변화 없음"
    );
    assert_eq!(store.counts().unwrap().iterations, 2);
}

#[test]
fn unterminated_last_line_is_not_consumed() {
    let dir = tempfile::tempdir().unwrap();
    let content = read_fixture("s1_notool.jsonl");
    let path = dir.path().join("t.jsonl");
    fs::write(&path, content.trim_end_matches('\n')).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(
        &mut store,
        HookEvent::PostToolBatch,
        "742660a8-64b5-4e03-9892-bd32ca4d5f59",
        &path,
    );
    fs::write(&path, &content).unwrap();
    run(
        &mut store,
        HookEvent::Stop,
        "742660a8-64b5-4e03-9892-bd32ca4d5f59",
        &path,
    );
    let turn = store
        .iterations_for_prompt("2ca92f21-3f9e-4079-ba71-69e7ef08d365")
        .unwrap();
    assert_eq!(turn.len(), 1);
    assert_eq!(turn[0].response_text, "2");
}

#[test]
fn session_end_drops_unattached_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_fixture(dir.path(), "s4_bg_agent.jsonl");
    let session = "40c046b1-c6a7-4e0c-8d41-f78b1f948059";
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, session, &path);
    assert!(
        store.session_state(session).unwrap().unwrap().safe_offset
            < fs::metadata(&path).unwrap().len()
    );
    run(&mut store, HookEvent::SessionEnd, session, &path);
    assert_eq!(
        store.session_state(session).unwrap().unwrap().safe_offset,
        fs::metadata(&path).unwrap().len()
    );
    assert_eq!(
        store
            .bg_completions("toolu_011MdRAe8c4q2Ua6xuusj2hf")
            .unwrap()
            .len(),
        1,
        "알림은 이미 처리됨"
    );
}

#[test]
fn rewritten_shorter_transcript_is_reprocessed_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_fixture(dir.path(), "s2_tools.jsonl");
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, S2, &path);
    let content = read_fixture("s2_tools.jsonl");
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    fs::write(&path, lines[..7].concat()).unwrap();
    run(&mut store, HookEvent::Stop, S2, &path);
    assert_eq!(store.counts().unwrap().iterations, 2);
}

#[test]
fn missing_transcript_is_an_error() {
    let mut store = RecordStore::open_in_memory().unwrap();
    let err = ingest(
        &mut store,
        &Redactor::new(&[]).unwrap(),
        OPTS,
        &req(HookEvent::Stop, "s", Path::new("/nonexistent/t.jsonl")),
    )
    .unwrap_err();
    assert!(matches!(err, IngestError::Transcript { .. }));
}

// ---- 합성 transcript: 마스킹, 백그라운드 결과 ----

fn line(v: Value) -> String {
    let mut s = v.to_string();
    s.push('\n');
    s
}

fn base(kind: &str, uuid: &str, extra: Value) -> Value {
    let mut v = json!({"type": kind, "uuid": uuid, "sessionId": "S", "cwd": "/w", "timestamp": "2026-11-02T00:00:00Z"});
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

fn bg_transcript(tool: &str, input: Value, structured: Value, output_file: &Path) -> String {
    let note = format!(
        "<task-notification>\n<task-id>T1</task-id>\n<tool-use-id>tu1</tool-use-id>\n<output-file>{}</output-file>\n<status>completed</status>\n<summary>done SECRET_TOKEN=abcdefgh123</summary>\n</task-notification>",
        output_file.display()
    );
    [
        line(base("user", "u1", json!({"promptId": "p1", "message": {"role": "user", "content": "실행해 API_KEY=abcdefgh123"}}))),
        line(base("assistant", "a1", json!({"message": {"id": "m1", "role": "assistant", "content": [{"type": "tool_use", "id": "tu1", "name": tool, "input": input}]}}))),
        line(base("user", "u2", json!({"promptId": "p1", "toolUseResult": structured, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "tu1", "content": "started"}]}}))),
        line(base("assistant", "a2", json!({"message": {"id": "m2", "role": "assistant", "content": [{"type": "text", "text": "기다립니다"}]}}))),
        line(base("user", "u3", json!({"promptId": "p2", "message": {"role": "user", "content": note}}))),
        line(base("assistant", "a3", json!({"message": {"id": "m3", "role": "assistant", "content": [{"type": "text", "text": "완료"}]}}))),
    ]
    .concat()
}

#[test]
fn bash_background_output_is_read_and_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bm.output");
    fs::write(&out, "line ok\nexport API_KEY=abcdefgh123\n").unwrap();
    let path = dir.path().join("t.jsonl");
    fs::write(
        &path,
        bg_transcript(
            "Bash",
            json!({"command": "x", "run_in_background": true}),
            json!({"backgroundTaskId": "T1"}),
            &out,
        ),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    let report = run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!((report.iterations, report.completions), (3, 1));

    let done = store.bg_completions("tu1").unwrap();
    let result = store
        .result(done[0].result_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert!(result.text.contains("line ok"));
    assert!(!result.text.contains("abcdefgh123"));
    assert_eq!(result.tool_name, "Bash");
    assert!(!done[0].summary.contains("abcdefgh123"));
    let prompt = &store.iteration("S:m1").unwrap().unwrap();
    assert!(
        !serde_json::to_string(&prompt.inputs)
            .unwrap()
            .contains("abcdefgh123")
    );
    assert_eq!(store.bg_task("tu1").unwrap().unwrap().status, "completed");
}

#[test]
fn agent_background_report_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let out = copy_fixture(dir.path(), "s4_bg_agent.agent-a65370863753f6d78.jsonl");
    let path = dir.path().join("t.jsonl");
    fs::write(
        &path,
        bg_transcript(
            "Agent",
            json!({"prompt": "count", "run_in_background": true}),
            json!({"agentId": "T1", "isAsync": true}),
            &out,
        ),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    let done = store.bg_completions("tu1").unwrap();
    let result = store
        .result(done[0].result_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.text, "24");
    assert_eq!(result.tool_name, "Agent");
}

#[test]
fn missing_output_file_keeps_summary_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    fs::write(
        &path,
        bg_transcript(
            "Bash",
            json!({"command": "x", "run_in_background": true}),
            json!({"backgroundTaskId": "T1"}),
            &dir.path().join("gone.output"),
        ),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    let done = store.bg_completions("tu1").unwrap();
    assert_eq!(done[0].result_id, None);
    assert!(done[0].summary.starts_with("done"));
}

#[test]
fn appended_lines_after_partial_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let full = bg_transcript(
        "Bash",
        json!({"command": "x"}),
        json!({}),
        &dir.path().join("none"),
    );
    let cut = full.len() / 2;
    fs::write(&path, &full.as_bytes()[..cut]).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::PostToolBatch, "S", &path);
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(&full.as_bytes()[cut..]).unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(store.counts().unwrap().iterations, 3);
}
