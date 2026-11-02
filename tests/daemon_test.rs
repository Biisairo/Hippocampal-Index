mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use serde_json::json;
use smem::client::{request, run_hook};
use smem::config::{Config, Paths};
use smem::daemon::{DaemonError, DaemonHandle, start};
use smem::embedder::Embedder;
use smem::ingest::{HookEvent, IngestRequest};
use smem::protocol::{Request, Response, StatusInfo};
use smem::search::{SearchKind, SearchRequest};
use smem::spool;
use smem::store::{RecordKind, RecordStore};
use smem::testing::{HashEmbedder, MemoryVectorStore};
use smem::vector::{VectorRecord, VectorStore};

const S2: &str = "afd070e6-4625-4b31-8c43-f1f553949222";
const S2_CWD: &str = "/Users/user/personal/session_db/probe";
const TIMEOUT: Duration = Duration::from_secs(10);

struct Env {
    _dir: tempfile::TempDir,
    paths: Paths,
    transcript: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let paths = Paths::new(dir.path().join("root"));
    let transcript = dir.path().join(format!("{S2}.jsonl"));
    fs::write(&transcript, read_fixture("s2_tools.jsonl")).unwrap();
    Env {
        _dir: dir,
        paths,
        transcript,
    }
}

fn config() -> Config {
    Config::parse(
        "[embedder]\nbackend = \"hash\"\ndimension = 256\n[vector]\nbackend = \"memory\"\n",
    )
    .unwrap()
}

fn launch(paths: &Paths, vectors: Arc<dyn VectorStore>) -> Result<DaemonHandle, DaemonError> {
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    start(paths.clone(), config(), embedder, vectors)
}

fn status(paths: &Paths) -> StatusInfo {
    match request(&paths.socket(), &Request::Status, TIMEOUT).unwrap() {
        Response::Status(s) => s,
        other => panic!("{other:?}"),
    }
}

fn wait_until(mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !cond() {
        assert!(Instant::now() < deadline, "시간 초과");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ingest(paths: &Paths, transcript: &Path, event: HookEvent) -> Response {
    let req = Request::Ingest(IngestRequest {
        event,
        session_id: S2.into(),
        transcript_path: transcript.to_path_buf(),
    });
    request(&paths.socket(), &req, TIMEOUT).unwrap()
}

fn text(resp: Response) -> String {
    match resp {
        Response::Text { text } => text,
        other => panic!("{other:?}"),
    }
}

#[test]
fn ingest_embed_search_show_end_to_end() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert_eq!(
        ingest(&e.paths, &e.transcript, HookEvent::Stop),
        Response::Done
    );
    assert_eq!(status(&e.paths).iterations, 2);
    wait_until(|| status(&e.paths).embed_jobs == 0);
    let st = status(&e.paths);
    assert_eq!(st.compatibility, "compatible");
    assert_eq!(st.embed_model, "test-hash-256");
    assert!(st.vectors >= 4);

    let search = Request::Search(SearchRequest {
        query: "두 파일은 병렬로".into(),
        kind: SearchKind::Iteration,
        cwd: Some(S2_CWD.into()),
        exclude_session: None,
        limit: 5,
    });
    let out = text(request(&e.paths.socket(), &search, TIMEOUT).unwrap());
    assert!(out.contains("[iteration]"), "{out}");

    let prompt_id = "8f64252a-0037-484d-a451-35aa2c0076ed";
    let shown = text(
        request(
            &e.paths.socket(),
            &Request::Show {
                prompt_id: prompt_id.into(),
                with_results: false,
            },
            TIMEOUT,
        )
        .unwrap(),
    );
    assert!(shown.contains("[0]") && shown.contains("[1]"));
    handle.shutdown();
}

#[test]
fn inject_returns_context_for_other_session() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| status(&e.paths).embed_jobs == 0);
    let embed_text = RecordStore::open(&e.paths.records())
        .unwrap()
        .iterations_for_prompt("8f64252a-0037-484d-a451-35aa2c0076ed")
        .unwrap()[1]
        .embed_text
        .clone();

    let input = json!({"session_id": "other", "transcript_path": "/t", "cwd": S2_CWD, "prompt": embed_text}).to_string();
    let out = run_hook(
        &e.paths,
        Duration::from_secs(5),
        "user-prompt-submit",
        &input,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("<memory ")
    );

    let same =
        json!({"session_id": S2, "transcript_path": "/t", "cwd": S2_CWD, "prompt": embed_text})
            .to_string();
    assert_eq!(
        run_hook(
            &e.paths,
            Duration::from_secs(5),
            "user-prompt-submit",
            &same
        ),
        None,
        "현재 세션은 제외"
    );
    handle.shutdown();
}

#[test]
fn hook_ingest_through_daemon() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let input = json!({"session_id": S2, "transcript_path": e.transcript, "cwd": S2_CWD, "hook_event_name": "Stop"}).to_string();
    assert_eq!(
        run_hook(&e.paths, Duration::from_millis(800), "stop", &input),
        None
    );
    assert_eq!(status(&e.paths).iterations, 2);
    assert_eq!(spool::pending_count(&e.paths.spool()), 0);
    handle.shutdown();
}

#[test]
fn second_instance_is_rejected_and_lock_released_on_shutdown() {
    let e = env();
    let first = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert!(matches!(
        launch(&e.paths, Arc::new(MemoryVectorStore::default())),
        Err(DaemonError::AlreadyRunning)
    ));
    first.shutdown();
    let again = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert_eq!(status(&e.paths).iterations, 0);
    again.shutdown();
}

#[test]
fn spool_is_drained_on_start() {
    let e = env();
    let req = IngestRequest {
        event: HookEvent::Stop,
        session_id: S2.into(),
        transcript_path: e.transcript.clone(),
    };
    spool::append(&e.paths.spool(), &req).unwrap();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    wait_until(|| status(&e.paths).iterations == 2);
    assert_eq!(spool::pending_count(&e.paths.spool()), 0);
    handle.shutdown();
}

#[test]
fn stale_socket_file_is_replaced() {
    let e = env();
    fs::create_dir_all(&e.paths.root).unwrap();
    fs::write(e.paths.socket(), "stale").unwrap();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert_eq!(status(&e.paths).iterations, 0);
    handle.shutdown();
}

#[cfg(unix)]
#[test]
fn data_root_and_socket_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let e = env();
    fs::create_dir_all(&e.paths.root).unwrap();
    fs::set_permissions(&e.paths.root, fs::Permissions::from_mode(0o755)).unwrap();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&e.paths.root), 0o700);
    assert_eq!(mode(&e.paths.spool()), 0o700);
    assert_eq!(mode(&e.paths.logs()), 0o700);
    assert_eq!(mode(&e.paths.socket()), 0o600);
    handle.shutdown();
}

#[test]
fn incompatible_vector_store_falls_back_to_fts() {
    let e = env();
    let vectors = MemoryVectorStore::default();
    vectors
        .upsert(&[VectorRecord {
            kind: RecordKind::Iteration,
            ref_id: "old".into(),
            chunk_index: 0,
            vector: vec![1.0, 0.0, 0.0],
            embed_model: "other-model".into(),
            cwd: None,
            session_id: None,
            ts: String::new(),
        }])
        .unwrap();
    let handle = launch(&e.paths, Arc::new(vectors)).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    std::thread::sleep(Duration::from_millis(300));
    let st = status(&e.paths);
    assert_eq!(st.compatibility, "mismatch");
    assert_eq!(st.embed_jobs, 4, "임베딩 작업은 처리하지 않고 남김");
    assert_eq!(st.vectors, 1);

    let search = Request::Search(SearchRequest {
        query: "두 파일은 병렬로".into(),
        kind: SearchKind::Iteration,
        cwd: None,
        exclude_session: None,
        limit: 5,
    });
    let out = text(request(&e.paths.socket(), &search, TIMEOUT).unwrap());
    assert!(out.contains("sim=-"), "FTS 전용: {out}");
    let input = json!({"session_id": "other", "transcript_path": "/t", "cwd": S2_CWD, "prompt": "두 파일은 병렬로 다 읽었지만"}).to_string();
    assert_eq!(
        run_hook(
            &e.paths,
            Duration::from_secs(2),
            "user-prompt-submit",
            &input
        ),
        None
    );
    handle.shutdown();
}

#[test]
fn ingest_error_is_reported_and_daemon_keeps_running() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert!(matches!(
        ingest(&e.paths, Path::new("/nonexistent.jsonl"), HookEvent::Stop),
        Response::Error { .. }
    ));
    assert_eq!(
        ingest(&e.paths, &e.transcript, HookEvent::Stop),
        Response::Done
    );
    assert!(
        fs::read_dir(e.paths.logs()).unwrap().count() > 0,
        "오류 로그 기록"
    );
    handle.shutdown();
}
