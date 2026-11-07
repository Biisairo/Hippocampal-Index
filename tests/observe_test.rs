//! 관찰 기록: 저장 시각·경로, 주입 기록, 기억 활용 기록, 통계. (FR-OBS-4~8)

use hippo::embed_text::build_embed_text;
use hippo::i18n::Lang;
use hippo::search::render_turn;
use hippo::store::{
    Batch, ForgetTarget, InjectOutcome, InjectionRecord, NewIteration, RecordStore, Source,
};
use hippo::transcript::{ExtractorState, Iteration, IterationInput, ToolCall, ToolResultData};
use rusqlite::Connection;
use serde_json::json;

const DAY_MS: i64 = 86_400_000;
/// 2026-11-07T00:00:00Z
const NOW_MS: i64 = 1_794_009_600_000;

fn iteration(session: &str, mid: &str, prompt: &str, ts: &str, commands: &[&str]) -> NewIteration {
    let it = Iteration {
        session_id: session.into(),
        prompt_id: Some(prompt.into()),
        message_id: mid.into(),
        iteration_index: 0,
        cwd: "/w".into(),
        ts: ts.into(),
        inputs: vec![IterationInput::Prompt {
            text: "질문".into(),
        }],
        texts: vec!["답".into()],
        tool_calls: commands
            .iter()
            .enumerate()
            .map(|(i, c)| ToolCall {
                id: format!("toolu_{mid}_{i}"),
                name: "Bash".into(),
                input: json!({"command": c}),
                result: Some(ToolResultData {
                    text: "out".into(),
                    is_error: false,
                    structured: None,
                }),
            })
            .collect(),
    };
    let embed_text = build_embed_text(&it, 500, &[]);
    NewIteration {
        iteration: it,
        embed_text,
    }
}

fn batch(session: &str, iterations: Vec<NewIteration>, source: Source) -> Batch {
    Batch {
        session_id: session.into(),
        transcript_path: "/t".into(),
        iterations,
        completions: vec![],
        state: ExtractorState::default(),
        bulk: source == Source::Import,
        source,
    }
}

fn injected(session: &str, at_ms: i64, prompt_ids: &[&str]) -> InjectionRecord {
    InjectionRecord {
        session_id: session.into(),
        at_ms,
        outcome: InjectOutcome::Injected,
        reason: None,
        turns: prompt_ids.len(),
        chars: 100,
        elapsed_ms: 12,
        prompt_ids: prompt_ids.iter().map(|p| (*p).to_string()).collect(),
    }
}

fn record(outcome: InjectOutcome, reason: Option<&str>, at_ms: i64) -> InjectionRecord {
    InjectionRecord {
        session_id: "S".into(),
        at_ms,
        outcome,
        reason: reason.map(str::to_string),
        turns: 0,
        chars: 0,
        elapsed_ms: 5,
        prompt_ids: vec![],
    }
}

// ---- FR-OBS-4 ----

#[test]
fn stored_at_and_source_are_saved_per_path() {
    let mut s = RecordStore::open_in_memory().unwrap();
    for (session, source) in [
        ("A", Source::Live),
        ("B", Source::CatchUp),
        ("C", Source::Import),
    ] {
        s.apply(&batch(
            session,
            vec![iteration(
                session,
                "m1",
                &format!("p{session}"),
                "2026-11-06T00:00:00Z",
                &[],
            )],
            source,
        ))
        .unwrap();
    }
    for (session, name) in [("A", "live"), ("B", "catch_up"), ("C", "import")] {
        let it = s.iteration(&format!("{session}:m1")).unwrap().unwrap();
        assert_eq!(it.source.as_deref(), Some(name));
        assert!(it.stored_at.is_some_and(|t| t > 1_700_000_000_000));
    }
}

#[test]
fn old_schema_gets_columns_and_old_rows_stay_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE iterations (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, prompt_id TEXT, message_id TEXT NOT NULL,
                iteration_index INTEGER NOT NULL, cwd TEXT NOT NULL, ts TEXT NOT NULL, inputs TEXT NOT NULL,
                tool_calls TEXT NOT NULL, response_text TEXT NOT NULL, embed_text TEXT NOT NULL);
             INSERT INTO iterations VALUES ('S:m0','S','p0','m0',0,'/w','2026-11-01T00:00:00Z','[]','[]','old','old');",
        )
        .unwrap();
    }
    let mut s = RecordStore::open(&path).unwrap();
    let old = s.iteration("S:m0").unwrap().unwrap();
    assert_eq!((old.stored_at, old.source), (None, None));
    s.apply(&batch(
        "S",
        vec![iteration("S", "m1", "p1", "2026-11-06T00:00:00Z", &[])],
        Source::Live,
    ))
    .unwrap();
    assert_eq!(
        s.iteration("S:m1").unwrap().unwrap().source.as_deref(),
        Some("live")
    );
}

#[test]
fn merge_keeps_stored_at_and_source() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.sqlite");
    {
        let mut src = RecordStore::open(&src_path).unwrap();
        src.apply(&batch(
            "S",
            vec![iteration("S", "m1", "p1", "2026-11-06T00:00:00Z", &[])],
            Source::CatchUp,
        ))
        .unwrap();
    }
    let src_it = RecordStore::open(&src_path)
        .unwrap()
        .iteration("S:m1")
        .unwrap()
        .unwrap();
    let mut dst = RecordStore::open(&dir.path().join("dst.sqlite")).unwrap();
    dst.merge_from(&src_path, false).unwrap();
    let it = dst.iteration("S:m1").unwrap().unwrap();
    assert_eq!(it.source.as_deref(), Some("catch_up"));
    assert_eq!(it.stored_at, src_it.stored_at);
}

#[test]
fn show_header_includes_stored_time_and_source() {
    let mut s = RecordStore::open_in_memory().unwrap();
    s.apply(&batch(
        "S",
        vec![iteration("S", "m1", "p1", "2026-11-06T00:00:00Z", &[])],
        Source::Import,
    ))
    .unwrap();
    let out = render_turn(&s, "p1", false, Lang::En).unwrap();
    let header = out.lines().find(|l| l.starts_with("## [0]")).unwrap();
    assert!(
        header.contains(" stored ") && header.ends_with(" import"),
        "{header}"
    );
}

// ---- FR-OBS-6 ----

#[test]
fn memory_uses_are_classified() {
    let mut s = RecordStore::open_in_memory().unwrap();
    let injected_prompt = "11111111-2222-4333-8444-555555555555";
    let other_prompt = "99999999-2222-4333-8444-555555555555";
    s.record_injection(&injected("S", NOW_MS - 1000, &[injected_prompt]))
        .unwrap();
    s.apply(&batch(
        "S",
        vec![iteration(
            "S",
            "m1",
            "p1",
            "2026-11-06T12:00:00Z",
            &[
                &format!("~/.hippo/bin/hippo show {injected_prompt} --with-results"),
                "hippo search \"lance 버전\"",
                &format!("hippo show {other_prompt}"),
                "ls -la",
                "cargo test -p hippo",
            ],
        )],
        Source::Live,
    ))
    .unwrap();
    let stats = s.inject_stats(NOW_MS).unwrap();
    assert_eq!((stats.opened, stats.search, stats.show), (1, 1, 1));
    // 다른 세션의 주입 기록은 (a)에 쓰지 않는다.
    s.apply(&batch(
        "T",
        vec![iteration(
            "T",
            "m2",
            "p2",
            "2026-11-06T12:00:00Z",
            &[&format!("hippo show {injected_prompt}")],
        )],
        Source::Live,
    ))
    .unwrap();
    let stats = s.inject_stats(NOW_MS).unwrap();
    assert_eq!((stats.opened, stats.show), (1, 2));
    // 같은 반복을 다시 저장해도 늘지 않는다.
    s.apply(&batch(
        "T",
        vec![iteration(
            "T",
            "m2",
            "p2",
            "2026-11-06T12:00:00Z",
            &[&format!("hippo show {injected_prompt}")],
        )],
        Source::Live,
    ))
    .unwrap();
    assert_eq!(s.inject_stats(NOW_MS).unwrap().show, 2);
}

// ---- FR-OBS-5, FR-OBS-7 ----

#[test]
fn inject_stats_count_outcomes_and_reasons_within_seven_days() {
    let mut s = RecordStore::open_in_memory().unwrap();
    s.record_injection(&injected(
        "S",
        NOW_MS - 1000,
        &["aaaaaaaa-2222-4333-8444-555555555555"],
    ))
    .unwrap();
    s.record_injection(&record(InjectOutcome::NoMatch, None, NOW_MS - DAY_MS))
        .unwrap();
    s.record_injection(&record(
        InjectOutcome::Skipped,
        Some("too_short"),
        NOW_MS - 2 * DAY_MS,
    ))
    .unwrap();
    s.record_injection(&record(
        InjectOutcome::Skipped,
        Some("too_short"),
        NOW_MS - 3 * DAY_MS,
    ))
    .unwrap();
    s.record_injection(&record(
        InjectOutcome::Skipped,
        Some("task_notification"),
        NOW_MS,
    ))
    .unwrap();
    s.record_injection(&record(
        InjectOutcome::Timeout,
        None,
        NOW_MS - 7 * DAY_MS + 1000,
    ))
    .unwrap();
    s.record_injection(&record(
        InjectOutcome::Error,
        None,
        NOW_MS - 7 * DAY_MS - 1000,
    ))
    .unwrap();
    s.apply(&batch(
        "S",
        vec![
            iteration(
                "S",
                "new",
                "p1",
                "2026-11-06T00:00:00Z",
                &["hippo search x"],
            ),
            iteration(
                "S",
                "old",
                "p2",
                "2026-10-30T00:00:00Z",
                &["hippo search y"],
            ),
        ],
        Source::Live,
    ))
    .unwrap();
    let st = s.inject_stats(NOW_MS).unwrap();
    assert_eq!(st.requests, 6, "7일 밖의 error는 세지 않음");
    assert_eq!(
        (st.injected, st.no_match, st.skipped, st.timeout, st.error),
        (1, 1, 3, 1, 0)
    );
    assert_eq!(
        st.skip_reasons,
        vec![
            ("task_notification".to_string(), 1),
            ("too_short".to_string(), 2)
        ]
    );
    assert_eq!(st.search, 1, "7일 밖의 활용은 세지 않음");
}

// ---- FR-OBS-8 ----

#[test]
fn forget_removes_injections_and_uses() {
    let mut s = RecordStore::open_in_memory().unwrap();
    s.record_injection(&injected("S", NOW_MS, &["p9"])).unwrap();
    s.record_injection(&injected("T", NOW_MS, &["p9"])).unwrap();
    s.apply(&batch(
        "S",
        vec![iteration(
            "S",
            "m1",
            "p1",
            "2026-11-06T00:00:00Z",
            &["hippo search a"],
        )],
        Source::Live,
    ))
    .unwrap();
    s.apply(&batch(
        "T",
        vec![iteration(
            "T",
            "m2",
            "p2",
            "2026-11-06T00:00:00Z",
            &["hippo search b"],
        )],
        Source::Live,
    ))
    .unwrap();
    s.forget(&ForgetTarget::Prompt("p2".into()), false).unwrap();
    let st = s.inject_stats(NOW_MS).unwrap();
    assert_eq!((st.requests, st.search), (2, 1));
    s.forget(&ForgetTarget::Session("S".into()), false).unwrap();
    let st = s.inject_stats(NOW_MS).unwrap();
    assert_eq!((st.requests, st.search), (1, 0));
}
