use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use smem::backends::{BackendError, build_embedder, vector_source};
use smem::client::{HookAction, inject_output, parse_hook, run_hook};
use smem::config::{Config, Paths};
use smem::generations::active_dir;
use smem::ingest::{HookEvent, IngestRequest};
use smem::protocol::{Request, Response, StatusInfo};
use smem::search::{SearchKind, SearchRequest};
use smem::setup::{hooks_config, launchd_plist};
use smem::spool;

fn probe(path: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("probe")
            .join(path),
    )
    .unwrap()
}

const S2_SESSION: &str = "afd070e6-4625-4b31-8c43-f1f553949222";

fn ingest_req(event: HookEvent) -> IngestRequest {
    IngestRequest {
        event,
        session_id: "s".into(),
        transcript_path: PathBuf::from("/t/s.jsonl"),
    }
}

// ---- IF-HK-1, FR-CAP-1 ----

#[test]
fn parses_real_capture_hook_inputs() {
    let batch = probe("r_s2_tools/1793582115945504000_PostToolBatch.json");
    match parse_hook("post-tool-batch", &batch) {
        HookAction::Ingest(r) => {
            assert_eq!(r.event, HookEvent::PostToolBatch);
            assert_eq!(r.session_id, S2_SESSION);
            assert!(
                r.transcript_path
                    .to_string_lossy()
                    .ends_with(&format!("{S2_SESSION}.jsonl"))
            );
        }
        other => panic!("{other:?}"),
    }
    let stop = probe("r_s2_tools/1793582122364771000_Stop.json");
    assert!(
        matches!(parse_hook("stop", &stop), HookAction::Ingest(r) if r.event == HookEvent::Stop)
    );
    assert!(
        matches!(parse_hook("stop-failure", &stop), HookAction::Ingest(r) if r.event == HookEvent::StopFailure)
    );
    assert!(
        matches!(parse_hook("session-end", &stop), HookAction::Ingest(r) if r.event == HookEvent::SessionEnd)
    );
}

#[test]
fn skips_subagent_hooks() {
    let sub = probe("r_s3_fg_agent/1793582132385659000_PostToolBatch.json");
    assert!(sub.contains("agent_id"));
    assert_eq!(parse_hook("post-tool-batch", &sub), HookAction::Skip);
}

#[test]
fn parses_user_prompt_submit() {
    let input = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "transcript_path": "/t", "cwd": "/w", "prompt": "로그인 버그"}).to_string();
    assert_eq!(
        parse_hook("user-prompt-submit", &input),
        HookAction::Inject {
            prompt: "로그인 버그".into(),
            cwd: "/w".into(),
            session_id: "s".into()
        }
    );
}

#[test]
fn invalid_input_or_event_is_skipped() {
    assert_eq!(parse_hook("stop", "not json"), HookAction::Skip);
    assert_eq!(parse_hook("stop", "{}"), HookAction::Skip, "필수 필드 없음");
    assert_eq!(
        parse_hook(
            "pre-tool-use",
            &probe("r_s2_tools/1793582122364771000_Stop.json")
        ),
        HookAction::Skip
    );
}

// ---- IF-HK-3 ----

#[test]
fn inject_output_is_hook_json() {
    let out: Value = serde_json::from_str(&inject_output("기억")).unwrap();
    assert_eq!(
        out["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert_eq!(out["hookSpecificOutput"]["additionalContext"], "기억");
}

// ---- IF-SK-2 ----

#[test]
fn protocol_roundtrip() {
    let requests = [
        Request::Ingest(ingest_req(HookEvent::Stop)),
        Request::Inject {
            prompt: "p".into(),
            cwd: "/w".into(),
            session_id: "s".into(),
        },
        Request::Search(SearchRequest {
            query: "q".into(),
            kind: SearchKind::Both,
            cwd: None,
            exclude_session: None,
            limit: 5,
        }),
        Request::Show {
            prompt_id: "p".into(),
            with_results: true,
        },
        Request::Status,
    ];
    for r in requests {
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
    }
    let status = Response::Status(StatusInfo {
        iterations: 1,
        results: 2,
        embed_jobs: 3,
        spool_pending: 0,
        vectors: 4,
        embed_model: "m".into(),
        compatibility: "compatible".into(),
        reindex: "idle".into(),
    });
    for r in [
        Response::Done,
        Response::Text { text: "t".into() },
        Response::Inject { context: None },
        status,
        Response::Error {
            message: "e".into(),
        },
    ] {
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&r).unwrap()).unwrap(),
            r
        );
    }
}

// ---- FR-CAP-5, FR-CAP-6 ----

#[test]
fn spool_preserves_order_and_isolates_new_appends() {
    let dir = tempfile::tempdir().unwrap();
    spool::append(dir.path(), &ingest_req(HookEvent::PostToolBatch)).unwrap();
    spool::append(dir.path(), &ingest_req(HookEvent::Stop)).unwrap();
    assert_eq!(spool::pending_count(dir.path()), 2);

    let taken = spool::take_all(dir.path()).unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(
        taken[0].1.iter().map(|r| r.event).collect::<Vec<_>>(),
        [HookEvent::PostToolBatch, HookEvent::Stop]
    );

    spool::append(dir.path(), &ingest_req(HookEvent::SessionEnd)).unwrap();
    spool::finish(&taken[0].0).unwrap();
    let again = spool::take_all(dir.path()).unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].1[0].event, HookEvent::SessionEnd);
    spool::finish(&again[0].0).unwrap();
    assert_eq!(spool::pending_count(dir.path()), 0);
    assert!(spool::take_all(dir.path()).unwrap().is_empty());
}

#[test]
fn spool_returns_leftover_files_first_and_skips_bad_lines() {
    let dir = tempfile::tempdir().unwrap();
    spool::append(dir.path(), &ingest_req(HookEvent::PostToolBatch)).unwrap();
    let first = spool::take_all(dir.path()).unwrap();
    spool::append(dir.path(), &ingest_req(HookEvent::Stop)).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join("pending.jsonl"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"garbage\n"))
        .unwrap();
    let all = spool::take_all(dir.path()).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].0, first[0].0, "처리하지 못한 이전 파일이 먼저");
    assert_eq!(all[1].1.len(), 1, "해석할 수 없는 줄은 건너뜀");
}

#[cfg(unix)]
#[test]
fn spool_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    spool::append(dir.path(), &ingest_req(HookEvent::Stop)).unwrap();
    let mode = fs::metadata(dir.path().join("pending.jsonl"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn spool_missing_dir_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/spool");
    spool::append(&nested, &ingest_req(HookEvent::Stop)).unwrap();
    assert_eq!(spool::pending_count(&nested), 1);
    assert_eq!(spool::pending_count(&dir.path().join("none")), 0);
}

// ---- IF-HK-5, QA-5: 데몬이 없을 때 ----

#[test]
fn hook_without_daemon_spools_ingest_and_skips_inject() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let paths = Paths::new(dir.path().to_path_buf());
    let stop = probe("r_s2_tools/1793582122364771000_Stop.json");
    assert_eq!(
        run_hook(&paths, Duration::from_millis(800), "stop", &stop),
        None
    );
    assert_eq!(spool::pending_count(&paths.spool()), 1);

    let input =
        json!({"session_id": "s", "transcript_path": "/t", "cwd": "/w", "prompt": "q"}).to_string();
    let started = Instant::now();
    assert_eq!(
        run_hook(
            &paths,
            Duration::from_millis(800),
            "user-prompt-submit",
            &input
        ),
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        run_hook(
            &paths,
            Duration::from_millis(800),
            "post-tool-batch",
            "garbage"
        ),
        None
    );
    assert_eq!(spool::pending_count(&paths.spool()), 1);
}

// ---- IF-HK-4, IF-CLI-4 ----

#[test]
fn hooks_config_registers_all_events() {
    let exe = Path::new("/opt/bin/smem");
    let cfg = hooks_config(exe, 5);
    let hooks = &cfg["hooks"];
    for (event, arg) in [
        ("PostToolBatch", "post-tool-batch"),
        ("Stop", "stop"),
        ("StopFailure", "stop-failure"),
        ("SessionEnd", "session-end"),
    ] {
        let h = &hooks[event][0]["hooks"][0];
        assert_eq!(h["type"], "command");
        assert_eq!(h["command"], format!("/opt/bin/smem hook {arg}"));
        assert_eq!(h["async"], true, "{event}");
    }
    let ups = &hooks["UserPromptSubmit"][0]["hooks"][0];
    assert_eq!(ups["command"], "/opt/bin/smem hook user-prompt-submit");
    assert!(ups.get("async").is_none_or(|v| v == false));
    assert_eq!(ups["timeout"], 5);
}

#[test]
fn launchd_plist_runs_daemon() {
    let plist = launchd_plist(Path::new("/opt/bin/smem"), &Paths::new(PathBuf::from("/r")));
    assert!(plist.starts_with("<?xml"));
    assert!(plist.contains("<string>/opt/bin/smem</string>"));
    assert!(plist.contains("<string>daemon</string>"));
    assert!(plist.contains("/r/logs/"));
    assert!(plist.contains("<key>KeepAlive</key>"));
}

// ---- FR-SWP-3 ----

#[test]
fn backend_selection() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path().to_path_buf());
    let mut config = Config::parse(
        "[embedder]\nbackend = \"hash\"\ndimension = 16\n[vector]\nbackend = \"memory\"\n",
    )
    .unwrap();
    assert_eq!(build_embedder(&config).unwrap().dimension(), 16);
    let memory = vector_source(&config, &paths).unwrap();
    let store = (memory.open)(&active_dir(&memory.base).unwrap()).unwrap();
    assert_eq!(store.count().unwrap(), 0);

    let real = Config::default();
    assert_eq!(
        build_embedder(&real).unwrap().model_id(),
        "qwen3-embedding:0.6b"
    );
    let lance = vector_source(&real, &paths).unwrap();
    assert_eq!(lance.base, paths.vectors("default"));
    let generation = active_dir(&lance.base).unwrap();
    assert_eq!((lance.open)(&generation).unwrap().count().unwrap(), 0);
    assert!(generation.starts_with(paths.vectors("default")));

    config.embedder.backend = "nope".into();
    assert!(matches!(
        build_embedder(&config),
        Err(BackendError::Unknown(_))
    ));
    config.vector.backend = "nope".into();
    assert!(matches!(
        vector_source(&config, &paths),
        Err(BackendError::Unknown(_))
    ));
}

// ---- FR-SWP-10 ----

fn status_info(iterations: u64, vectors: u64, compatibility: &str, reindex: &str) -> StatusInfo {
    StatusInfo {
        iterations,
        results: 0,
        embed_jobs: 0,
        spool_pending: 0,
        vectors,
        embed_model: "m".into(),
        compatibility: compatibility.into(),
        reindex: reindex.into(),
    }
}

#[test]
fn status_render_shows_fields_and_reindex_hints() {
    let ok = status_info(3, 5, "compatible", "idle").render();
    assert!(ok.contains("iterations: 3") && ok.contains("reindex: idle"));
    assert!(!ok.contains("smem reindex"));
    assert!(
        status_info(3, 0, "empty", "idle")
            .render()
            .contains("smem reindex"),
        "벡터 0개 + 반복 있음"
    );
    assert!(
        status_info(3, 5, "mismatch", "idle")
            .render()
            .contains("smem reindex")
    );
    assert!(
        !status_info(3, 5, "mismatch", "running")
            .render()
            .contains("smem reindex"),
        "진행 중이면 안내 생략"
    );
    assert!(
        !status_info(0, 0, "empty", "idle")
            .render()
            .contains("smem reindex")
    );
}
