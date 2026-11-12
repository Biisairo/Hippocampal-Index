use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hippo::backends::{BackendError, build_embedder, vector_source};
use hippo::client::{
    HookAction, hook_paused, inject_output, launch_disabled, parse_hook, request_until_reply,
    run_hook,
};
use hippo::config::{Config, Paths};
use hippo::generations::active_dir;
use hippo::i18n::Lang;
use hippo::import::{Candidate, ImportStatus};
use hippo::ingest::{HookEvent, ImportOutcome, IngestRequest};
use hippo::protocol::{ImportSessionReport, ProjectStatus, Request, Response, Scope, StatusInfo};
use hippo::search::SearchKind;
use hippo::setup::{hooks_config, launchd_plist};
use hippo::spool;
use serde_json::{Value, json};

fn probe(path: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/hooks")
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
        prompt_id: Some("p".into()),
    }
}

// ---- IF-HK-1, FR-CAP-1 ----

fn hook_no_autostart(paths: &Paths, budget: Duration, event: &str, input: &str) -> Option<String> {
    run_hook(paths, budget, budget, event, input, &launch_disabled)
}

#[test]
fn parses_real_capture_hook_inputs() {
    let batch = probe("r_s2_tools/1793582115945504000_PostToolBatch.json");
    match parse_hook("post-tool-batch", &batch) {
        HookAction::Ingest(r) => {
            assert_eq!(r.event, HookEvent::PostToolBatch);
            assert_eq!(r.session_id, S2_SESSION);
            assert_eq!(
                r.prompt_id.as_deref(),
                Some("8f64252a-0037-484d-a451-35aa2c0076ed"),
                "기준 턴은 hook 입력의 prompt_id (FR-CAP-7)"
            );
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
            budget_ms: None,
        },
        Request::Inject {
            prompt: "p".into(),
            cwd: "/w".into(),
            session_id: "s".into(),
            budget_ms: Some(2_500),
        },
        Request::Search {
            query: "q".into(),
            kind: SearchKind::Both,
            limit: 5,
            scope: Scope::Path("/w".into()),
            cwd: "/w".into(),
            structured: false,
        },
        Request::Show {
            prompt_id: "p".into(),
            with_results: true,
            scope: Some(Scope::Project),
            cwd: "/w".into(),
        },
        Request::Show {
            prompt_id: "p".into(),
            with_results: false,
            scope: None,
            cwd: "/w".into(),
        },
        Request::Status,
        Request::Reindex {
            scope: Scope::All,
            cwd: "/w".into(),
        },
        Request::ProjectList,
        Request::ImportPrepare {
            from: "/old".into(),
            to: "/w".into(),
            dry_run: true,
        },
        Request::Shutdown,
        Request::ImportSession {
            project_id: "0123".into(),
            session_id: "s".into(),
            transcript_path: PathBuf::from("/t/s.jsonl"),
        },
        Request::CurrentProject { cwd: "/w".into() },
    ];
    for r in requests {
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
    }
    let status = Response::Status(status_info(1, 4, "compatible", "idle"));
    for r in [
        Response::Done,
        Response::Text { text: "t".into() },
        Response::Inject { context: None },
        status,
        Response::ImportPlan {
            project_id: "0123".into(),
            source_id: Some("4567".into()),
            summary: "변경 없음".into(),
            sessions: vec![Candidate {
                session_id: "s".into(),
                transcript_path: PathBuf::from("/t/s.jsonl"),
                started_at: "2026-10-01T00:00:00Z".into(),
                start_cwd: "/w".into(),
                size: 10,
                first_prompt: "hi".into(),
                status: ImportStatus::LiveOnly,
                entrypoint: "sdk-py".into(),
                prompt_offset: None,
            }],
        },
        Response::ImportSession(ImportSessionReport {
            outcome: ImportOutcome::Imported,
            iterations: 2,
            completions: 0,
            fork_skipped: 1,
            pending_embeds: 3,
        }),
        Response::CurrentProject { id: None },
        Response::CurrentProject {
            id: Some("0123".into()),
        },
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
        hook_no_autostart(&paths, Duration::from_millis(800), "stop", &stop),
        None
    );
    assert_eq!(spool::pending_count(&paths.spool()), 1);

    let input =
        json!({"session_id": "s", "transcript_path": "/t", "cwd": "/w", "prompt": "q"}).to_string();
    let started = Instant::now();
    assert_eq!(
        hook_no_autostart(
            &paths,
            Duration::from_millis(800),
            "user-prompt-submit",
            &input
        ),
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        hook_no_autostart(
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
    let exe = Path::new("/opt/bin/hippo");
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
        assert_eq!(h["command"], format!("/opt/bin/hippo hook {arg}"));
        assert_eq!(h["async"], true, "{event}");
    }
    let ups = &hooks["UserPromptSubmit"][0]["hooks"][0];
    assert_eq!(ups["command"], "/opt/bin/hippo hook user-prompt-submit");
    assert!(ups.get("async").is_none_or(|v| v == false));
    assert_eq!(ups["timeout"], 5);
}

#[test]
fn launchd_plist_runs_daemon() {
    let plist = launchd_plist(
        Path::new("/opt/bin/hippo"),
        &Paths::new(PathBuf::from("/r")),
    );
    assert!(plist.starts_with("<?xml"));
    assert!(plist.contains("<string>/opt/bin/hippo</string>"));
    assert!(plist.contains("<string>daemon</string>"));
    assert!(plist.contains("/r/logs/"));
    assert!(plist.contains("<key>KeepAlive</key>"));
    assert!(
        plist.contains("<key>SuccessfulExit</key>\n    <false/>"),
        "유휴 종료(정상 종료) 뒤에는 재시작하지 않음"
    );
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
    assert_eq!(build_embedder(&config, &paths).unwrap().dimension(), 16);
    let memory = vector_source(&config).unwrap();
    let base = paths.project_vectors("p", &memory.name);
    let store = (memory.open)(&active_dir(&base).unwrap()).unwrap();
    assert_eq!(store.count().unwrap(), 0);

    let real = Config::default();
    assert_eq!(
        build_embedder(&real, &paths).unwrap().model_id(),
        "qwen3-embedding:0.6b"
    );
    let lance = vector_source(&real).unwrap();
    assert_eq!(lance.name, "default");
    let base = paths.project_vectors("p", &lance.name);
    let generation = active_dir(&base).unwrap();
    assert_eq!((lance.open)(&generation).unwrap().count().unwrap(), 0);
    assert!(generation.starts_with(&base));

    config.embedder.backend = "nope".into();
    assert!(matches!(
        build_embedder(&config, &paths),
        Err(BackendError::Unknown(_))
    ));
    config.vector.backend = "nope".into();
    assert!(matches!(
        vector_source(&config),
        Err(BackendError::Unknown(_))
    ));
}

// ---- FR-SWP-10 ----

fn status_info(iterations: u64, vectors: u64, compatibility: &str, reindex: &str) -> StatusInfo {
    StatusInfo {
        spool_pending: 0,
        embed_model: "m".into(),
        reindex: reindex.into(),
        projects: vec![ProjectStatus {
            id: "p1".into(),
            location: "/w".into(),
            exists: true,
            iterations,
            results: 0,
            embed_jobs: 0,
            embed_jobs_import: 0,
            vectors,
            compatibility: compatibility.into(),
            embed_per_min: None,
            inject: Default::default(),
        }],
    }
}

#[test]
fn status_render_shows_embedding_rate_and_remaining_time() {
    let mut st = status_info(3, 5, "compatible", "idle");
    st.projects[0].embed_jobs = 300;
    assert!(
        !st.render(Lang::En).contains("/min"),
        "속도를 모르면 표시하지 않음"
    );
    st.projects[0].embed_per_min = Some(120);
    let en = st.render(Lang::En);
    assert!(en.contains("120/min") && en.contains("3 min left"), "{en}");
    let ko = st.render(Lang::Ko);
    assert!(ko.contains("분당 120") && ko.contains("3분"), "{ko}");
    st.projects[0].embed_jobs = 0;
    assert!(
        !st.render(Lang::En).contains("/min"),
        "대기 건수가 없으면 표시하지 않음"
    );
}

#[test]
fn status_render_shows_fields_and_reindex_hints() {
    let ok = status_info(3, 5, "compatible", "idle").render(Lang::En);
    assert!(ok.contains("iterations: 3") && ok.contains("reindex: idle"));
    assert!(ok.contains("projects: 1") && ok.contains("[p1] /w"), "{ok}");
    assert!(!ok.contains("location missing") && !ok.contains("hippo reindex"));
    let mut gone = status_info(3, 5, "compatible", "idle");
    gone.projects[0].exists = false;
    assert!(gone.render(Lang::En).contains("location missing"));
    assert!(gone.render(Lang::Ko).contains("위치 없음"));
    assert!(
        status_info(3, 0, "empty", "idle")
            .render(Lang::En)
            .contains("hippo reindex"),
        "벡터 0개 + 반복 있음"
    );
    assert!(
        status_info(3, 5, "mismatch", "idle")
            .render(Lang::En)
            .contains("hippo reindex")
    );
    assert!(
        !status_info(3, 5, "mismatch", "running")
            .render(Lang::En)
            .contains("hippo reindex"),
        "진행 중이면 안내 생략"
    );
    assert!(
        !status_info(0, 0, "empty", "idle")
            .render(Lang::En)
            .contains("hippo reindex")
    );
}

// ---- FR-DMN-7 ----

#[test]
fn ensure_daemon_with_disabled_launcher_fails_fast() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let paths = Paths::new(dir.path().to_path_buf());
    let started = Instant::now();
    assert!(!hippo::client::ensure_daemon(
        &paths,
        &launch_disabled,
        Duration::from_secs(2)
    ));
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn ensure_daemon_gives_up_after_wait() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let paths = Paths::new(dir.path().to_path_buf());
    let started = Instant::now();
    assert!(!hippo::client::ensure_daemon(
        &paths,
        &|_: &Paths| Ok(()),
        Duration::from_millis(300)
    ));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(2),
        "{elapsed:?}"
    );
}

// ---- QA-6 ----

#[test]
fn status_render_escapes_control_characters_in_location() {
    let mut st = status_info(1, 1, "compatible", "idle");
    st.projects[0].location = "/w\u{1b}[31m".into();
    let out = st.render(Lang::En);
    assert!(!out.contains('\u{1b}'), "{out:?}");
    assert!(out.contains("\\u{1b}"), "{out:?}");
}

#[test]
fn spool_line_without_prompt_id_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("pending.jsonl"),
        "{\"event\":\"Stop\",\"session_id\":\"s\",\"transcript_path\":\"/t/s.jsonl\"}\n",
    )
    .unwrap();
    let taken = spool::take_all(dir.path()).unwrap();
    assert_eq!(taken[0].1[0].prompt_id, None);
    let line = serde_json::to_string(&ingest_req(HookEvent::Stop)).unwrap();
    assert!(
        line.contains("\"prompt_id\":\"p\""),
        "스풀에 prompt_id 기록 (FR-CAP-5)"
    );
}

#[test]
fn inject_request_without_budget_still_parses() {
    let line = r#"{"type":"inject","prompt":"p","cwd":"/w","session_id":"s"}"#;
    let parsed: Request = serde_json::from_str(line).unwrap();
    assert_eq!(
        parsed,
        Request::Inject {
            prompt: "p".into(),
            cwd: "/w".into(),
            session_id: "s".into(),
            budget_ms: None,
        }
    );
}

#[test]
fn older_import_messages_without_new_fields_still_parse() {
    let req: Request =
        serde_json::from_str(r#"{"type":"import_prepare","from":"/a","to":"/b"}"#).unwrap();
    assert_eq!(
        req,
        Request::ImportPrepare {
            from: "/a".into(),
            to: "/b".into(),
            dry_run: false
        }
    );
    let resp: Response = serde_json::from_str(
        r#"{"type":"import_plan","project_id":"p","summary":"s","sessions":[{"session_id":"x","transcript_path":"/t/x.jsonl"}]}"#,
    )
    .unwrap();
    let Response::ImportPlan {
        source_id,
        sessions,
        ..
    } = resp
    else {
        panic!("{resp:?}");
    };
    assert_eq!(source_id, None);
    assert_eq!(
        (sessions[0].size, sessions[0].status),
        (0, ImportStatus::New)
    );
}

/// 요청 한 줄을 읽은 뒤 `reply`를 보내거나(Some), 응답 없이 연결을 닫는(None) 가짜 데몬.
fn fake_daemon(
    reply: Option<Response>,
) -> (tempfile::TempDir, PathBuf, std::thread::JoinHandle<()>) {
    use std::io::{BufRead, BufReader, Write};
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("hippo.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        if let Some(reply) = reply {
            let mut stream = stream;
            writeln!(stream, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
        }
    });
    (dir, socket, server)
}

#[test]
fn request_until_reply_returns_the_reply() {
    let (_dir, socket, server) = fake_daemon(Some(Response::Done));
    assert_eq!(
        request_until_reply(&socket, &Request::Status).unwrap(),
        Response::Done
    );
    server.join().unwrap();
}

#[test]
fn request_until_reply_fails_when_connection_closes_without_reply() {
    let (_dir, socket, server) = fake_daemon(None);
    assert!(request_until_reply(&socket, &Request::Status).is_err());
    server.join().unwrap();
}

// ---- FR-CAP-8, FR-RET-12 ----

#[test]
fn pause_switches_skip_only_their_hooks() {
    let on = Config::default();
    assert!(on.inject.enabled && on.capture.enabled, "기본은 동작");
    for event in [
        "post-tool-batch",
        "stop",
        "stop-failure",
        "session-end",
        "user-prompt-submit",
    ] {
        assert!(!hook_paused(&on, event), "{event}");
    }
    let no_inject = Config::parse("[inject]\nenabled = false\n").unwrap();
    assert!(hook_paused(&no_inject, "user-prompt-submit"));
    assert!(!hook_paused(&no_inject, "stop"), "저장은 계속");
    let no_capture = Config::parse("[capture]\nenabled = false\n").unwrap();
    for event in ["post-tool-batch", "stop", "stop-failure", "session-end"] {
        assert!(hook_paused(&no_capture, event), "{event}");
    }
    assert!(
        !hook_paused(&no_capture, "user-prompt-submit"),
        "주입은 계속"
    );
}

// ---- IF-CLI-5: 실시간·가져오기 구분 ----

#[test]
fn status_render_splits_live_and_import_jobs() {
    let mut st = status_info(3, 5, "compatible", "idle");
    assert!(
        !st.render(Lang::En).contains("import"),
        "대기가 없으면 구분하지 않음"
    );
    st.projects[0].embed_jobs = 10;
    st.projects[0].embed_jobs_import = 7;
    let en = st.render(Lang::En);
    assert!(en.contains("embed_jobs: 10 [live 3, import 7]"), "{en}");
    let ko = st.render(Lang::Ko);
    assert!(ko.contains("embed_jobs: 10 [실시간 3, 가져오기 7]"), "{ko}");
}
