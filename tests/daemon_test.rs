mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use hippo::backends::{VectorSource, vector_source};
use hippo::client::{Launcher, ensure_daemon, launch_disabled, request, run_hook};
use hippo::config::{Config, Paths};
use hippo::daemon::{DaemonError, DaemonHandle, start};
use hippo::embedder::Embedder;
use hippo::ingest::{HookEvent, IngestRequest};
use hippo::protocol::{Request, Response, Scope, StatusInfo};
use hippo::search::SearchKind;
use hippo::spool;
use hippo::store::{RecordKind, RecordStore};
use hippo::testing::{HashEmbedder, MemoryVectorStore};
use hippo::vector::{VectorRecord, VectorStore};
use serde_json::json;

const S2: &str = "afd070e6-4625-4b31-8c43-f1f553949222";
fn s2_cwd() -> String {
    fixture_cwd("s2_tools.jsonl")
}
const TIMEOUT: Duration = Duration::from_secs(10);

fn hook_no_autostart(paths: &Paths, budget: Duration, event: &str, input: &str) -> Option<String> {
    run_hook(paths, budget, event, input, &launch_disabled)
}

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

/// 첫 번째 열기에는 주어진 저장소를, 이후(재색인 세대)에는 새 메모리 저장소를 돌려준다.
fn memory_source(paths: &Paths, initial: Arc<dyn VectorStore>) -> VectorSource {
    let first = Mutex::new(Some(initial));
    let _ = paths;
    VectorSource {
        name: "default".into(),
        open: Arc::new(move |_dir: &Path| {
            let initial = first.lock().unwrap().take();
            Ok(initial.unwrap_or_else(|| Arc::new(MemoryVectorStore::default())))
        }),
    }
}

fn launch(paths: &Paths, vectors: Arc<dyn VectorStore>) -> Result<DaemonHandle, DaemonError> {
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    start(
        paths.clone(),
        config(),
        embedder,
        memory_source(paths, vectors),
    )
}

fn status(paths: &Paths) -> StatusInfo {
    match request(&paths.socket(), &Request::Status, TIMEOUT).unwrap() {
        Response::Status(s) => s,
        other => panic!("{other:?}"),
    }
}

/// 모든 프로젝트를 합친 (반복 수, 임베딩 대기 수, 벡터 수).
fn totals(st: &StatusInfo) -> (u64, u64, u64) {
    st.projects.iter().fold((0, 0, 0), |(i, j, v), p| {
        (i + p.iterations, j + p.embed_jobs, v + p.vectors)
    })
}

/// 프로젝트가 하나뿐일 때 그 id.
fn only_project(paths: &Paths) -> String {
    let st = status(paths);
    assert_eq!(st.projects.len(), 1, "{st:?}");
    st.projects[0].id.clone()
}

fn project_records(paths: &Paths) -> RecordStore {
    RecordStore::open(&paths.project_records(&only_project(paths))).unwrap()
}

fn search_req(query: &str, scope: Scope, cwd: &str) -> Request {
    Request::Search {
        query: query.into(),
        kind: SearchKind::Iteration,
        limit: 5,
        scope,
        cwd: cwd.into(),
    }
}

fn reindex_all() -> Request {
    Request::Reindex {
        scope: Scope::All,
        cwd: String::new(),
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
    assert_eq!(totals(&status(&e.paths)).0, 2);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1);
    assert_eq!(st.projects[0].compatibility, "compatible");
    assert_eq!(
        st.projects[0].location,
        s2_cwd(),
        "존재하지 않는 경로는 그대로 위치"
    );
    assert_eq!(st.embed_model, "test-hash-256");
    assert!(totals(&st).2 >= 4);

    let search = search_req("두 파일은 병렬로", Scope::Project, &s2_cwd());
    let out = text(request(&e.paths.socket(), &search, TIMEOUT).unwrap());
    assert!(out.contains("[iteration]"), "{out}");

    let prompt_id = "8f64252a-0037-484d-a451-35aa2c0076ed";
    let shown = text(
        request(
            &e.paths.socket(),
            &Request::Show {
                prompt_id: prompt_id.into(),
                with_results: false,
                scope: None,
                cwd: s2_cwd(),
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
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let embed_text = project_records(&e.paths)
        .iterations_for_prompt("8f64252a-0037-484d-a451-35aa2c0076ed")
        .unwrap()[1]
        .embed_text
        .clone();

    let input = json!({"session_id": "other", "transcript_path": "/t", "cwd": s2_cwd(), "prompt": embed_text}).to_string();
    let out = hook_no_autostart(
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
        json!({"session_id": S2, "transcript_path": "/t", "cwd": s2_cwd(), "prompt": embed_text})
            .to_string();
    assert_eq!(
        hook_no_autostart(
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
    let input = json!({"session_id": S2, "transcript_path": e.transcript, "cwd": s2_cwd(), "hook_event_name": "Stop"}).to_string();
    assert_eq!(
        hook_no_autostart(&e.paths, Duration::from_millis(800), "stop", &input),
        None
    );
    assert_eq!(totals(&status(&e.paths)).0, 2);
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
    assert!(status(&e.paths).projects.is_empty());
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
    wait_until(|| totals(&status(&e.paths)).0 == 2);
    assert_eq!(spool::pending_count(&e.paths.spool()), 0);
    handle.shutdown();
}

#[test]
fn stale_socket_file_is_replaced() {
    let e = env();
    fs::create_dir_all(&e.paths.root).unwrap();
    fs::write(e.paths.socket(), "stale").unwrap();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert!(status(&e.paths).projects.is_empty());
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
    assert_eq!(st.projects[0].compatibility, "mismatch");
    assert_eq!(
        st.projects[0].embed_jobs, 4,
        "임베딩 작업은 처리하지 않고 남김"
    );
    assert_eq!(st.projects[0].vectors, 1);
    assert!(st.render().contains("hippo reindex"), "{}", st.render());

    let search = search_req("두 파일은 병렬로", Scope::All, "/");
    let out = text(request(&e.paths.socket(), &search, TIMEOUT).unwrap());
    assert!(out.contains("sim=-"), "FTS 전용: {out}");
    let input = json!({"session_id": "other", "transcript_path": "/t", "cwd": s2_cwd(), "prompt": "두 파일은 병렬로 다 읽었지만"}).to_string();
    assert_eq!(
        hook_no_autostart(
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

// ---- FR-SWP-5, 9, 10 ----

fn wait_reindex(paths: &Paths) -> StatusInfo {
    let mut last = status(paths);
    wait_until(|| {
        last = status(paths);
        last.reindex != "running" && last.reindex != "idle"
    });
    last
}

fn mismatched_store() -> MemoryVectorStore {
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
    vectors
}

#[test]
fn reindex_recovers_from_mismatch() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(mismatched_store())).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    assert_eq!(status(&e.paths).projects[0].compatibility, "mismatch");
    assert_eq!(
        request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap(),
        Response::Done
    );
    let done = wait_reindex(&e.paths);
    assert_eq!(done.reindex, "done");
    assert_eq!(done.projects[0].compatibility, "compatible");
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    assert_eq!(
        totals(&status(&e.paths)).2,
        4,
        "반복 2 + 결과 2, 이전 모델 벡터 없음"
    );

    let embed_text = project_records(&e.paths)
        .iterations_for_prompt("8f64252a-0037-484d-a451-35aa2c0076ed")
        .unwrap()[1]
        .embed_text
        .clone();
    let input = json!({"session_id": "other", "transcript_path": "/t", "cwd": s2_cwd(), "prompt": embed_text}).to_string();
    assert!(
        hook_no_autostart(
            &e.paths,
            Duration::from_secs(5),
            "user-prompt-submit",
            &input
        )
        .is_some()
    );
    handle.shutdown();
}

struct SlowEmbedder(HashEmbedder);

impl Embedder for SlowEmbedder {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.0.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        std::thread::sleep(Duration::from_millis(150));
        self.0.embed(texts)
    }
}

#[test]
fn concurrent_reindex_is_rejected() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(SlowEmbedder(HashEmbedder::new(256, 8_000)));
    let handle = start(
        e.paths.clone(),
        config(),
        embedder,
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    assert_eq!(
        request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap(),
        Response::Done
    );
    assert!(matches!(
        request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap(),
        Response::Error { .. }
    ));
    assert_eq!(wait_reindex(&e.paths).reindex, "done");
    handle.shutdown();
}

#[test]
fn lancedb_reindex_switches_generation_and_survives_restart() {
    let e = env();
    let config = Config::parse(
        "[embedder]\nbackend = \"hash\"\ndimension = 256\n[vector]\nbackend = \"lancedb\"\n",
    )
    .unwrap();
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    let handle = start(
        e.paths.clone(),
        config.clone(),
        embedder.clone(),
        vector_source(&config).unwrap(),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let base = e.paths.project_vectors(&only_project(&e.paths), "default");
    let before = fs::read_to_string(base.join("ACTIVE")).unwrap();
    request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap();
    assert_eq!(wait_reindex(&e.paths).reindex, "done");
    let after = fs::read_to_string(base.join("ACTIVE")).unwrap();
    assert_ne!(before, after);
    assert_eq!(totals(&status(&e.paths)).2, 4);
    handle.shutdown();

    let handle = start(
        e.paths.clone(),
        config.clone(),
        embedder,
        vector_source(&config).unwrap(),
    )
    .unwrap();
    let st = status(&e.paths);
    assert_eq!(
        (
            st.projects[0].vectors,
            st.projects[0].compatibility.as_str()
        ),
        (4, "compatible")
    );
    let generations = fs::read_dir(&base)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|d| d.path().is_dir())
        .count();
    assert_eq!(generations, 1, "시작할 때 이전 세대 정리");
    handle.shutdown();
}

// ---- FR-DMN-7, FR-DMN-8 ----

fn config_with(extra: &str) -> Config {
    Config::parse(&format!(
        "[embedder]\nbackend = \"hash\"\ndimension = 256\n[vector]\nbackend = \"memory\"\n{extra}"
    ))
    .unwrap()
}

/// 데몬을 같은 프로세스 안에서 시작하는 launcher. 이미 실행 중이면 성공으로 본다.
fn in_process_launcher(
    slot: &Mutex<Option<DaemonHandle>>,
) -> impl Fn(&Paths) -> std::io::Result<()> + '_ {
    move |paths: &Paths| match launch(paths, Arc::new(MemoryVectorStore::default())) {
        Ok(handle) => {
            *slot.lock().unwrap() = Some(handle);
            Ok(())
        }
        Err(DaemonError::AlreadyRunning) => Ok(()),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    }
}

#[test]
fn hook_autostarts_daemon_and_ingests() {
    let e = env();
    let slot = Mutex::new(None);
    let launcher = in_process_launcher(&slot);
    let input =
        json!({"session_id": S2, "transcript_path": e.transcript, "cwd": s2_cwd()}).to_string();
    assert_eq!(
        run_hook(
            &e.paths,
            Duration::from_millis(800),
            "stop",
            &input,
            &launcher
        ),
        None
    );
    assert!(slot.lock().unwrap().is_some(), "데몬을 띄움");
    assert_eq!(totals(&status(&e.paths)).0, 2);
    assert_eq!(spool::pending_count(&e.paths.spool()), 0);
    assert!(
        ensure_daemon(&e.paths, &launch_disabled, Duration::from_millis(100)),
        "이미 실행 중이면 바로 true"
    );
    slot.lock().unwrap().take().unwrap().shutdown();
}

#[test]
fn autostart_failure_falls_back_to_spool() {
    let e = env();
    let failing: Launcher = &|_: &Paths| Err(std::io::Error::other("cannot start"));
    let input =
        json!({"session_id": S2, "transcript_path": e.transcript, "cwd": s2_cwd()}).to_string();
    assert_eq!(
        run_hook(
            &e.paths,
            Duration::from_millis(800),
            "stop",
            &input,
            failing
        ),
        None
    );
    assert_eq!(spool::pending_count(&e.paths.spool()), 1);
}

fn exits_within(handle: DaemonHandle, limit: Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        handle.wait();
        let _ = tx.send(());
    });
    rx.recv_timeout(limit).is_ok()
}

#[test]
fn idle_daemon_exits_and_removes_socket() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    let handle = start(
        e.paths.clone(),
        config_with("[daemon]\nidle_timeout_secs = 1\n"),
        embedder,
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    assert!(exits_within(handle, Duration::from_secs(6)));
    assert!(!e.paths.socket().exists());
}

#[test]
fn requests_postpone_idle_exit() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    let handle = start(
        e.paths.clone(),
        config_with("[daemon]\nidle_timeout_secs = 1\n"),
        embedder,
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(250));
        status(&e.paths);
    }
    assert!(exits_within(handle, Duration::from_secs(6)));
}

#[test]
fn zero_idle_timeout_never_exits() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    let handle = start(
        e.paths.clone(),
        config_with("[daemon]\nidle_timeout_secs = 0\n"),
        embedder,
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(status(&e.paths).projects.is_empty());
    handle.shutdown();
}

struct VerySlowEmbedder(HashEmbedder);

impl Embedder for VerySlowEmbedder {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.0.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        std::thread::sleep(Duration::from_millis(600));
        self.0.embed(texts)
    }
}

#[test]
fn reindex_blocks_idle_exit() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(VerySlowEmbedder(HashEmbedder::new(256, 8_000)));
    let handle = start(
        e.paths.clone(),
        config_with("[daemon]\nidle_timeout_secs = 1\n"),
        embedder,
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    assert_eq!(
        request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap(),
        Response::Done
    );
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(
        status(&e.paths).reindex,
        "running",
        "재색인 중에는 유휴 종료하지 않음"
    );
    assert!(exits_within(handle, Duration::from_secs(20)));
}

// ---- FR-RET-8 ----

struct DownEmbedder;

impl Embedder for DownEmbedder {
    fn model_id(&self) -> &str {
        "down"
    }
    fn dimension(&self) -> usize {
        8
    }
    fn max_input_tokens(&self) -> usize {
        8_000
    }
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        Err(hippo::embedder::EmbedError::Unavailable(
            "ollama down".into(),
        ))
    }
}

#[test]
fn search_falls_back_to_fts_when_embedding_fails() {
    let e = env();
    let handle = start(
        e.paths.clone(),
        config(),
        Arc::new(DownEmbedder),
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    let search = search_req("두 파일은 병렬로", Scope::Path(s2_cwd()), "/");
    let out = text(request(&e.paths.socket(), &search, TIMEOUT).unwrap());
    let first = out.lines().next().unwrap();
    assert!(
        first.contains("키워드 검색") && first.contains("ollama down"),
        "{out}"
    );
    assert!(
        out.contains("[iteration]") && out.contains("sim=-"),
        "{out}"
    );
    let input = json!({"session_id": "other", "transcript_path": "/t", "cwd": s2_cwd(), "prompt": "두 파일은 병렬로"}).to_string();
    assert_eq!(
        hook_no_autostart(
            &e.paths,
            Duration::from_secs(2),
            "user-prompt-submit",
            &input
        ),
        None
    );
    handle.shutdown();
}

// ---- SRS 3.2.10 프로젝트 (FR-PRJ), FR-DMN-6, FR-DMN-9 ----

const S2_PROMPT: &str = "8f64252a-0037-484d-a451-35aa2c0076ed";

fn prompt_of(session: &str) -> String {
    format!("p-{session}")
}

/// s2 fixture를 세션 id, prompt id, cwd를 바꿔 쓴다. `later_cwd`가 있으면 첫 cwd 라인 뒤의 cwd를 그 값으로 바꾼다.
fn transcript_at(dir: &Path, session: &str, cwd: &Path, later_cwd: Option<&Path>) -> PathBuf {
    let mut first = true;
    let body: String = read_fixture("s2_tools.jsonl")
        .replace(S2, session)
        .replace(S2_PROMPT, &prompt_of(session))
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if v.get("cwd").is_some() {
                let value = match (first, later_cwd) {
                    (false, Some(later)) => later,
                    _ => cwd,
                };
                v["cwd"] = json!(value.to_string_lossy());
                first = false;
            }
            format!("{v}\n")
        })
        .collect();
    let path = dir.join(format!("{session}.jsonl"));
    fs::write(&path, body).unwrap();
    path
}

fn ingest_session(paths: &Paths, session: &str, transcript: &Path) -> Response {
    let req = Request::Ingest(IngestRequest {
        event: HookEvent::Stop,
        session_id: session.into(),
        transcript_path: transcript.to_path_buf(),
    });
    request(&paths.socket(), &req, TIMEOUT).unwrap()
}

fn mkdir(e: &Env, name: &str) -> PathBuf {
    let p = e._dir.path().join(name);
    fs::create_dir_all(&p).unwrap();
    fs::canonicalize(&p).unwrap()
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn project_at<'a>(st: &'a StatusInfo, location: &Path) -> &'a hippo::protocol::ProjectStatus {
    st.projects
        .iter()
        .find(|p| p.location == s(location))
        .unwrap_or_else(|| panic!("{location:?} not in {st:?}"))
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
}

fn two_project_env() -> (Env, DaemonHandle, PathBuf, PathBuf) {
    let e = env();
    let a = mkdir(&e, "a");
    let b = mkdir(&e, "b");
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let ta = transcript_at(e._dir.path(), "sa", &a, None);
    let tb = transcript_at(e._dir.path(), "sb", &b, None);
    assert_eq!(ingest_session(&e.paths, "sa", &ta), Response::Done);
    assert_eq!(ingest_session(&e.paths, "sb", &tb), Response::Done);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    (e, handle, a, b)
}

#[test]
fn sessions_in_different_projects_are_stored_and_searched_separately() {
    let (e, handle, a, b) = two_project_env();
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 2);
    assert_eq!(project_at(&st, &a).iterations, 2);
    assert_eq!(project_at(&st, &b).iterations, 2);
    let rendered = st.render();
    assert!(rendered.contains(&project_at(&st, &a).id), "{rendered}");

    let q = "두 파일은 병렬로";
    let only_a = text(
        request(
            &e.paths.socket(),
            &search_req(q, Scope::Project, &s(&a)),
            TIMEOUT,
        )
        .unwrap(),
    );
    assert!(
        only_a.contains("sa:") && !only_a.contains("sb:"),
        "{only_a}"
    );
    let only_b = text(
        request(
            &e.paths.socket(),
            &search_req(q, Scope::Path(s(&b)), "/"),
            TIMEOUT,
        )
        .unwrap(),
    );
    assert!(
        only_b.contains("sb:") && !only_b.contains("sa:"),
        "{only_b}"
    );
    let all = text(
        request(
            &e.paths.socket(),
            &search_req(q, Scope::All, &s(&a)),
            TIMEOUT,
        )
        .unwrap(),
    );
    assert!(
        all.contains("sa:") && all.contains("sb:") && all.contains("project="),
        "{all}"
    );
    let none = text(
        request(
            &e.paths.socket(),
            &search_req(q, Scope::Project, "/no/such/project"),
            TIMEOUT,
        )
        .unwrap(),
    );
    assert!(none.contains("프로젝트 없음"), "{none}");

    let embed_text = RecordStore::open(&e.paths.project_records(&project_at(&st, &a).id))
        .unwrap()
        .iterations_for_prompt(&prompt_of("sa"))
        .unwrap()[1]
        .embed_text
        .clone();
    let input = |cwd: &str| {
        json!({"session_id": "new", "transcript_path": "/t", "cwd": cwd, "prompt": embed_text})
            .to_string()
    };
    let injected = hook_no_autostart(
        &e.paths,
        Duration::from_secs(5),
        "user-prompt-submit",
        &input(&s(&a)),
    )
    .unwrap();
    assert!(
        injected.contains(&prompt_of("sa")) && !injected.contains(&prompt_of("sb")),
        "{injected}"
    );
    assert_eq!(
        hook_no_autostart(
            &e.paths,
            Duration::from_secs(5),
            "user-prompt-submit",
            &input("/no/such/project")
        ),
        None,
        "프로젝트가 없으면 주입하지 않음"
    );
    handle.shutdown();
}

#[test]
fn session_stays_in_its_start_project_after_cd() {
    let e = env();
    let a = mkdir(&e, "a");
    let b = mkdir(&e, "b");
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let t = transcript_at(e._dir.path(), "sa", &a, Some(&b));
    assert_eq!(ingest_session(&e.paths, "sa", &t), Response::Done);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1);
    assert_eq!(project_at(&st, &a).iterations, 2);
    handle.shutdown();
}

#[test]
fn worktrees_share_the_main_project_and_moved_repository_is_followed() {
    let e = env();
    let repo = mkdir(&e, "repo");
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    let wt = e._dir.path().join("wt");
    git(&repo, &["worktree", "add", "-q", "-b", "feature", &s(&wt)]);
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let ta = transcript_at(e._dir.path(), "sa", &repo.join("src"), None);
    let tb = transcript_at(e._dir.path(), "sb", &wt, None);
    ingest_session(&e.paths, "sa", &ta);
    ingest_session(&e.paths, "sb", &tb);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1, "{st:?}");
    assert_eq!(project_at(&st, &repo.join(".git")).iterations, 4);

    let moved = e._dir.path().join("moved");
    fs::rename(&repo, &moved).unwrap();
    let moved = fs::canonicalize(&moved).unwrap();
    let tc = transcript_at(e._dir.path(), "sc", &moved, None);
    ingest_session(&e.paths, "sc", &tc);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1, "{st:?}");
    assert_eq!(project_at(&st, &moved.join(".git")).iterations, 6);
    handle.shutdown();
}

fn project_list(paths: &Paths) -> String {
    text(request(&paths.socket(), &Request::ProjectList, TIMEOUT).unwrap())
}

fn adopt(paths: &Paths, source: &str, cwd: &Path) -> Response {
    request(
        &paths.socket(),
        &Request::ProjectAdopt {
            source: source.into(),
            cwd: s(cwd),
        },
        TIMEOUT,
    )
    .unwrap()
}

#[test]
fn project_list_shows_id_location_and_counts() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert!(project_list(&e.paths).contains("프로젝트 없음"));
    let a = mkdir(&e, "a");
    let t = transcript_at(e._dir.path(), "sa", &a, None);
    ingest_session(&e.paths, "sa", &t);
    let id = only_project(&e.paths);
    let list = project_list(&e.paths);
    assert!(
        list.contains(&id) && list.contains(&s(&a)) && list.contains("iterations=2"),
        "{list}"
    );
    assert!(!list.contains("위치 없음"), "{list}");
    fs::remove_dir(&a).unwrap();
    assert!(project_list(&e.paths).contains("위치 없음"));
    handle.shutdown();
}

#[test]
fn adopt_moves_location_when_current_directory_has_no_project() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let a = mkdir(&e, "a");
    ingest_session(
        &e.paths,
        "sa",
        &transcript_at(e._dir.path(), "sa", &a, None),
    );
    let id = only_project(&e.paths);
    let b = e._dir.path().join("b");
    fs::rename(&a, &b).unwrap();
    let b = fs::canonicalize(&b).unwrap();
    assert!(matches!(adopt(&e.paths, &s(&a), &b), Response::Text { .. }));
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1);
    assert_eq!(project_at(&st, &b).id, id);
    ingest_session(
        &e.paths,
        "sb",
        &transcript_at(e._dir.path(), "sb", &b, None),
    );
    let st = status(&e.paths);
    assert_eq!((st.projects.len(), project_at(&st, &b).iterations), (1, 4));
    handle.shutdown();
}

#[test]
fn adopt_merges_into_current_project_copying_vectors() {
    let (e, handle, a, b) = two_project_env();
    let st = status(&e.paths);
    let source = project_at(&st, &a).id.clone();
    let target = project_at(&st, &b).id.clone();
    let out = text(adopt(&e.paths, &source, &b));
    assert!(out.contains("복사"), "{out}");
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1);
    let p = project_at(&st, &b);
    assert_eq!(
        (p.id.as_str(), p.iterations, p.embed_jobs, p.vectors),
        (target.as_str(), 4, 0, 6),
        "반복 4 + 결과 2(같은 원문은 해시로 합쳐짐)"
    );
    assert!(!e.paths.project_dir(&source).exists(), "원본 디렉터리 삭제");

    ingest_session(&e.paths, "sa", &e._dir.path().join("sa.jsonl"));
    assert_eq!(
        status(&e.paths).projects.len(),
        1,
        "원본 세션은 대상 프로젝트로 감"
    );
    let found = text(
        request(
            &e.paths.socket(),
            &search_req("두 파일은 병렬로", Scope::Project, &s(&b)),
            TIMEOUT,
        )
        .unwrap(),
    );
    assert!(found.contains("sa:") && found.contains("sb:"), "{found}");
    handle.shutdown();
}

#[test]
fn adopt_merge_reembeds_when_source_vectors_are_incompatible() {
    let e = env();
    let a = mkdir(&e, "a");
    let b = mkdir(&e, "b");
    let handle = launch(&e.paths, Arc::new(mismatched_store())).unwrap();
    ingest_session(
        &e.paths,
        "sa",
        &transcript_at(e._dir.path(), "sa", &a, None),
    );
    ingest_session(
        &e.paths,
        "sb",
        &transcript_at(e._dir.path(), "sb", &b, None),
    );
    wait_until(|| project_at(&status(&e.paths), &b).embed_jobs == 0);
    let source = project_at(&status(&e.paths), &a).id.clone();
    assert_eq!(project_at(&status(&e.paths), &a).compatibility, "mismatch");
    let out = text(adopt(&e.paths, &source, &b));
    assert!(out.contains("임베딩 작업"), "{out}");
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1);
    assert_eq!(
        project_at(&st, &b).vectors,
        6,
        "반복 4 + 결과 2(같은 원문은 해시로 합쳐짐). 이전 모델 벡터는 옮기지 않고 다시 임베딩"
    );
    handle.shutdown();
}

#[test]
fn adopt_same_project_or_unknown_source() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let a = mkdir(&e, "a");
    ingest_session(
        &e.paths,
        "sa",
        &transcript_at(e._dir.path(), "sa", &a, None),
    );
    assert!(text(adopt(&e.paths, &s(&a), &a)).contains("변경 없음"));
    assert!(matches!(
        adopt(&e.paths, "/no/such/source", &a),
        Response::Error { .. }
    ));
    assert_eq!(status(&e.paths).projects.len(), 1);
    handle.shutdown();
}

fn show(paths: &Paths, prompt: &str, scope: Option<Scope>, cwd: &str) -> String {
    text(
        request(
            &paths.socket(),
            &Request::Show {
                prompt_id: prompt.into(),
                with_results: false,
                scope,
                cwd: cwd.into(),
            },
            TIMEOUT,
        )
        .unwrap(),
    )
}

#[test]
fn show_looks_in_current_project_first_then_others() {
    let (e, handle, a, b) = two_project_env();
    let p = prompt_of("sa");
    assert!(
        show(&e.paths, &p, None, &s(&b)).contains("[1]"),
        "다른 프로젝트에서도 찾음"
    );
    assert!(show(&e.paths, &p, Some(Scope::Project), &s(&b)).contains("기록 없음"));
    assert!(show(&e.paths, &p, Some(Scope::Path(s(&a))), "/").contains("[1]"));
    handle.shutdown();
}

#[test]
fn reindex_scope_selects_projects() {
    let (e, handle, a, b) = two_project_env();
    let st = status(&e.paths);
    let active = |id: &str| {
        fs::read_to_string(e.paths.project_vectors(id, "default").join("ACTIVE")).unwrap()
    };
    let (ia, ib) = (
        project_at(&st, &a).id.clone(),
        project_at(&st, &b).id.clone(),
    );
    let (a0, b0) = (active(&ia), active(&ib));
    let one = Request::Reindex {
        scope: Scope::Path(s(&a)),
        cwd: "/".into(),
    };
    assert_eq!(
        request(&e.paths.socket(), &one, TIMEOUT).unwrap(),
        Response::Done
    );
    assert_eq!(wait_reindex(&e.paths).reindex, "done");
    let (a1, b1) = (active(&ia), active(&ib));
    assert_ne!(a1, a0);
    assert_eq!(b1, b0);
    assert_eq!(
        request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap(),
        Response::Done
    );
    assert_eq!(wait_reindex(&e.paths).reindex, "done");
    assert_ne!(active(&ia), a1);
    assert_ne!(active(&ib), b1);
    let unknown = Request::Reindex {
        scope: Scope::Project,
        cwd: "/no/such/project".into(),
    };
    assert!(matches!(
        request(&e.paths.socket(), &unknown, TIMEOUT).unwrap(),
        Response::Error { .. }
    ));
    handle.shutdown();
}

fn counting_source(opens: Arc<std::sync::atomic::AtomicUsize>) -> VectorSource {
    VectorSource {
        name: "default".into(),
        open: Arc::new(move |_dir: &Path| {
            opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Arc::new(MemoryVectorStore::default()) as Arc<dyn VectorStore>)
        }),
    }
}

fn reopen_count(max_open: usize) -> usize {
    let e = env();
    let a = mkdir(&e, "a");
    let b = mkdir(&e, "b");
    let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handle = start(
        e.paths.clone(),
        config_with(&format!("[daemon]\nmax_open_projects = {max_open}\n")),
        Arc::new(HashEmbedder::new(256, 8_000)),
        counting_source(Arc::clone(&opens)),
    )
    .unwrap();
    ingest_session(
        &e.paths,
        "sa",
        &transcript_at(e._dir.path(), "sa", &a, None),
    );
    ingest_session(
        &e.paths,
        "sb",
        &transcript_at(e._dir.path(), "sb", &b, None),
    );
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let before = opens.load(std::sync::atomic::Ordering::SeqCst);
    for dir in [&a, &b, &a, &b] {
        text(
            request(
                &e.paths.socket(),
                &search_req("병렬로", Scope::Project, &s(dir)),
                TIMEOUT,
            )
            .unwrap(),
        );
    }
    let reopened = opens.load(std::sync::atomic::Ordering::SeqCst) - before;
    handle.shutdown();
    reopened
}

#[test]
fn open_projects_are_limited_and_least_recently_used_is_closed() {
    assert_eq!(reopen_count(8), 0, "상한 안이면 다시 열지 않음");
    assert!(
        reopen_count(1) >= 3,
        "상한 1이면 프로젝트를 바꿀 때마다 다시 엶"
    );
}

struct RecordingEmbedder(HashEmbedder, Arc<Mutex<Vec<String>>>);

impl Embedder for RecordingEmbedder {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.0.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        self.1.lock().unwrap().extend(texts.iter().cloned());
        self.0.embed(texts)
    }
}

fn marked_iteration(session: &str, marker: &str) -> hippo::store::NewIteration {
    let iteration = hippo::transcript::Iteration {
        session_id: session.into(),
        prompt_id: Some(prompt_of(session)),
        message_id: "m1".into(),
        iteration_index: 0,
        cwd: "/x".into(),
        ts: "2026-11-02T00:00:00Z".into(),
        inputs: vec![],
        texts: vec![marker.into()],
        tool_calls: vec![],
    };
    let embed_text = hippo::embed_text::build_embed_text(&iteration, 500);
    hippo::store::NewIteration {
        iteration,
        embed_text,
    }
}

#[test]
fn startup_queue_orders_jobs_across_projects_by_enqueue_time() {
    use hippo::project::{Location, LocationKind};
    let e = env();
    let dir_loc = |p: &str| Location {
        path: p.into(),
        kind: LocationKind::Dir,
        toplevel: None,
        root_commits: vec![],
    };
    let mut registry = hippo::registry::Registry::open(&e.paths.registry()).unwrap();
    let ia = registry.resolve(&dir_loc("/gone/a"), 2).unwrap();
    let ib = registry.resolve(&dir_loc("/gone/b"), 1).unwrap();
    drop(registry);
    for (id, session, marker) in [(&ib, "sb", "MARKER_B"), (&ia, "sa", "MARKER_A")] {
        RecordStore::open(&e.paths.project_records(id))
            .unwrap()
            .apply(&hippo::store::Batch {
                session_id: session.into(),
                transcript_path: "/t".into(),
                iterations: vec![marked_iteration(session, marker)],
                completions: vec![],
                state: hippo::transcript::ExtractorState::default(),
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let handle = start(
        e.paths.clone(),
        config(),
        Arc::new(RecordingEmbedder(
            HashEmbedder::new(256, 8_000),
            Arc::clone(&seen),
        )),
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let seen = seen.lock().unwrap().clone();
    let pos = |m: &str| seen.iter().position(|t| t.contains(m)).unwrap();
    assert!(pos("MARKER_B") < pos("MARKER_A"), "{seen:?}");
    handle.shutdown();
}

#[test]
fn project_list_escapes_control_characters() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let odd = mkdir(&e, "odd\u{1b}[31m");
    ingest_session(
        &e.paths,
        "sa",
        &transcript_at(e._dir.path(), "sa", &odd, None),
    );
    let list = project_list(&e.paths);
    assert!(
        !list.contains('\u{1b}') && list.contains("\\u{1b}"),
        "{list:?}"
    );
    handle.shutdown();
}
