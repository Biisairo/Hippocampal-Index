mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use common::*;
use hippo::ingest::{
    HookEvent, ImportOutcome, ImportReport, IngestError, IngestOptions, IngestRequest, catch_up,
    import_session, ingest, session_start_cwd,
};
use hippo::redact::Redactor;
use hippo::store::{RecordStore, iteration_id};
use serde_json::{Value, json};

fn opts() -> IngestOptions {
    IngestOptions {
        max_tool_input_chars: 500,
        answer_tools: vec!["AskUserQuestion".into()],
    }
}

/// 기존 시나리오는 transcript의 첫 턴을 기준 턴으로 삼는다. (FR-EXT-12)
fn req(event: HookEvent, session: &str, path: &Path) -> IngestRequest {
    IngestRequest {
        event,
        session_id: session.into(),
        transcript_path: path.to_path_buf(),
        prompt_id: first_prompt(path),
    }
}

fn first_prompt(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| {
            v.get("promptId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn run(
    store: &mut RecordStore,
    event: HookEvent,
    session: &str,
    path: &Path,
) -> hippo::ingest::IngestReport {
    ingest(
        store,
        &Redactor::new(&[]).unwrap(),
        &opts(),
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
        &opts(),
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
    bg_transcript_with_task(tool, input, structured, output_file, "T1")
}

fn bg_transcript_with_task(
    tool: &str,
    input: Value,
    structured: Value,
    output_file: &Path,
    note_task: &str,
) -> String {
    let note = format!(
        "<task-notification>\n<task-id>{note_task}</task-id>\n<tool-use-id>tu1</tool-use-id>\n<output-file>{}</output-file>\n<status>completed</status>\n<summary>done SECRET_TOKEN=abcdefgh123</summary>\n</task-notification>",
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

/// 실측 배치: `<tmp>/<project>/<session>/tasks/<task_id>.output`
fn task_output_path(root: &Path, task: &str) -> PathBuf {
    let dir = root.join("tmp/claude-501/proj/S/tasks");
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{task}.output"))
}

fn bash_start() -> (Value, Value) {
    (
        json!({"command": "x", "run_in_background": true}),
        json!({"backgroundTaskId": "T1"}),
    )
}

fn completion_result(store: &RecordStore) -> Option<String> {
    let done = store.bg_completions("tu1").unwrap();
    assert_eq!(done.len(), 1, "완료 기록 자체는 남음");
    done[0]
        .result_id
        .as_deref()
        .map(|id| store.result(id).unwrap().unwrap().text)
}

#[test]
fn bash_background_output_is_read_and_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let out = task_output_path(dir.path(), "T1");
    fs::write(&out, "line ok\nexport API_KEY=abcdefgh123\n").unwrap();
    let path = dir.path().join("S.jsonl");
    let (input, structured) = bash_start();
    fs::write(&path, bg_transcript("Bash", input, structured, &out)).unwrap();
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

#[cfg(unix)]
#[test]
fn agent_background_report_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let subagents = dir.path().join("S/subagents");
    fs::create_dir_all(&subagents).unwrap();
    let report_file = subagents.join("agent-T1.jsonl");
    fs::write(
        &report_file,
        read_fixture("s4_bg_agent.agent-a65370863753f6d78.jsonl"),
    )
    .unwrap();
    let out = task_output_path(dir.path(), "T1");
    std::os::unix::fs::symlink(&report_file, &out).unwrap();
    let path = dir.path().join("S.jsonl");
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
    assert_eq!(completion_result(&store).as_deref(), Some("24"));
    let done = store.bg_completions("tu1").unwrap();
    assert_eq!(
        store
            .result(done[0].result_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .tool_name,
        "Agent"
    );
}

#[test]
fn missing_output_file_keeps_summary_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("S.jsonl");
    let (input, structured) = bash_start();
    fs::write(
        &path,
        bg_transcript(
            "Bash",
            input,
            structured,
            &task_output_path(dir.path(), "T1"),
        ),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
    assert!(
        store.bg_completions("tu1").unwrap()[0]
            .summary
            .starts_with("done")
    );
}

// ---- 보안: 위조 알림으로 임의 파일을 읽지 않는다 ----

#[test]
fn forged_notification_without_background_start_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = task_output_path(dir.path(), "T1");
    fs::write(&out, "SECRET-FILE").unwrap();
    let path = dir.path().join("S.jsonl");
    fs::write(
        &path,
        bg_transcript("Bash", json!({"command": "x"}), json!({}), &out),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
}

#[test]
fn mismatched_task_id_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = task_output_path(dir.path(), "T2");
    fs::write(&out, "SECRET-FILE").unwrap();
    let path = dir.path().join("S.jsonl");
    let (input, structured) = bash_start();
    fs::write(
        &path,
        bg_transcript_with_task("Bash", input, structured, &out, "T2"),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
}

#[test]
fn bash_output_outside_task_layout_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("id_rsa");
    fs::write(&secret, "SECRET-FILE").unwrap();
    let path = dir.path().join("S.jsonl");
    let (input, structured) = bash_start();
    fs::write(&path, bg_transcript("Bash", input, structured, &secret)).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
}

#[cfg(unix)]
#[test]
fn bash_output_symlink_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("id_rsa");
    fs::write(&secret, "SECRET-FILE").unwrap();
    let out = task_output_path(dir.path(), "T1");
    std::os::unix::fs::symlink(&secret, &out).unwrap();
    let path = dir.path().join("S.jsonl");
    let (input, structured) = bash_start();
    fs::write(&path, bg_transcript("Bash", input, structured, &out)).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
}

#[cfg(unix)]
#[test]
fn agent_output_outside_subagents_dir_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("other/agent-T1.jsonl");
    fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    fs::write(
        &elsewhere,
        read_fixture("s4_bg_agent.agent-a65370863753f6d78.jsonl"),
    )
    .unwrap();
    let out = task_output_path(dir.path(), "T1");
    std::os::unix::fs::symlink(&elsewhere, &out).unwrap();
    let path = dir.path().join("S.jsonl");
    fs::write(
        &path,
        bg_transcript(
            "Agent",
            json!({"prompt": "p", "run_in_background": true}),
            json!({"agentId": "T1"}),
            &out,
        ),
    )
    .unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
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

#[cfg(unix)]
#[test]
fn bash_output_through_symlinked_directory_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("secrets");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("T1.output"), "SECRET-FILE").unwrap();
    let session_dir = dir.path().join("tmp/S");
    fs::create_dir_all(&session_dir).unwrap();
    std::os::unix::fs::symlink(&real, session_dir.join("tasks")).unwrap();
    let out = session_dir.join("tasks/T1.output");
    let path = dir.path().join("S.jsonl");
    let (input, structured) = bash_start();
    fs::write(&path, bg_transcript("Bash", input, structured, &out)).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::Stop, "S", &path);
    assert_eq!(completion_result(&store), None);
}

#[test]
fn bash_output_with_parent_or_relative_components_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = task_output_path(dir.path(), "T1");
    fs::write(&out, "SECRET-FILE").unwrap();
    let dotted = out.parent().unwrap().join("../tasks/T1.output");
    for candidate in [dotted, PathBuf::from("S/tasks/T1.output")] {
        let path = dir.path().join("S.jsonl");
        let (input, structured) = bash_start();
        fs::write(&path, bg_transcript("Bash", input, structured, &candidate)).unwrap();
        let mut store = RecordStore::open_in_memory().unwrap();
        run(&mut store, HookEvent::Stop, "S", &path);
        assert_eq!(completion_result(&store), None, "{}", candidate.display());
    }
}

// ---- FR-PRJ-1: 세션 시작 경로 ----

#[test]
fn session_start_cwd_is_first_complete_line_with_cwd() {
    let text = [
        r#"{"type":"queue-operation"}"#,
        r#"{"type":"user","cwd":"/start","message":{"content":"a"}}"#,
        r#"{"type":"assistant","cwd":"/later"}"#,
    ]
    .join("\n")
        + "\n";
    assert_eq!(
        session_start_cwd(text.as_bytes()).as_deref(),
        Some("/start")
    );
}

#[test]
fn session_start_cwd_ignores_unterminated_and_missing_lines() {
    assert_eq!(session_start_cwd(b""), None);
    assert_eq!(
        session_start_cwd(br#"{"type":"user","cwd":"/partial"}"#),
        None,
        "개행으로 끝나지 않은 라인은 기록 중"
    );
    assert_eq!(
        session_start_cwd(b"not json\n{\"type\":\"system\"}\n"),
        None
    );
    assert_eq!(
        session_start_cwd(b"not json\n{\"cwd\":\"/x\"}\n").as_deref(),
        Some("/x")
    );
}

// ---- 기준 턴(FR-EXT-12)과 가져오기(FR-IMP-3~5) ----

fn turn(prompt: &str) -> Vec<String> {
    vec![
        user_line(
            &format!("u-{prompt}"),
            prompt,
            json!(format!("질문 {prompt}")),
        ),
        assistant_line(
            &format!("a-{prompt}"),
            &format!("m-{prompt}"),
            json!([{"type": "text", "text": format!("답변 {prompt}")}]),
        ),
    ]
}

/// 도구 호출만 있고 결과가 아직 없는 턴.
fn open_tool_turn(prompt: &str) -> Vec<String> {
    vec![
        user_line(
            &format!("u-{prompt}"),
            prompt,
            json!(format!("질문 {prompt}")),
        ),
        assistant_line(
            &format!("a-{prompt}"),
            &format!("m-{prompt}"),
            json!([{"type": "tool_use", "id": format!("tu-{prompt}"), "name": "Bash", "input": {"command": "ls"}}]),
        ),
    ]
}

fn tool_result_and_answer(prompt: &str) -> Vec<String> {
    vec![
        user_line(
            &format!("r-{prompt}"),
            prompt,
            json!([{"type": "tool_result", "tool_use_id": format!("tu-{prompt}"), "content": "결과 파일 목록"}]),
        ),
        assistant_line(
            &format!("b-{prompt}"),
            &format!("n-{prompt}"),
            json!([{"type": "text", "text": format!("마무리 {prompt}")}]),
        ),
    ]
}

fn in_session(lines: &[String], session: &str) -> String {
    joined(lines).replace(
        "\"sessionId\":\"S\"",
        &format!("\"sessionId\":\"{session}\""),
    )
}

fn concat(parts: &[Vec<String>]) -> Vec<String> {
    parts.concat()
}

fn write_transcript(dir: &Path, session: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("{session}.jsonl"));
    fs::write(&path, body).unwrap();
    path
}

fn live(
    store: &mut RecordStore,
    event: HookEvent,
    session: &str,
    path: &Path,
    prompt: Option<&str>,
) -> hippo::ingest::IngestReport {
    ingest(
        store,
        &Redactor::new(&[]).unwrap(),
        &opts(),
        &IngestRequest {
            event,
            session_id: session.into(),
            transcript_path: path.to_path_buf(),
            prompt_id: prompt.map(str::to_string),
        },
    )
    .unwrap()
}

fn import(store: &mut RecordStore, session: &str, path: &Path) -> ImportReport {
    import_session(store, &Redactor::new(&[]).unwrap(), &opts(), session, path).unwrap()
}

fn has(store: &RecordStore, session: &str, message: &str) -> bool {
    store
        .iteration(&iteration_id(session, message))
        .unwrap()
        .is_some()
}

#[test]
fn first_seen_session_starts_at_hook_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let first = joined(&turn("p1"));
    let body = in_session(&concat(&[turn("p1"), turn("p2")]), "S");
    let path = write_transcript(dir.path(), "S", &body);
    let mut store = RecordStore::open_in_memory().unwrap();
    let report = live(&mut store, HookEvent::Stop, "S", &path, Some("p2"));
    assert_eq!(report.iterations, 1);
    assert!(!has(&store, "S", "m-p1"), "기준 턴 앞은 저장하지 않음");
    assert!(has(&store, "S", "m-p2"));
    let state = store.session_state("S").unwrap().unwrap();
    assert_eq!(state.start_offset, first.len() as u64);
    assert_eq!(state.safe_offset, body.len() as u64);
}

#[test]
fn first_seen_session_without_or_unknown_prompt_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_transcript(dir.path(), "S", &in_session(&turn("p1"), "S"));
    let mut store = RecordStore::open_in_memory().unwrap();
    for prompt in [None, Some("no-such-prompt")] {
        let report = live(&mut store, HookEvent::Stop, "S", &path, prompt);
        assert_eq!(report.iterations, 0, "{prompt:?}");
        assert!(store.session_state("S").unwrap().is_none(), "{prompt:?}");
    }
}

#[test]
fn known_session_continues_from_safe_offset_regardless_of_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_transcript(dir.path(), "S", &in_session(&turn("p1"), "S"));
    let mut store = RecordStore::open_in_memory().unwrap();
    live(&mut store, HookEvent::Stop, "S", &path, Some("p1"));
    fs::write(&path, in_session(&concat(&[turn("p1"), turn("p2")]), "S")).unwrap();
    let report = live(&mut store, HookEvent::Stop, "S", &path, Some("p2"));
    assert_eq!(report.iterations, 1);
    assert!(has(&store, "S", "m-p1") && has(&store, "S", "m-p2"));
    assert_eq!(store.session_state("S").unwrap().unwrap().start_offset, 0);
}

#[test]
fn fork_session_stores_only_turns_after_fork() {
    let dir = tempfile::tempdir().unwrap();
    let copied = in_session(&concat(&[turn("p1"), turn("p2")]), "B");
    let body = format!("{copied}{}", in_session(&turn("p3"), "B"));
    let path = write_transcript(dir.path(), "B", &body);
    let mut store = RecordStore::open_in_memory().unwrap();
    let report = live(&mut store, HookEvent::Stop, "B", &path, Some("p3"));
    assert_eq!(report.iterations, 1);
    assert!(!has(&store, "B", "m-p1") && !has(&store, "B", "m-p2"));
    assert!(has(&store, "B", "m-p3"));
}

#[test]
fn rewritten_transcript_restarts_at_hook_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_transcript(
        dir.path(),
        "S",
        &in_session(&concat(&[turn("p1"), turn("p2")]), "S"),
    );
    let mut store = RecordStore::open_in_memory().unwrap();
    live(&mut store, HookEvent::Stop, "S", &path, Some("p1"));
    fs::write(&path, in_session(&turn("p5"), "S")).unwrap();
    let report = live(&mut store, HookEvent::Stop, "S", &path, Some("p5"));
    assert_eq!(report.iterations, 1);
    assert!(has(&store, "S", "m-p5"));
}

#[test]
fn import_reads_unseen_session_from_start() {
    let dir = tempfile::tempdir().unwrap();
    let body = in_session(&concat(&[turn("p1"), turn("p2")]), "S");
    let path = write_transcript(dir.path(), "S", &body);
    let mut store = RecordStore::open_in_memory().unwrap();
    let report = import(&mut store, "S", &path);
    assert_eq!(report.outcome, ImportOutcome::Imported);
    assert_eq!((report.iterations, report.fork_skipped), (2, 0));
    let state = store.session_state("S").unwrap().unwrap();
    assert_eq!(
        (state.start_offset, state.safe_offset),
        (0, body.len() as u64)
    );
    let again = import(&mut store, "S", &path);
    assert_eq!(again.outcome, ImportOutcome::Unchanged);
    assert_eq!(again.iterations, 0);
}

#[test]
fn import_holds_last_iteration_with_missing_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let head = in_session(&turn("p1"), "S");
    let body = format!("{head}{}", in_session(&open_tool_turn("p2"), "S"));
    let path = write_transcript(dir.path(), "S", &body);
    let mut store = RecordStore::open_in_memory().unwrap();
    let report = import(&mut store, "S", &path);
    assert_eq!(report.iterations, 1);
    assert!(!has(&store, "S", "m-p2"), "결과가 없는 도구 호출은 보류");
    assert_eq!(
        store.session_state("S").unwrap().unwrap().safe_offset,
        head.len() as u64
    );

    let done = format!("{body}{}", in_session(&tool_result_and_answer("p2"), "S"));
    fs::write(&path, done).unwrap();
    let report = import(&mut store, "S", &path);
    assert_eq!(report.outcome, ImportOutcome::Imported);
    assert_eq!(report.iterations, 2);
    assert!(has(&store, "S", "m-p2") && has(&store, "S", "n-p2"));
}

#[test]
fn import_backfills_turns_before_live_start() {
    let dir = tempfile::tempdir().unwrap();
    let body = in_session(&concat(&[turn("p1"), turn("p2")]), "S");
    let path = write_transcript(dir.path(), "S", &body);
    let mut store = RecordStore::open_in_memory().unwrap();
    live(&mut store, HookEvent::Stop, "S", &path, Some("p2"));
    let before = store.session_state("S").unwrap().unwrap();
    assert!(before.start_offset > 0);

    let report = import(&mut store, "S", &path);
    assert_eq!(report.outcome, ImportOutcome::Imported);
    assert_eq!(report.iterations, 1);
    assert!(has(&store, "S", "m-p1"));
    let after = store.session_state("S").unwrap().unwrap();
    assert_eq!(after.start_offset, 0);
    assert_eq!(
        after.safe_offset, before.safe_offset,
        "실시간 offset은 그대로"
    );
    assert_eq!(
        import(&mut store, "S", &path).outcome,
        ImportOutcome::Unchanged
    );
    assert_eq!(store.counts().unwrap().iterations, 2);
}

#[test]
fn import_skips_fork_copies_of_other_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_transcript(
        dir.path(),
        "A",
        &in_session(&concat(&[turn("p1"), turn("p2")]), "A"),
    );
    let b = write_transcript(
        dir.path(),
        "B",
        &in_session(&concat(&[turn("p1"), turn("p2"), turn("p3")]), "B"),
    );
    let mut store = RecordStore::open_in_memory().unwrap();
    assert_eq!(import(&mut store, "A", &a).iterations, 2);
    let report = import(&mut store, "B", &b);
    assert_eq!((report.iterations, report.fork_skipped), (1, 2));
    assert!(!has(&store, "B", "m-p1") && has(&store, "B", "m-p3"));
    assert_eq!(store.counts().unwrap().iterations, 3);
}

#[test]
fn imported_embed_jobs_are_bulk_and_live_jobs_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_transcript(dir.path(), "A", &in_session(&turn("p1"), "A"));
    let b = write_transcript(dir.path(), "B", &in_session(&turn("p9"), "B"));
    let mut store = RecordStore::open_in_memory().unwrap();
    import(&mut store, "A", &a);
    live(&mut store, HookEvent::Stop, "B", &b, Some("p9"));
    let jobs = store.pending_jobs().unwrap();
    let bulk: Vec<_> = jobs
        .iter()
        .map(|j| (j.job.ref_id.clone(), j.bulk))
        .collect();
    assert_eq!(
        bulk,
        vec![
            (iteration_id("A", "m-p1"), true),
            (iteration_id("B", "m-p9"), false)
        ]
    );
}

#[test]
fn user_answer_is_part_of_embed_text() {
    let dir = tempfile::tempdir().unwrap();
    let lines = vec![
        user_line("u1", "p1", json!("결정해 줘")),
        assistant_line(
            "a1",
            "m1",
            json!([{"type": "tool_use", "id": "tu1", "name": "AskUserQuestion", "input": {"q": "fork?"}}]),
        ),
        user_line(
            "r1",
            "p1",
            json!([{"type": "tool_result", "tool_use_id": "tu1", "content": "User answered: skip copies"}]),
        ),
        assistant_line("a2", "m2", json!([{"type": "text", "text": "정했습니다"}])),
    ];
    let path = write_transcript(dir.path(), "S", &in_session(&lines, "S"));
    let mut store = RecordStore::open_in_memory().unwrap();
    live(&mut store, HookEvent::Stop, "S", &path, Some("p1"));
    let it = store.iteration(&iteration_id("S", "m1")).unwrap().unwrap();
    assert!(
        it.embed_text
            .contains("[answer] User answered: skip copies"),
        "{}",
        it.embed_text
    );
}

// ---- 따라잡기 (FR-CAP-9) ----

const TURN_END: &str = "{\"type\":\"system\",\"subtype\":\"turn_duration\",\"sessionId\":\"afd070e6-4625-4b31-8c43-f1f553949222\"}\n";

fn catch_up_run(store: &mut RecordStore, path: &Path) -> hippo::ingest::IngestReport {
    catch_up(store, &Redactor::new(&[]).unwrap(), &opts(), S2, path).unwrap()
}

/// Stop 처리가 마지막 assistant 라인이 기록되기 전에 읽은 상태를 만든다.
fn stale_stop(dir: &Path) -> (RecordStore, PathBuf, String) {
    let content = read_fixture("s2_tools.jsonl");
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let path = dir.join("t.jsonl");
    fs::write(&path, lines[..8].concat()).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    assert_eq!(run(&mut store, HookEvent::Stop, S2, &path).iterations, 1);
    (store, path, content)
}

#[test]
fn catch_up_stores_last_iteration_after_turn_end_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, path, content) = stale_stop(dir.path());
    let start = store.session_state(S2).unwrap().unwrap().start_offset;
    fs::write(&path, format!("{content}{TURN_END}")).unwrap();
    assert_eq!(catch_up_run(&mut store, &path).iterations, 1);
    assert_eq!(store.counts().unwrap().iterations, 2);
    let state = store.session_state(S2).unwrap().unwrap();
    assert_eq!(state.safe_offset, fs::metadata(&path).unwrap().len());
    assert_eq!(state.start_offset, start, "시작 offset은 바꾸지 않음");
    assert_eq!(catch_up_run(&mut store, &path).iterations, 0, "멱등");
}

#[test]
fn catch_up_does_not_finish_turn_without_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, path, content) = stale_stop(dir.path());
    fs::write(&path, &content).unwrap();
    assert_eq!(catch_up_run(&mut store, &path).iterations, 0);
    assert_eq!(store.counts().unwrap().iterations, 1);
}

#[test]
fn catch_up_keeps_iteration_with_pending_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    let content = read_fixture("s2_tools.jsonl");
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let path = dir.path().join("t.jsonl");
    fs::write(&path, lines[..2].concat()).unwrap();
    let mut store = RecordStore::open_in_memory().unwrap();
    run(&mut store, HookEvent::PostToolBatch, S2, &path);
    fs::write(&path, lines[..4].concat()).unwrap();
    assert_eq!(catch_up_run(&mut store, &path).iterations, 0);
    assert_eq!(store.counts().unwrap().iterations, 0);
}

#[test]
fn catch_up_ignores_unknown_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_fixture(dir.path(), "s2_tools.jsonl");
    let mut store = RecordStore::open_in_memory().unwrap();
    assert_eq!(catch_up_run(&mut store, &path).iterations, 0);
    assert!(store.session_state(S2).unwrap().is_none());
}
