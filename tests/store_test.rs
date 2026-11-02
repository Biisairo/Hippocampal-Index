mod common;

use common::*;
use hippo::bg::{BackgroundKind, TaskNotification};
use hippo::embed_text::build_embed_text;
use hippo::store::{
    Batch, EmbedJob, FtsFilter, NewCompletion, NewIteration, RecordKind, RecordStore,
};
use hippo::transcript::{ExtractorState, Iteration, IterationInput, ToolCall, ToolResultData};
use rusqlite::Connection;
use serde_json::json;

fn batch_from_fixture(name: &str) -> Batch {
    let (mut ex, events) = run_fixture(name);
    let mut its = iterations(&events);
    its.extend(iterations(&ex.finish_turn()));
    let session_id = its[0].session_id.clone();
    Batch {
        session_id,
        transcript_path: format!("/t/{name}"),
        iterations: its.into_iter().map(new_iteration).collect(),
        completions: vec![],
        state: ex.state(),
        bulk: false,
    }
}

fn new_iteration(it: Iteration) -> NewIteration {
    let embed_text = build_embed_text(&it, 500, &[]);
    NewIteration {
        iteration: it,
        embed_text,
    }
}

fn synthetic(
    session: &str,
    mid: &str,
    cwd: &str,
    text: &str,
    results: &[(&str, &str)],
) -> NewIteration {
    new_iteration(Iteration {
        session_id: session.into(),
        prompt_id: Some(format!("p-{session}")),
        message_id: mid.into(),
        iteration_index: 0,
        cwd: cwd.into(),
        ts: "2026-11-02T00:00:00Z".into(),
        inputs: vec![IterationInput::Prompt {
            text: "질문".into(),
        }],
        texts: vec![text.into()],
        tool_calls: results
            .iter()
            .map(|(id, out)| ToolCall {
                id: (*id).into(),
                name: "Read".into(),
                input: json!({"file_path": "a.rs"}),
                result: Some(ToolResultData {
                    text: (*out).into(),
                    is_error: false,
                    structured: None,
                }),
            })
            .collect(),
    })
}

fn batch(session: &str, iterations: Vec<NewIteration>, completions: Vec<NewCompletion>) -> Batch {
    Batch {
        session_id: session.into(),
        transcript_path: "/t".into(),
        iterations,
        completions,
        state: ExtractorState::default(),
        bulk: false,
    }
}

fn completion(uuid: &str, tool_use_id: &str, result: Option<&str>) -> NewCompletion {
    NewCompletion {
        session_id: "78507f8e-dc4c-49a8-8c47-a6e478d0ed71".into(),
        line_uuid: uuid.into(),
        notification: TaskNotification {
            task_id: "bm0rqrvzr".into(),
            tool_use_id: tool_use_id.into(),
            output_file: None,
            status: "completed".into(),
            summary: "done (exit code 0)".into(),
        },
        result_text: result.map(str::to_string),
        ts: "2026-11-02T01:00:00Z".into(),
    }
}

// ---- FR-STO-1, FR-STO-3 ----

#[test]
fn stores_iterations_and_results_from_fixture() {
    let mut store = RecordStore::open_in_memory().unwrap();
    let b = batch_from_fixture("s2_tools.jsonl");
    store.apply(&b).unwrap();

    let counts = store.counts().unwrap();
    assert_eq!(
        (counts.iterations, counts.results, counts.embed_jobs),
        (2, 2, 4)
    );

    let first = &b.iterations[0].iteration;
    let id = format!("{}:{}", first.session_id, first.message_id);
    let stored = store.iteration(&id).unwrap().unwrap();
    assert_eq!(stored.prompt_id, first.prompt_id);
    assert_eq!(stored.inputs, first.inputs);
    assert_eq!(stored.embed_text, b.iterations[0].embed_text);
    assert_eq!(stored.tool_calls.len(), 2);
    let rid = stored.tool_calls[0].result_id.clone().unwrap();
    assert_eq!(rid.len(), 64, "SHA-256 hex");
    let result = store.result(&rid).unwrap().unwrap();
    assert!(result.text.contains("import json"));
    assert_eq!(result.tool_name, "Read");
    assert_eq!(
        store.iterations_referencing_result(&rid).unwrap(),
        vec![id.clone()]
    );

    let turn = store
        .iterations_for_prompt(first.prompt_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        turn.iter().map(|i| i.iteration_index).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        turn[1].response_text,
        b.iterations[1].iteration.texts.join("\n")
    );
}

#[test]
fn tool_calls_json_excludes_result_text() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch(
            "s",
            vec![synthetic(
                "s",
                "m1",
                "/w",
                "응답",
                &[("c1", "SECRET_CONTENT_BODY")],
            )],
            vec![],
        ))
        .unwrap();
    let it = store.iteration("s:m1").unwrap().unwrap();
    assert!(
        !serde_json::to_string(&it.tool_calls.iter().map(|c| &c.input).collect::<Vec<_>>())
            .unwrap()
            .contains("SECRET_CONTENT_BODY")
    );
    assert_eq!(it.tool_calls[0].name, "Read");
    assert!(!it.tool_calls[0].is_error);
}

// ---- FR-EXT-10, QA-2 ----

#[test]
fn applying_same_batch_twice_is_idempotent() {
    let mut store = RecordStore::open_in_memory().unwrap();
    let b = batch_from_fixture("s2_tools.jsonl");
    store.apply(&b).unwrap();
    let before = store.counts().unwrap();
    store.apply(&b).unwrap();
    assert_eq!(store.counts().unwrap(), before);
}

// ---- FR-STO-4 ----

#[test]
fn identical_results_are_deduplicated() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch(
            "s",
            vec![
                synthetic("s", "m1", "/w", "첫 번째", &[("c1", "same file body")]),
                synthetic("s", "m2", "/w", "두 번째", &[("c2", "same file body")]),
            ],
            vec![],
        ))
        .unwrap();
    let counts = store.counts().unwrap();
    assert_eq!(
        (counts.iterations, counts.results, counts.embed_jobs),
        (2, 1, 3)
    );
    let rid = store.iteration("s:m1").unwrap().unwrap().tool_calls[0]
        .result_id
        .clone()
        .unwrap();
    let mut refs = store.iterations_referencing_result(&rid).unwrap();
    refs.sort();
    assert_eq!(refs, vec!["s:m1".to_string(), "s:m2".to_string()]);
}

#[test]
fn empty_results_are_not_stored() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch(
            "s",
            vec![synthetic("s", "m1", "/w", "응답", &[("c1", "")])],
            vec![],
        ))
        .unwrap();
    assert_eq!(store.counts().unwrap().results, 0);
    assert_eq!(
        store.iteration("s:m1").unwrap().unwrap().tool_calls[0].result_id,
        None
    );
}

// ---- FR-EXT-1 상태 저장 ----

#[test]
fn session_state_is_saved_and_updated() {
    let mut store = RecordStore::open_in_memory().unwrap();
    assert_eq!(store.session_state("s").unwrap(), None);
    let mut b = batch("s", vec![], vec![]);
    b.state.safe_offset = 10;
    store.apply(&b).unwrap();
    assert_eq!(store.session_state("s").unwrap().unwrap().safe_offset, 10);
    b.state.safe_offset = 42;
    b.state.current_prompt_id = Some("p".into());
    store.apply(&b).unwrap();
    assert_eq!(store.session_state("s").unwrap(), Some(b.state.clone()));
}

// ---- FR-BG-1, 3, 4, 6 ----

#[test]
fn background_start_and_completion_are_linked() {
    let mut store = RecordStore::open_in_memory().unwrap();
    let b = batch_from_fixture("s5_bg_bash.jsonl");
    store.apply(&b).unwrap();
    let tool_use_id = "toolu_01D9M8zTeQeTTXW1FhAmqgRV";
    let task = store.bg_task(tool_use_id).unwrap().unwrap();
    assert_eq!(task.kind, BackgroundKind::Bash);
    assert_eq!(task.task_id, "bm0rqrvzr");
    assert_eq!(task.status, "pending");
    assert_eq!(
        task.iteration_id,
        format!("{}:{}", b.session_id, b.iterations[0].iteration.message_id)
    );

    store
        .apply(&batch(
            &b.session_id,
            vec![],
            vec![completion(
                "n1",
                tool_use_id,
                Some("bg-done\n[exited with code 0]"),
            )],
        ))
        .unwrap();
    let task = store.bg_task(tool_use_id).unwrap().unwrap();
    assert_eq!(task.status, "completed");
    let done = store.bg_completions(tool_use_id).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].seq, 1);
    let result = store
        .result(done[0].result_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert!(result.text.starts_with("bg-done"));
    assert_eq!(result.tool_name, "Bash");
}

#[test]
fn repeated_notification_uuid_is_ignored_and_new_uuid_appends() {
    let mut store = RecordStore::open_in_memory().unwrap();
    let c1 = completion("n1", "t1", Some("out-1"));
    store.apply(&batch("s", vec![], vec![c1.clone()])).unwrap();
    store.apply(&batch("s", vec![], vec![c1])).unwrap();
    store
        .apply(&batch(
            "s",
            vec![],
            vec![completion("n2", "t1", Some("out-2"))],
        ))
        .unwrap();
    let done = store.bg_completions("t1").unwrap();
    assert_eq!(
        done.iter()
            .map(|c| (c.line_uuid.as_str(), c.seq))
            .collect::<Vec<_>>(),
        vec![("n1", 1), ("n2", 2)]
    );
}

#[test]
fn completion_without_start_or_output_is_kept() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch("s", vec![], vec![completion("n1", "unknown", None)]))
        .unwrap();
    let done = store.bg_completions("unknown").unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].result_id, None);
    assert_eq!(done[0].summary, "done (exit code 0)");
    assert_eq!(store.bg_task("unknown").unwrap(), None);
}

// ---- FR-STO-7 작업 큐 ----

#[test]
fn embed_jobs_are_listed_and_completed() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch(
            "s",
            vec![synthetic("s", "m1", "/w", "응답", &[("c1", "body")])],
            vec![],
        ))
        .unwrap();
    let mut jobs = store.embed_jobs(10).unwrap();
    jobs.sort();
    assert_eq!(jobs.len(), 2);
    assert_eq!(
        jobs[0],
        EmbedJob {
            kind: RecordKind::Iteration,
            ref_id: "s:m1".into()
        }
    );
    assert_eq!(jobs[1].kind, RecordKind::Result);
    assert_eq!(store.embed_jobs(1).unwrap().len(), 1);
    store.complete_embed_jobs(&jobs[..1]).unwrap();
    assert_eq!(store.embed_jobs(10).unwrap(), jobs[1..].to_vec());
}

// ---- FR-STO-8 ----

fn fts_store() -> RecordStore {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch(
            "s1",
            vec![
                synthetic(
                    "s1",
                    "m1",
                    "/proj/a",
                    "토큰 만료 비교가 반대입니다 validateToken",
                    &[("c1", "fn validateToken() {}")],
                ),
                synthetic(
                    "s1",
                    "m2",
                    "/proj/b",
                    "다른 프로젝트의 validateToken 이야기",
                    &[],
                ),
            ],
            vec![],
        ))
        .unwrap();
    store
        .apply(&batch(
            "s2",
            vec![synthetic(
                "s2",
                "m3",
                "/proj/a",
                "현재 세션 validateToken",
                &[],
            )],
            vec![],
        ))
        .unwrap();
    store
}

fn ids(hits: Vec<hippo::store::FtsHit>) -> Vec<String> {
    let mut v: Vec<String> = hits.into_iter().map(|h| h.id).collect();
    v.sort();
    v
}

#[test]
fn fts_matches_korean_and_code_tokens_with_filters() {
    let store = fts_store();
    let all = store
        .search_iterations_fts("validateToken", &FtsFilter::default(), 10)
        .unwrap();
    assert_eq!(ids(all), vec!["s1:m1", "s1:m2", "s2:m3"]);

    let filter = FtsFilter {
        exclude_session: Some("s2".into()),
    };
    assert_eq!(
        ids(store
            .search_iterations_fts("validateToken", &filter, 10)
            .unwrap()),
        vec!["s1:m1", "s1:m2"],
        "제외한 세션만 빠짐"
    );
    assert_eq!(
        ids(store
            .search_iterations_fts("만료 비교가", &FtsFilter::default(), 10)
            .unwrap()),
        vec!["s1:m1"]
    );
}

#[test]
fn fts_ignores_short_tokens_and_escapes_quotes() {
    let store = fts_store();
    assert!(
        store
            .search_iterations_fts("토큰 버그", &FtsFilter::default(), 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .search_iterations_fts("\"unterminated AND (", &FtsFilter::default(), 10)
            .is_ok()
    );
}

#[test]
fn fts_limit_and_results_search() {
    let store = fts_store();
    assert_eq!(
        store
            .search_iterations_fts("validateToken", &FtsFilter::default(), 1)
            .unwrap()
            .len(),
        1
    );
    let hits = store
        .search_results_fts(
            "validateToken",
            &FtsFilter {
                exclude_session: None,
            },
            10,
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        store
            .search_results_fts(
                "validateToken",
                &FtsFilter {
                    exclude_session: Some("s1".into())
                },
                10
            )
            .unwrap()
            .is_empty()
    );
}

// ---- FR-STO-10, 파일 저장 ----

#[test]
fn failed_batch_leaves_no_partial_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.sqlite");
    let mut store = RecordStore::open(&path).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TRIGGER fail BEFORE INSERT ON bg_completions BEGIN SELECT RAISE(ABORT, 'boom'); END;")
        .unwrap();
    let mut b = batch(
        "s",
        vec![synthetic("s", "m1", "/w", "응답", &[("c1", "body")])],
        vec![completion("n1", "t1", None)],
    );
    b.state.safe_offset = 99;
    assert!(store.apply(&b).is_err());
    assert_eq!(store.counts().unwrap().iterations, 0);
    assert_eq!(store.counts().unwrap().results, 0);
    assert_eq!(store.session_state("s").unwrap(), None);
}

#[test]
fn file_store_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.sqlite");
    {
        let mut store = RecordStore::open(&path).unwrap();
        store.apply(&batch_from_fixture("s1_notool.jsonl")).unwrap();
    }
    let store = RecordStore::open(&path).unwrap();
    assert_eq!(store.counts().unwrap().iterations, 1);
}

// ---- DR-2 ----

#[cfg(unix)]
#[test]
fn database_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let path = root.join("records.sqlite");
    let mut store = RecordStore::open(&path).unwrap();
    store.apply(&batch_from_fixture("s1_notool.jsonl")).unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&path), 0o600);
    let wal = root.join("records.sqlite-wal");
    if wal.exists() {
        assert_eq!(mode(&wal), 0o600);
    }
}

#[cfg(unix)]
#[test]
fn existing_database_permissions_are_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.sqlite");
    std::fs::write(&path, b"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    RecordStore::open(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// ---- FR-STO-7, FR-DMN-6: 작업 등록 시각 ----

#[test]
fn pending_jobs_carry_enqueue_time_in_order() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch(
            "s",
            vec![synthetic("s", "m1", "/w", "첫째", &[])],
            vec![],
        ))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    store
        .apply(&batch(
            "s",
            vec![synthetic("s", "m2", "/w", "둘째", &[])],
            vec![],
        ))
        .unwrap();
    let pending = store.pending_jobs().unwrap();
    let ids: Vec<&str> = pending.iter().map(|p| p.job.ref_id.as_str()).collect();
    assert_eq!(ids, ["s:m1", "s:m2"]);
    assert!(pending[0].enqueued_at < pending[1].enqueued_at);
    assert!(pending[0].enqueued_at > 1_700_000_000_000, "유닉스 밀리초");
}

// ---- FR-PRJ-9 (1): 병합 ----

fn merge_pair(dir: &std::path::Path) -> (std::path::PathBuf, RecordStore) {
    let src_path = dir.join("a.sqlite");
    let mut src = RecordStore::open(&src_path).unwrap();
    src.apply(&batch(
        "sa",
        vec![synthetic(
            "sa",
            "m1",
            "/a",
            "옮겨올 응답 validateToken",
            &[("c1", "원본결과 본문"), ("c2", "공통 결과")],
        )],
        vec![],
    ))
    .unwrap();
    let mut dst = RecordStore::open(&dir.join("b.sqlite")).unwrap();
    dst.apply(&batch(
        "sb",
        vec![synthetic(
            "sb",
            "m9",
            "/b",
            "대상 응답",
            &[("c9", "공통 결과")],
        )],
        vec![],
    ))
    .unwrap();
    (src_path, dst)
}

#[test]
fn merge_copies_records_fts_and_links_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let (src_path, mut dst) = merge_pair(dir.path());
    let before = dst.counts().unwrap();
    let report = dst.merge_from(&src_path, true).unwrap();
    assert_eq!((report.iterations, report.results), (1, 1));
    let after = dst.counts().unwrap();
    assert_eq!(after.iterations, before.iterations + 1);
    assert_eq!(
        after.results,
        before.results + 1,
        "공통 결과는 중복 저장하지 않음"
    );

    let it = dst.iteration("sa:m1").unwrap().unwrap();
    assert!(it.tool_calls.iter().all(|c| c.result_id.is_some()));
    let hits = dst
        .search_iterations_fts("validateToken", &FtsFilter::default(), 10)
        .unwrap();
    assert_eq!(ids(hits), ["sa:m1"]);
    let hits = dst
        .search_results_fts("원본결과", &FtsFilter::default(), 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        dst.session_state("sa").unwrap().is_some(),
        "세션 해석기 상태도 옮김"
    );

    let again = dst.merge_from(&src_path, true).unwrap();
    assert_eq!((again.iterations, again.results), (0, 0));
    assert_eq!(dst.counts().unwrap(), after);
    let hits = dst
        .search_iterations_fts("validateToken", &FtsFilter::default(), 10)
        .unwrap();
    assert_eq!(hits.len(), 1, "FTS 행도 중복되지 않음");
}

#[test]
fn merge_with_enqueue_registers_jobs_for_merged_records() {
    let dir = tempfile::tempdir().unwrap();
    let (src_path, mut dst) = merge_pair(dir.path());
    let own = dst.embed_jobs(100).unwrap();
    dst.complete_embed_jobs(&own).unwrap();
    {
        let mut src = RecordStore::open(&src_path).unwrap();
        let jobs = src.embed_jobs(100).unwrap();
        src.complete_embed_jobs(&jobs).unwrap();
    }
    let report = dst.merge_from(&src_path, true).unwrap();
    let mut jobs: Vec<String> = dst
        .embed_jobs(100)
        .unwrap()
        .into_iter()
        .map(|j| format!("{}:{}", j.kind.as_str(), j.ref_id))
        .collect();
    jobs.sort();
    assert_eq!(report.jobs, 2);
    assert_eq!(jobs.len(), 2);
    assert!(jobs.contains(&"iteration:sa:m1".to_string()), "{jobs:?}");
}

#[test]
fn merge_without_enqueue_copies_only_pending_source_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let (src_path, mut dst) = merge_pair(dir.path());
    let own = dst.embed_jobs(100).unwrap();
    dst.complete_embed_jobs(&own).unwrap();
    {
        let mut src = RecordStore::open(&src_path).unwrap();
        let jobs: Vec<EmbedJob> = src
            .embed_jobs(100)
            .unwrap()
            .into_iter()
            .filter(|j| j.kind == RecordKind::Result)
            .collect();
        src.complete_embed_jobs(&jobs).unwrap();
    }
    let report = dst.merge_from(&src_path, false).unwrap();
    let jobs = dst.embed_jobs(100).unwrap();
    assert_eq!(report.jobs, 1);
    assert_eq!(
        jobs,
        [EmbedJob {
            kind: RecordKind::Iteration,
            ref_id: "sa:m1".into()
        }]
    );
}

#[test]
fn merge_copies_background_records() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("a.sqlite");
    let mut src = RecordStore::open(&src_path).unwrap();
    src.apply(&batch_from_fixture("s5_bg_bash.jsonl")).unwrap();
    let tool_use_ids: Vec<String> = src
        .iteration_page(0, 100)
        .unwrap()
        .into_iter()
        .filter_map(|(_, id)| src.iteration(&id).unwrap())
        .flat_map(|it| it.tool_calls.into_iter().map(|c| c.id))
        .collect();
    let bg: Vec<_> = tool_use_ids
        .iter()
        .filter_map(|id| src.bg_task(id).unwrap())
        .collect();
    assert!(!bg.is_empty(), "fixture에 백그라운드 시작 호출이 있음");
    drop(src);
    let mut dst = RecordStore::open(&dir.path().join("b.sqlite")).unwrap();
    dst.merge_from(&src_path, false).unwrap();
    for task in bg {
        assert_eq!(dst.bg_task(&task.tool_use_id).unwrap(), Some(task));
    }
}

// ---- FR-STO-11 ----

fn hippo_iteration(session: &str, mid: &str) -> NewIteration {
    let result = |text: &str| {
        Some(ToolResultData {
            text: text.into(),
            is_error: false,
            structured: None,
        })
    };
    new_iteration(Iteration {
        session_id: session.into(),
        prompt_id: Some(format!("p-{session}")),
        message_id: mid.into(),
        iteration_index: 0,
        cwd: "/a".into(),
        ts: "2026-11-02T00:00:00Z".into(),
        inputs: vec![],
        texts: vec![],
        tool_calls: vec![
            ToolCall {
                id: "h1".into(),
                name: "Bash".into(),
                input: json!({"command": "/opt/bin/hippo search fork"}),
                result: result("hippoOutputMarker recalled memory"),
            },
            ToolCall {
                id: "r1".into(),
                name: "Read".into(),
                input: json!({"file_path": "a.rs"}),
                result: result("readOutputMarker file body"),
            },
        ],
    })
}

#[test]
fn hippo_call_results_are_stored_but_not_indexed() {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&batch("s1", vec![hippo_iteration("s1", "m1")], vec![]))
        .unwrap();
    assert_eq!(store.counts().unwrap().results, 2, "원본에는 둘 다 저장");
    let fts = |q: &str| {
        store
            .search_results_fts(q, &FtsFilter::default(), 10)
            .unwrap()
            .len()
    };
    assert_eq!(fts("readOutputMarker"), 1);
    assert_eq!(fts("hippoOutputMarker"), 0, "hippo 결과는 FTS 제외");
    let jobs: Vec<String> = store
        .embed_jobs(100)
        .unwrap()
        .into_iter()
        .map(|j| j.ref_id)
        .collect();
    let page: Vec<String> = store
        .result_page(0, 100)
        .unwrap()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for listed in [&jobs, &page] {
        assert!(
            listed.iter().all(|id| store
                .result(id)
                .unwrap()
                .is_none_or(|r| !r.text.contains("hippoOutputMarker"))),
            "임베딩 작업과 재색인 대상에서 제외: {listed:?}"
        );
    }
    assert_eq!(page.len(), 1);
    let shown = store.iteration("s1:m1").unwrap().unwrap();
    assert!(
        shown.tool_calls[0].result_id.is_some(),
        "show는 결과를 그대로 연결"
    );
}

#[test]
fn merge_keeps_hippo_call_results_unindexed() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("a.sqlite");
    RecordStore::open(&src_path)
        .unwrap()
        .apply(&batch("s1", vec![hippo_iteration("s1", "m1")], vec![]))
        .unwrap();
    let mut dst = RecordStore::open(&dir.path().join("b.sqlite")).unwrap();
    let report = dst.merge_from(&src_path, true).unwrap();
    assert_eq!(report.results, 2);
    assert_eq!(
        dst.search_results_fts("hippoOutputMarker", &FtsFilter::default(), 10)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        dst.search_results_fts("readOutputMarker", &FtsFilter::default(), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(dst.result_page(0, 100).unwrap().len(), 1);
    let queued_results = dst
        .embed_jobs(100)
        .unwrap()
        .into_iter()
        .filter(|j| j.kind == RecordKind::Result)
        .count();
    assert_eq!(queued_results, 1, "hippo 결과는 임베딩 작업도 만들지 않음");
}
