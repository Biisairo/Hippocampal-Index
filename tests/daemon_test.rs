mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use hippo::backends::{VectorSource, vector_source};
use hippo::client::{
    Launcher, ensure_daemon, launch_disabled, request, request_until_reply, run_hook,
};
use hippo::config::{Config, Paths};
use hippo::daemon::{DaemonError, DaemonHandle, start};
use hippo::embedder::Embedder;
use hippo::i18n::Lang;
use hippo::import::{Candidate, ImportStatus};
use hippo::ingest::{HookEvent, ImportOutcome, IngestRequest};
use hippo::protocol::{ImportSessionReport, Request, Response, Scope, StatusInfo};
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
const RELAUNCH_WAIT: Duration = Duration::from_secs(2);

fn hook_no_autostart(paths: &Paths, budget: Duration, event: &str, input: &str) -> Option<String> {
    run_hook(paths, budget, budget, event, input, &launch_disabled)
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

/// 테스트 데몬이 실제 `~/.claude/projects`를 읽지 않도록 transcripts 디렉터리를 비워 둔다.
/// 벡터 수를 세는 테스트가 청크 분할에 영향받지 않도록 청크 한도를 8,000자로 둔다.
fn config() -> Config {
    Config::parse(
        "[embedder]\nbackend = \"hash\"\ndimension = 256\nquery_prefix = \"\"\n[vector]\nbackend = \"memory\"\n[chunking]\nmax_tokens = 8000\n[import]\ntranscripts_dir = \"/nonexistent/hippo-test-transcripts\"\n",
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
    launch_with(paths, vectors, config())
}

fn launch_with(
    paths: &Paths,
    vectors: Arc<dyn VectorStore>,
    config: Config,
) -> Result<DaemonHandle, DaemonError> {
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    start(
        paths.clone(),
        config,
        embedder,
        memory_source(paths, vectors),
    )
}

/// 같은 루트에서 데몬을 다시 띄운다. 다른 테스트 스레드가 자식 프로세스를 띄우는 동안에는
/// 그 자식이 닫힌 잠금 fd를 exec 전까지 잠깐 쥐므로, `AlreadyRunning`이면 잠시 다시 시도한다.
fn relaunch(
    launch: impl Fn() -> Result<DaemonHandle, DaemonError>,
) -> Result<DaemonHandle, DaemonError> {
    let deadline = Instant::now() + RELAUNCH_WAIT;
    loop {
        match launch() {
            Err(DaemonError::AlreadyRunning) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
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
        structured: false,
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
        prompt_id: Some(S2_PROMPT.into()),
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
    assert!(
        st.projects[0].embed_per_min.is_some_and(|n| n > 0),
        "최근 처리 속도 (IF-CLI-5)"
    );
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

/// 데몬이 설정의 질의 접두사(FR-RET-11)와 최소 길이(FR-RET-1)를 주입 경로에 쓰는지 확인한다.
#[test]
fn inject_uses_configured_query_prefix_and_min_prompt_chars() {
    let injected = |mutate: &dyn Fn(&mut Config)| {
        let e = env();
        let mut c = config();
        mutate(&mut c);
        let handle = launch_with(&e.paths, Arc::new(MemoryVectorStore::default()), c).unwrap();
        ingest(&e.paths, &e.transcript, HookEvent::Stop);
        wait_until(|| totals(&status(&e.paths)).1 == 0);
        let prompt = project_records(&e.paths)
            .iterations_for_prompt(S2_PROMPT)
            .unwrap()[1]
            .embed_text
            .clone();
        let out = inject_with_budget(&e.paths, &prompt, Some(5_000));
        handle.shutdown();
        matches!(out, Response::Inject { context: Some(_) })
    };
    assert!(injected(&|_| {}), "기본 설정에서는 주입");
    let noise: String = (0..400).map(|i| format!("w{i} ")).collect();
    assert!(
        !injected(&|c| c.embedder.query_prefix = noise.clone()),
        "긴 접두사가 질의 임베딩을 바꿔 임계값 아래로 떨어진다"
    );
    assert!(
        !injected(&|c| c.inject.min_prompt_chars = 1_000_000),
        "최소 길이 미만이면 주입하지 않는다"
    );
}

#[test]
fn hook_ingest_through_daemon() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let input = json!({"session_id": S2, "transcript_path": e.transcript, "cwd": s2_cwd(), "prompt_id": S2_PROMPT, "hook_event_name": "Stop"}).to_string();
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
    let again = relaunch(|| launch(&e.paths, Arc::new(MemoryVectorStore::default()))).unwrap();
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
        prompt_id: Some(S2_PROMPT.into()),
    };
    spool::append(&e.paths.spool(), &req).unwrap();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    // 데몬은 스풀 요청을 수집한 뒤에 파일을 지우므로, 두 조건을 함께 기다린다.
    wait_until(|| totals(&status(&e.paths)).0 == 2 && spool::pending_count(&e.paths.spool()) == 0);
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
    fs::write(e.paths.lock(), "").unwrap();
    fs::set_permissions(e.paths.lock(), fs::Permissions::from_mode(0o644)).unwrap();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&e.paths.root), 0o700);
    assert_eq!(mode(&e.paths.spool()), 0o700);
    assert_eq!(mode(&e.paths.logs()), 0o700);
    assert_eq!(mode(&e.paths.socket()), 0o600);
    assert_eq!(
        mode(&e.paths.lock()),
        0o600,
        "이전 실행이 남긴 0644 잠금 파일도 맞춤"
    );
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
    assert!(
        st.render(Lang::En).contains("hippo reindex"),
        "{}",
        st.render(Lang::En)
    );

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
        "[embedder]\nbackend = \"hash\"\ndimension = 256\nquery_prefix = \"\"\n[vector]\nbackend = \"lancedb\"\n[chunking]\nmax_tokens = 8000\n",
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

    let handle = relaunch(|| {
        start(
            e.paths.clone(),
            config.clone(),
            embedder.clone(),
            vector_source(&config).unwrap(),
        )
    })
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
        "[embedder]\nbackend = \"hash\"\ndimension = 256\nquery_prefix = \"\"\n[vector]\nbackend = \"memory\"\n{extra}"
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
        json!({"session_id": S2, "transcript_path": e.transcript, "cwd": s2_cwd(), "prompt_id": S2_PROMPT}).to_string();
    assert_eq!(
        run_hook(
            &e.paths,
            Duration::from_millis(800),
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
        json!({"session_id": S2, "transcript_path": e.transcript, "cwd": s2_cwd(), "prompt_id": S2_PROMPT}).to_string();
    assert_eq!(
        run_hook(
            &e.paths,
            Duration::from_millis(800),
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
        first.contains("keyword results") && first.contains("ollama down"),
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
        prompt_id: Some(prompt_of(session)),
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
    let rendered = st.render(Lang::En);
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
    assert!(none.contains("No project"), "{none}");

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

#[test]
fn project_list_shows_id_location_and_counts() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert!(project_list(&e.paths).contains("No project"));
    let a = mkdir(&e, "a");
    let t = transcript_at(e._dir.path(), "sa", &a, None);
    ingest_session(&e.paths, "sa", &t);
    let id = only_project(&e.paths);
    let list = project_list(&e.paths);
    assert!(
        list.contains(&id) && list.contains(&s(&a)) && list.contains("iterations=2"),
        "{list}"
    );
    assert!(!list.contains("location missing"), "{list}");
    let stamp = list.split("last_active=").nth(1).unwrap().trim();
    assert!(
        stamp.len() == 19
            && stamp.chars().enumerate().all(|(i, c)| match i {
                4 | 7 => c == '-',
                10 => c == ' ',
                13 | 16 => c == ':',
                _ => c.is_ascii_digit(),
            }),
        "로컬 YYYY-MM-DD HH:MM:SS: {stamp}"
    );
    fs::remove_dir(&a).unwrap();
    assert!(project_list(&e.paths).contains("location missing"));
    handle.shutdown();
}

// ---- 가져오기 (FR-PRJ-7, FR-IMP-*) ----

fn import_config(e: &Env) -> Config {
    config_with(&format!(
        "[import]\ntranscripts_dir = \"{}\"\n",
        s(&e._dir.path().join("tx"))
    ))
}

/// `tx/<slug>/<session>.jsonl`에 과거 transcript를 둔다. message.id를 세션마다 달리해 fork 복사본이 되지 않게 한다.
fn past(e: &Env, slug: &str, session: &str, cwd: &Path) -> PathBuf {
    let dir = e._dir.path().join("tx").join(slug);
    fs::create_dir_all(&dir).unwrap();
    let path = transcript_at(&dir, session, cwd, None);
    let body = fs::read_to_string(&path)
        .unwrap()
        .replace("\"id\":\"msg_", &format!("\"id\":\"msg_{session}_"));
    fs::write(&path, body).unwrap();
    path
}

fn prepare(paths: &Paths, from: &str, to: &Path) -> (String, String, Vec<Candidate>) {
    match request(
        &paths.socket(),
        &Request::ImportPrepare {
            from: from.into(),
            to: s(to),
            dry_run: false,
        },
        TIMEOUT,
    )
    .unwrap()
    {
        Response::ImportPlan {
            project_id,
            summary,
            sessions,
            ..
        } => (project_id, summary, sessions),
        other => panic!("{other:?}"),
    }
}

/// CLI와 같이 준비한 뒤 후보마다 `import_session`을 보낸다. (FR-IMP-1)
fn run_import(
    paths: &Paths,
    from: &str,
    to: &Path,
) -> (String, Vec<(String, ImportSessionReport)>) {
    let (project_id, summary, sessions) = prepare(paths, from, to);
    let reports = sessions
        .into_iter()
        .map(|c| {
            let resp = request(
                &paths.socket(),
                &Request::ImportSession {
                    project_id: project_id.clone(),
                    session_id: c.session_id.clone(),
                    transcript_path: c.transcript_path,
                },
                TIMEOUT,
            )
            .unwrap();
            match resp {
                Response::ImportSession(r) => (c.session_id, r),
                other => panic!("{other:?}"),
            }
        })
        .collect();
    (summary, reports)
}

fn outcomes(reports: &[(String, ImportSessionReport)]) -> Vec<(&str, ImportOutcome, usize)> {
    reports
        .iter()
        .map(|(id, r)| (id.as_str(), r.outcome, r.iterations))
        .collect()
}

#[test]
fn import_moves_source_location_when_target_has_no_project() {
    let e = env();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        import_config(&e),
    )
    .unwrap();
    let a = mkdir(&e, "a");
    ingest_session(&e.paths, "sa", &past(&e, "slug-a", "sa", &a));
    let id = only_project(&e.paths);
    let b = e._dir.path().join("b");
    fs::rename(&a, &b).unwrap();
    let b = fs::canonicalize(&b).unwrap();
    let (summary, reports) = run_import(&e.paths, &s(&a), &b);
    assert!(summary.contains("Moved project"), "{summary}");
    assert_eq!(
        outcomes(&reports),
        vec![("sa", ImportOutcome::Unchanged, 0)]
    );
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
fn import_merges_source_project_copying_vectors() {
    let (e, handle, a, b) = two_project_env();
    let st = status(&e.paths);
    let source = project_at(&st, &a).id.clone();
    let target = project_at(&st, &b).id.clone();
    let (summary, reports) = run_import(&e.paths, &source, &b);
    assert!(summary.contains("Copied"), "{summary}");
    assert!(reports.is_empty());
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
    handle.shutdown();
}

#[test]
fn import_merge_reembeds_when_source_vectors_are_incompatible() {
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
    assert_eq!(project_at(&status(&e.paths), &a).compatibility, "mismatch");
    let (summary, _) = run_import(&e.paths, &s(&a), &b);
    assert!(summary.contains("embedding jobs"), "{summary}");
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1);
    assert_eq!(project_at(&st, &b).vectors, 6);
    handle.shutdown();
}

#[test]
fn import_into_same_project_or_from_unknown_path_changes_nothing() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let a = mkdir(&e, "a");
    ingest_session(
        &e.paths,
        "sa",
        &transcript_at(e._dir.path(), "sa", &a, None),
    );
    let id = only_project(&e.paths);
    let (project, summary, sessions) = prepare(&e.paths, &s(&a), &a);
    assert!(summary.contains("No change"), "{summary}");
    assert_eq!((project.as_str(), sessions.len()), (id.as_str(), 0));
    let (project, _, sessions) = prepare(&e.paths, "/no/such/source", &a);
    assert_eq!((project.as_str(), sessions.len()), (id.as_str(), 0));
    assert_eq!(status(&e.paths).projects.len(), 1);
    handle.shutdown();
}

#[test]
fn import_brings_past_sessions_of_old_path_into_target_project() {
    let e = env();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        import_config(&e),
    )
    .unwrap();
    let old = e._dir.path().join("gone/old");
    let other = mkdir(&e, "other");
    let b = mkdir(&e, "b");
    past(&e, "old-1", "s1", &old);
    past(&e, "old-2", "s2", &old.join("sub"));
    past(&e, "other", "s3", &other);
    past(&e, "old-1/s1/subagents", "agent-x", &old);
    let (summary, reports) = run_import(&e.paths, &s(&old), &b);
    assert!(!summary.is_empty());
    assert_eq!(
        outcomes(&reports),
        vec![
            ("s1", ImportOutcome::Imported, 2),
            ("s2", ImportOutcome::Imported, 2)
        ]
    );
    assert!(reports[1].1.pending_embeds > 0);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1, "{st:?}");
    assert_eq!(project_at(&st, &b).iterations, 4);

    let (_, again) = run_import(&e.paths, &s(&old), &b);
    assert_eq!(
        outcomes(&again),
        vec![
            ("s1", ImportOutcome::Unchanged, 0),
            ("s2", ImportOutcome::Unchanged, 0)
        ],
        "다시 실행하면 끝난 세션은 지나감"
    );
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    handle.shutdown();
}

#[test]
fn import_skips_session_fixed_to_another_project() {
    let e = env();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        import_config(&e),
    )
    .unwrap();
    let x = mkdir(&e, "x");
    let sub = mkdir(&e, "x/sub");
    let b = mkdir(&e, "b");
    ingest_session(&e.paths, "sx", &past(&e, "slug-x", "sx", &sub));
    let (_, reports) = run_import(&e.paths, &s(&x), &b);
    assert_eq!(
        outcomes(&reports),
        vec![("sx", ImportOutcome::OtherProject, 0)]
    );
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 2, "{st:?}");
    assert_eq!(project_at(&st, &sub).iterations, 2);
    assert_eq!(project_at(&st, &b).iterations, 0);
    handle.shutdown();
}

#[test]
fn import_collects_worktree_sessions_into_main_project() {
    let e = env();
    let repo = mkdir(&e, "repo");
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    let wt = e._dir.path().join("wt");
    git(&repo, &["worktree", "add", "-q", "-b", "feature", &s(&wt)]);
    let wt = fs::canonicalize(&wt).unwrap();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        import_config(&e),
    )
    .unwrap();
    past(&e, "slug-repo", "sa", &repo.join("src"));
    past(&e, "slug-wt", "sb", &wt);
    let (_, reports) = run_import(&e.paths, &s(&repo), &repo);
    assert_eq!(
        outcomes(&reports),
        vec![
            ("sa", ImportOutcome::Imported, 2),
            ("sb", ImportOutcome::Imported, 2)
        ]
    );
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1, "{st:?}");
    assert_eq!(project_at(&st, &repo.join(".git")).iterations, 4);
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
    assert!(show(&e.paths, &p, Some(Scope::Project), &s(&b)).contains("no records"));
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
    let embed_text = hippo::embed_text::build_embed_text(&iteration, 500, &[]);
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
                bulk: false,
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

#[test]
fn startup_queue_embeds_bulk_jobs_after_normal_jobs() {
    use hippo::project::{Location, LocationKind};
    let e = env();
    let dir_loc = |p: &str| Location {
        path: p.into(),
        kind: LocationKind::Dir,
        toplevel: None,
        root_commits: vec![],
    };
    let mut registry = hippo::registry::Registry::open(&e.paths.registry()).unwrap();
    let ia = registry.resolve(&dir_loc("/gone/a"), 1).unwrap();
    let ib = registry.resolve(&dir_loc("/gone/b"), 1).unwrap();
    drop(registry);
    for (id, session, marker, bulk) in [
        (&ia, "sa", "MARKER_BULK", true),
        (&ib, "sb", "MARKER_LIVE", false),
    ] {
        RecordStore::open(&e.paths.project_records(id))
            .unwrap()
            .apply(&hippo::store::Batch {
                session_id: session.into(),
                transcript_path: "/t".into(),
                iterations: vec![marked_iteration(session, marker)],
                completions: vec![],
                state: hippo::transcript::ExtractorState::default(),
                bulk,
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
    assert!(pos("MARKER_LIVE") < pos("MARKER_BULK"), "{seen:?}");
    handle.shutdown();
}

#[test]
fn daemon_text_follows_language_setting_without_restart() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert!(project_list(&e.paths).contains("No project"), "기본은 영어");
    fs::write(e.paths.config(), "[ui]\nlanguage = \"ko\"\n").unwrap();
    assert!(
        project_list(&e.paths).contains("프로젝트 없음"),
        "다음 요청부터 반영"
    );
    fs::write(e.paths.config(), "[ui]\nlanguage = \"en\"\n").unwrap();
    assert!(project_list(&e.paths).contains("No project"));
    handle.shutdown();
}

fn inject_log_lines(paths: &Paths) -> Vec<String> {
    fs::read_to_string(paths.logs().join("hippo.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("inject budget exceeded"))
        .map(str::to_string)
        .collect()
}

fn inject_request(paths: &Paths, session_id: &str) -> Response {
    request(
        &paths.socket(),
        &Request::Inject {
            prompt: "anything".into(),
            cwd: s2_cwd(),
            session_id: session_id.into(),
            budget_ms: None,
        },
        TIMEOUT,
    )
    .unwrap()
}

#[test]
fn inject_over_budget_is_logged_with_queue_state() {
    let e = env();
    let mut cfg = config();
    cfg.inject.budget_ms = 0;
    let handle = launch_with(&e.paths, Arc::new(MemoryVectorStore::default()), cfg).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);

    assert!(matches!(
        inject_request(&e.paths, "other"),
        Response::Inject { context: None }
    ));
    handle.shutdown();

    let lines = inject_log_lines(&e.paths);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(line.contains("project "), "{line}");
    assert!(line.contains("budget=0ms"), "{line}");
    assert!(line.contains("elapsed="), "{line}");
    assert!(line.contains("queue normal=0 bulk=0"), "{line}");
}

#[test]
fn inject_within_budget_is_not_logged() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);

    assert!(matches!(
        inject_request(&e.paths, "other"),
        Response::Inject { .. }
    ));
    handle.shutdown();

    assert!(inject_log_lines(&e.paths).is_empty());
}

// ---- FR-RET-5 콜드 시작 예산, FR-DMN-10 워밍업 ----

fn zero_budget_config() -> Config {
    let mut c = config();
    c.inject.budget_ms = 0;
    c
}

fn inject_with_budget(paths: &Paths, prompt: &str, budget_ms: Option<u64>) -> Response {
    request(
        &paths.socket(),
        &Request::Inject {
            prompt: prompt.into(),
            cwd: s2_cwd(),
            session_id: "other".into(),
            budget_ms,
        },
        TIMEOUT,
    )
    .unwrap()
}

/// 색인된 데이터를 만든 뒤 데몬을 내린다. 이후 hook이 데몬을 새로 띄우는 콜드 시작을 재현한다.
fn indexed_then_stopped(e: &Env, vectors: &Arc<MemoryVectorStore>) -> String {
    let handle = launch_with(&e.paths, vectors.clone(), zero_budget_config()).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let prompt = project_records(&e.paths)
        .iterations_for_prompt(S2_PROMPT)
        .unwrap()[1]
        .embed_text
        .clone();
    handle.shutdown();
    prompt
}

#[test]
fn cold_start_hook_uses_cold_budget_and_daemon_honors_request_budget() {
    let e = env();
    let vectors = Arc::new(MemoryVectorStore::default());
    let prompt = indexed_then_stopped(&e, &vectors);
    let slot: Mutex<Option<DaemonHandle>> = Mutex::new(None);
    let launcher = |paths: &Paths| match relaunch(|| {
        launch_with(paths, vectors.clone(), zero_budget_config())
    }) {
        Ok(handle) => {
            *slot.lock().unwrap() = Some(handle);
            Ok(())
        }
        Err(err) => Err(std::io::Error::other(err.to_string())),
    };
    let input =
        json!({"session_id": "other", "transcript_path": "/t", "cwd": s2_cwd(), "prompt": prompt})
            .to_string();

    let cold = run_hook(
        &e.paths,
        Duration::ZERO,
        Duration::from_secs(5),
        "user-prompt-submit",
        &input,
        &launcher,
    );
    assert!(slot.lock().unwrap().is_some(), "데몬을 새로 띄움");
    assert!(
        cold.is_some_and(|out| out.contains("<memory ")),
        "콜드 시작은 cold 예산으로 주입한다"
    );

    let warm = run_hook(
        &e.paths,
        Duration::ZERO,
        Duration::from_secs(5),
        "user-prompt-submit",
        &input,
        &launcher,
    );
    assert_eq!(warm, None, "데몬이 떠 있으면 평소 예산(0)을 쓴다");

    assert!(matches!(
        inject_with_budget(&e.paths, &prompt, Some(5_000)),
        Response::Inject { context: Some(_) }
    ));
    assert!(matches!(
        inject_with_budget(&e.paths, &prompt, None),
        Response::Inject { context: None }
    ));
    slot.lock().unwrap().take().unwrap().shutdown();
}

struct CountingEmbedder {
    inner: HashEmbedder,
    calls: std::sync::atomic::AtomicUsize,
}

impl Embedder for CountingEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.embed(texts)
    }
}

#[test]
fn daemon_warms_up_embedder_on_start() {
    let e = env();
    let embedder = Arc::new(CountingEmbedder {
        inner: HashEmbedder::new(256, 8_000),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let handle = start(
        e.paths.clone(),
        config(),
        embedder.clone(),
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    wait_until(|| embedder.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    handle.shutdown();
}

/// 모델이 메모리에 없다고 알리는 임베더. (FR-RET-5 모델 콜드)
struct UnloadedEmbedder(HashEmbedder);

impl Embedder for UnloadedEmbedder {
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
        self.0.embed(texts)
    }
    fn is_loaded(&self) -> bool {
        false
    }
}

#[test]
fn daemon_uses_cold_budget_when_model_is_not_loaded() {
    let e = env();
    let vectors = Arc::new(MemoryVectorStore::default());
    let prompt = indexed_then_stopped(&e, &vectors);
    let handle = relaunch(|| {
        start(
            e.paths.clone(),
            zero_budget_config(),
            Arc::new(UnloadedEmbedder(HashEmbedder::new(256, 8_000))),
            memory_source(&e.paths, vectors.clone()),
        )
    })
    .unwrap();
    assert!(
        matches!(
            inject_with_budget(&e.paths, &prompt, Some(0)),
            Response::Inject { context: Some(_) }
        ),
        "모델이 내려가 있으면 요청 예산(0) 대신 콜드 예산을 쓴다"
    );

    let input =
        json!({"session_id": "other", "transcript_path": "/t", "cwd": s2_cwd(), "prompt": prompt})
            .to_string();
    let out = run_hook(
        &e.paths,
        Duration::ZERO,
        Duration::from_secs(5),
        "user-prompt-submit",
        &input,
        &launch_disabled,
    );
    assert!(
        out.is_some_and(|o| o.contains("<memory ")),
        "데몬이 떠 있어도 클라이언트는 콜드 예산까지 기다린다"
    );
    handle.shutdown();
}

// ---- FR-DMN-11 ----

#[test]
fn shutdown_request_stops_daemon_and_removes_socket() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    assert_eq!(
        request(&e.paths.socket(), &Request::Shutdown, TIMEOUT).unwrap(),
        Response::Done
    );
    assert!(exits_within(handle, Duration::from_secs(6)));
    assert!(!e.paths.socket().exists());
    let log = fs::read_to_string(e.paths.logs().join("hippo.log")).unwrap();
    assert!(log.contains("shutdown requested"), "{log}");
}

#[test]
fn shutdown_is_rejected_during_reindex() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(VerySlowEmbedder(HashEmbedder::new(256, 8_000)));
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
    let resp = request(&e.paths.socket(), &Request::Shutdown, TIMEOUT).unwrap();
    assert!(matches!(resp, Response::Error { .. }), "{resp:?}");
    assert_eq!(status(&e.paths).reindex, "running");
    wait_reindex(&e.paths);
    handle.shutdown();
}

#[test]
fn pending_jobs_survive_shutdown_request() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(VerySlowEmbedder(HashEmbedder::new(256, 8_000)));
    let handle = start(
        e.paths.clone(),
        config_with("[chunking]\nmax_tokens = 8000\n[index]\njob_batch = 1\n[import]\ntranscripts_dir = \"/nonexistent/hippo-test-transcripts\"\n"),
        embedder,
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    let records = project_records(&e.paths);
    assert!(totals(&status(&e.paths)).1 > 1);
    assert_eq!(
        request(&e.paths.socket(), &Request::Shutdown, TIMEOUT).unwrap(),
        Response::Done
    );
    assert!(exits_within(handle, Duration::from_secs(10)));
    assert!(
        !records.embed_jobs(100).unwrap().is_empty(),
        "남은 작업은 원본 저장소에 남음"
    );

    let handle = relaunch(|| launch(&e.paths, Arc::new(MemoryVectorStore::default()))).unwrap();
    wait_until(|| {
        let (_, jobs, vectors) = totals(&status(&e.paths));
        jobs == 0 && vectors > 0
    });
    handle.shutdown();
}

// ---- FR-IMP-8, FR-IMP-9 ----

struct Preview {
    project_id: String,
    source_id: Option<String>,
    summary: String,
    sessions: Vec<Candidate>,
}

fn preview(paths: &Paths, from: &str, to: &Path) -> Preview {
    match request(
        &paths.socket(),
        &Request::ImportPrepare {
            from: from.into(),
            to: s(to),
            dry_run: true,
        },
        TIMEOUT,
    )
    .unwrap()
    {
        Response::ImportPlan {
            project_id,
            source_id,
            summary,
            sessions,
        } => Preview {
            project_id,
            source_id,
            summary,
            sessions,
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn preview_of_merge_changes_nothing_and_matches_real_prepare() {
    let (e, handle, a, b) = two_project_env();
    let st = status(&e.paths);
    let source = project_at(&st, &a).id.clone();
    let target = project_at(&st, &b).id.clone();
    let before = project_list(&e.paths);
    let p = preview(&e.paths, &source, &b);
    assert!(p.summary.contains("Would merge"), "{}", p.summary);
    assert_eq!(
        (p.project_id.as_str(), p.source_id.as_deref()),
        (target.as_str(), Some(source.as_str()))
    );
    assert_eq!(project_list(&e.paths), before, "목록 DB 그대로");
    assert!(e.paths.project_dir(&source).exists());
    let (real, _, _) = prepare(&e.paths, &source, &b);
    assert_eq!(real, target);
    handle.shutdown();
}

#[test]
fn preview_of_move_and_create_changes_nothing() {
    let e = env();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        import_config(&e),
    )
    .unwrap();
    let fresh = mkdir(&e, "fresh");
    let p = preview(&e.paths, &s(&fresh), &fresh);
    assert!(p.summary.contains("Would create"), "{}", p.summary);
    assert_eq!((p.project_id.as_str(), p.source_id.as_deref()), ("", None));
    assert!(
        status(&e.paths).projects.is_empty(),
        "프로젝트를 만들지 않음"
    );

    let a = mkdir(&e, "a");
    ingest_session(&e.paths, "sa", &past(&e, "slug-a", "sa", &a));
    let id = only_project(&e.paths);
    let b = e._dir.path().join("b");
    fs::rename(&a, &b).unwrap();
    let b = fs::canonicalize(&b).unwrap();
    let p = preview(&e.paths, &s(&a), &b);
    assert!(p.summary.contains("Would move"), "{}", p.summary);
    assert_eq!(p.project_id, id);
    assert_eq!(
        status(&e.paths).projects[0].location,
        s(&a),
        "위치를 바꾸지 않음"
    );
    assert_eq!(p.sessions.len(), 1);
    assert_ne!(
        p.sessions[0].status,
        ImportStatus::New,
        "원본 프로젝트의 세션 상태를 봄"
    );
    handle.shutdown();
}

#[test]
fn preview_lists_status_and_info_of_each_candidate() {
    let e = env();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        import_config(&e),
    )
    .unwrap();
    let x = mkdir(&e, "x");
    let sub = mkdir(&e, "x/sub");
    let b = mkdir(&e, "b");
    ingest_session(&e.paths, "sx", &past(&e, "slug-x", "sx", &sub));
    past(&e, "slug-x", "s1", &x);
    let p = preview(&e.paths, &s(&x), &b);
    let by_id = |id: &str| {
        p.sessions
            .iter()
            .find(|c| c.session_id == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by_id("sx").status, ImportStatus::OtherProject);
    let s1 = by_id("s1");
    assert_eq!(s1.status, ImportStatus::New);
    assert_eq!(s1.start_cwd, s(&x));
    assert!(
        s1.size > 0 && !s1.started_at.is_empty() && !s1.first_prompt.is_empty(),
        "{s1:?}"
    );

    run_import(&e.paths, &s(&x), &b);
    let again = preview(&e.paths, &s(&x), &b);
    let s1 = again
        .sessions
        .iter()
        .find(|c| c.session_id == "s1")
        .unwrap();
    assert_eq!(s1.status, ImportStatus::Imported);
    handle.shutdown();
}

#[test]
fn preview_is_allowed_during_reindex() {
    let e = env();
    let embedder: Arc<dyn Embedder> = Arc::new(VerySlowEmbedder(HashEmbedder::new(256, 8_000)));
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
    let p = preview(&e.paths, &s2_cwd(), Path::new(&s2_cwd()));
    assert!(p.summary.contains("No change"), "{}", p.summary);
    wait_reindex(&e.paths);
    handle.shutdown();
}

// ---- FR-STO-12: 임베딩 중 버전 상한과 큐가 빌 때 정리 ----

/// LanceDB 저장소를 Probe로 감싸 연다. 연 저장소와 디렉터리를 `opened`에 모은다.
type Opened = Arc<Mutex<Vec<(PathBuf, Arc<probe::Probe>)>>>;

fn probed_lance_source(opened: Opened) -> VectorSource {
    VectorSource {
        name: "default".into(),
        open: Arc::new(move |dir: &Path| {
            let inner = hippo::lance::LanceVectorStore::open(dir)?;
            let p = Arc::new(probe::Probe::new(Box::new(inner)));
            opened.lock().unwrap().push((dir.to_path_buf(), p.clone()));
            Ok(p as Arc<dyn VectorStore>)
        }),
    }
}

fn lance_config(optimize_every: u64) -> Config {
    Config::parse(&format!(
        "[embedder]\nbackend = \"hash\"\ndimension = 256\nquery_prefix = \"\"\n[vector]\nbackend = \"lancedb\"\n[chunking]\nmax_tokens = 8000\n[index]\njob_batch = 1\noptimize_every = {optimize_every}\n[import]\ntranscripts_dir = \"/nonexistent/hippo-test-transcripts\"\n"
    ))
    .unwrap()
}

/// 임베딩을 마치고 큐가 빈 뒤의 (쓰기 직후 최대 pending, 남은 버전 수).
fn embed_with_optimize_every(optimize_every: u64) -> (u64, usize) {
    let e = env();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    let handle = start(
        e.paths.clone(),
        lance_config(optimize_every),
        embedder,
        probed_lance_source(opened.clone()),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let (dir, p) = opened.lock().unwrap()[0].clone();
    assert_eq!(
        p.replaces.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "작업 4개, 묶음 1개씩"
    );
    wait_until(|| p.pending_changes() == 0);
    let result = (
        p.max_pending.load(std::sync::atomic::Ordering::SeqCst),
        probe::lance_versions(&dir),
    );
    handle.shutdown();
    result
}

#[test]
fn embedding_keeps_lancedb_versions_bounded() {
    let (max_pending, versions) = embed_with_optimize_every(2);
    assert!(max_pending <= 2 + 1, "max pending {max_pending}");
    assert_eq!(versions, 1, "큐가 비면 정리");
}

#[test]
fn zero_optimize_every_only_cleans_when_queue_is_empty() {
    let (max_pending, versions) = embed_with_optimize_every(0);
    assert_eq!(max_pending, 8, "쓰는 도중에는 정리하지 않음");
    assert_eq!(versions, 1, "큐가 비면 정리");
}

#[test]
fn reindexed_lancedb_generation_keeps_one_version() {
    let e = env();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(256, 8_000));
    let handle = start(
        e.paths.clone(),
        lance_config(20),
        embedder,
        probed_lance_source(opened.clone()),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap();
    assert_eq!(wait_reindex(&e.paths).reindex, "done");
    let (dir, p) = opened.lock().unwrap().last().unwrap().clone();
    assert_eq!(p.pending_changes(), 0);
    assert_eq!(probe::lance_versions(&dir), 1);
    assert_eq!(p.count().unwrap(), 4);
    handle.shutdown();
}

// ---- SRS 3.2.13 삭제 (FR-DEL), IF-SK-2 ----

use hippo::protocol::DeleteReport;
use hippo::store::ForgetTarget;

fn delete_report(resp: Response) -> Option<DeleteReport> {
    match resp {
        Response::Delete { report } => report,
        other => panic!("{other:?}"),
    }
}

fn forget_req(target: ForgetTarget, scope: Option<Scope>, dry_run: bool) -> Request {
    Request::Forget {
        target,
        scope,
        cwd: "/".into(),
        dry_run,
    }
}

fn remove_req(project: &str, dry_run: bool) -> Request {
    Request::ProjectRemove {
        project: project.into(),
        cwd: "/".into(),
        dry_run,
    }
}

#[test]
fn project_remove_deletes_everything_and_same_path_becomes_new_project() {
    let e = env();
    let handle = launch_with(
        &e.paths,
        Arc::new(MemoryVectorStore::default()),
        config_with("[index]\njob_batch = 1\n[import]\ntranscripts_dir = \"/nonexistent/hippo-test-transcripts\"\n"),
    )
    .unwrap();
    assert_eq!(
        ingest(&e.paths, &e.transcript, HookEvent::Stop),
        Response::Done
    );
    let id = only_project(&e.paths);
    let dir = e.paths.project_dir(&id);
    assert!(dir.exists());

    let preview =
        delete_report(request(&e.paths.socket(), &remove_req(&id, true), TIMEOUT).unwrap())
            .expect("대상 있음");
    assert_eq!(preview.project_id, id);
    assert_eq!(preview.location, s2_cwd());
    assert_eq!(preview.counts.sessions, 1);
    assert_eq!(preview.counts.iterations, 2);
    assert_eq!(status(&e.paths).projects.len(), 1, "dry_run은 바꾸지 않음");

    let path_req = Request::ProjectRemove {
        project: s2_cwd(),
        cwd: "/".into(),
        dry_run: false,
    };
    let done = delete_report(request(&e.paths.socket(), &path_req, TIMEOUT).unwrap())
        .expect("경로로도 찾음");
    assert_eq!(done.counts, preview.counts);
    assert!(status(&e.paths).projects.is_empty());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !dir.exists(),
        "큐에 남은 작업이 디렉터리를 다시 만들지 않음"
    );
    assert_eq!(
        delete_report(request(&e.paths.socket(), &remove_req(&id, false), TIMEOUT).unwrap()),
        None
    );

    assert_eq!(
        ingest(&e.paths, &e.transcript, HookEvent::Stop),
        Response::Done
    );
    let again = only_project(&e.paths);
    assert_ne!(again, id, "같은 경로의 새 세션은 새 프로젝트");
    assert_eq!(totals(&status(&e.paths)).0, 2);
    handle.shutdown();
}

#[test]
fn forget_session_removes_records_and_vectors_and_hook_saves_again() {
    let e = env();
    let vectors = Arc::new(MemoryVectorStore::default());
    let handle = launch(&e.paths, vectors.clone()).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    assert!(vectors.count().unwrap() > 0);
    let target = ForgetTarget::Session(S2.into());

    let preview = delete_report(
        request(
            &e.paths.socket(),
            &forget_req(target.clone(), None, true),
            TIMEOUT,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(preview.counts.sessions, 1);
    assert_eq!(preview.counts.iterations, 2);
    assert_eq!(totals(&status(&e.paths)).0, 2, "dry_run은 바꾸지 않음");

    let done = delete_report(
        request(
            &e.paths.socket(),
            &forget_req(target.clone(), None, false),
            TIMEOUT,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(done.counts, preview.counts);
    let st = status(&e.paths);
    assert_eq!(st.projects.len(), 1, "세션 삭제는 프로젝트를 남김");
    assert_eq!(totals(&st), (0, 0, 0), "반복, 작업, 벡터(모든 청크)");
    assert_eq!(vectors.count().unwrap(), 0);
    assert_eq!(st.projects[0].results, 0);
    assert_eq!(
        delete_report(
            request(&e.paths.socket(), &forget_req(target, None, false), TIMEOUT).unwrap()
        ),
        None,
        "대상 없음"
    );

    assert_eq!(
        ingest(&e.paths, &e.transcript, HookEvent::Stop),
        Response::Done
    );
    assert_eq!(
        totals(&status(&e.paths)).0,
        2,
        "기준 턴부터 다시 저장 (FR-EXT-12)"
    );
    handle.shutdown();
}

#[test]
fn forget_prompt_uses_show_lookup_order_and_project_id_scope() {
    let (e, handle, a, b) = two_project_env();
    let st = status(&e.paths);
    let (ia, ib) = (
        project_at(&st, &a).id.clone(),
        project_at(&st, &b).id.clone(),
    );
    let target = ForgetTarget::Prompt(prompt_of("sa"));
    assert_eq!(
        delete_report(
            request(
                &e.paths.socket(),
                &forget_req(target.clone(), Some(Scope::Id(ib.clone())), false),
                TIMEOUT
            )
            .unwrap()
        ),
        None,
        "다른 프로젝트 범위에서는 대상 없음"
    );
    let found = Request::Forget {
        target: target.clone(),
        scope: None,
        cwd: s(&b),
        dry_run: true,
    };
    let preview = delete_report(request(&e.paths.socket(), &found, TIMEOUT).unwrap()).unwrap();
    assert_eq!(
        preview.project_id, ia,
        "현재 프로젝트에 없으면 나머지에서 찾음"
    );
    let done = delete_report(
        request(
            &e.paths.socket(),
            &forget_req(target, Some(Scope::Id(ia.clone())), false),
            TIMEOUT,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(done.counts.turns, 1);
    let st = status(&e.paths);
    assert_eq!(project_at(&st, &a).iterations, 0);
    assert_eq!(project_at(&st, &b).iterations, 2, "다른 프로젝트는 그대로");
    let records = RecordStore::open(&e.paths.project_records(&ia)).unwrap();
    assert!(
        records.session_state("sa").unwrap().is_some(),
        "턴 삭제는 수집 위치를 남김"
    );
    assert!(
        text(
            request(
                &e.paths.socket(),
                &search_req("병렬로", Scope::Id(ib), "/"),
                TIMEOUT
            )
            .unwrap()
        )
        .contains("[iteration]"),
        "프로젝트 id 범위 검색"
    );
    assert!(
        text(
            request(
                &e.paths.socket(),
                &search_req("병렬로", Scope::Id("0".repeat(32)), "/"),
                TIMEOUT
            )
            .unwrap()
        )
        .contains("No project")
    );
    handle.shutdown();
}

#[test]
fn delete_is_rejected_while_reindexing() {
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
    let id = only_project(&e.paths);
    assert_eq!(
        request(&e.paths.socket(), &reindex_all(), TIMEOUT).unwrap(),
        Response::Done
    );
    for req in [
        remove_req(&id, false),
        forget_req(ForgetTarget::Session(S2.into()), None, false),
    ] {
        assert!(
            matches!(
                request(&e.paths.socket(), &req, TIMEOUT).unwrap(),
                Response::Error { .. }
            ),
            "{req:?}"
        );
    }
    assert_eq!(wait_reindex(&e.paths).reindex, "done");
    assert_eq!(totals(&status(&e.paths)).0, 2);
    handle.shutdown();
}

/// 웜업이 아닌 첫 임베딩에서 `release`가 참이 될 때까지 멈춘다.
struct GatedEmbedder {
    inner: HashEmbedder,
    entered: Arc<std::sync::atomic::AtomicBool>,
    release: Arc<std::sync::atomic::AtomicBool>,
}

impl Embedder for GatedEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        use std::sync::atomic::Ordering::SeqCst;
        if texts.first().is_some_and(|t| t != "warmup") {
            self.entered.store(true, SeqCst);
            while !self.release.load(SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.inner.embed(texts)
    }
}

#[test]
fn in_flight_embedding_batch_does_not_store_vectors_of_forgotten_records() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let e = env();
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let vectors = Arc::new(MemoryVectorStore::default());
    let handle = start(
        e.paths.clone(),
        config(),
        Arc::new(GatedEmbedder {
            inner: HashEmbedder::new(256, 8_000),
            entered: entered.clone(),
            release: release.clone(),
        }),
        memory_source(&e.paths, vectors.clone()),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| entered.load(SeqCst));
    let socket = e.paths.socket();
    let forget = std::thread::spawn(move || {
        request(
            &socket,
            &forget_req(ForgetTarget::Session(S2.into()), None, false),
            TIMEOUT,
        )
        .unwrap()
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!forget.is_finished(), "처리 중인 묶음이 끝나기를 기다림");
    release.store(true, SeqCst);
    assert!(delete_report(forget.join().unwrap()).is_some());
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        vectors.count().unwrap(),
        0,
        "지운 레코드의 벡터가 남지 않음"
    );
    handle.shutdown();
}

/// 조회 요청의 시간 제한(30초)보다 길게 기다린다.
const LONGER_THAN_QUERY_TIMEOUT: Duration = Duration::from_secs(31);

#[test]
fn delete_waits_for_long_embedding_batch_and_reports_success() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let e = env();
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let handle = start(
        e.paths.clone(),
        config(),
        Arc::new(GatedEmbedder {
            inner: HashEmbedder::new(256, 8_000),
            entered: entered.clone(),
            release: release.clone(),
        }),
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| entered.load(SeqCst));
    let socket = e.paths.socket();
    let forget = std::thread::spawn(move || {
        request_until_reply(
            &socket,
            &forget_req(ForgetTarget::Session(S2.into()), None, false),
        )
    });
    std::thread::sleep(LONGER_THAN_QUERY_TIMEOUT);
    assert!(!forget.is_finished(), "묶음이 끝날 때까지 오류 없이 기다림");
    release.store(true, SeqCst);
    assert!(delete_report(forget.join().unwrap().unwrap()).is_some());
    assert_eq!(
        totals(&status(&e.paths)).0,
        0,
        "성공 응답 뒤에는 지워져 있음"
    );
    handle.shutdown();
}

#[test]
fn import_prepare_waits_behind_blocked_writer_and_reports_result() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let e = env();
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let handle = start(
        e.paths.clone(),
        import_config(&e),
        Arc::new(GatedEmbedder {
            inner: HashEmbedder::new(256, 8_000),
            entered: entered.clone(),
            release: release.clone(),
        }),
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    let a = mkdir(&e, "a");
    ingest_session(&e.paths, "sa", &past(&e, "slug-a", "sa", &a));
    let other = mkdir(&e, "o");
    ingest_session(&e.paths, "so", &past(&e, "slug-o", "so", &other));
    wait_until(|| entered.load(SeqCst));
    let socket = e.paths.socket();
    // 묶음을 기다리는 삭제가 쓰기 스레드를 막는다.
    let forget = std::thread::spawn(move || {
        request_until_reply(
            &socket,
            &forget_req(ForgetTarget::Session("so".into()), None, false),
        )
    });
    std::thread::sleep(Duration::from_millis(300));
    let b = e._dir.path().join("b");
    fs::rename(&a, &b).unwrap();
    let b = fs::canonicalize(&b).unwrap();
    let socket = e.paths.socket();
    let (from, to) = (s(&a), s(&b));
    let prepare = std::thread::spawn(move || {
        request_until_reply(
            &socket,
            &Request::ImportPrepare {
                from,
                to,
                dry_run: false,
            },
        )
    });
    std::thread::sleep(LONGER_THAN_QUERY_TIMEOUT);
    assert!(
        !prepare.is_finished(),
        "쓰기 스레드가 풀릴 때까지 오류 없이 기다림"
    );
    release.store(true, SeqCst);
    assert!(delete_report(forget.join().unwrap().unwrap()).is_some());
    match prepare.join().unwrap().unwrap() {
        Response::ImportPlan { summary, .. } => {
            assert!(summary.contains("Moved project"), "{summary}")
        }
        other => panic!("{other:?}"),
    }
    project_at(&status(&e.paths), &b);
    handle.shutdown();
}

#[test]
fn current_project_resolves_cwd_without_registering() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    let current = |cwd: &str| match request(
        &e.paths.socket(),
        &Request::CurrentProject { cwd: cwd.into() },
        TIMEOUT,
    )
    .unwrap()
    {
        Response::CurrentProject { id } => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(current(&s2_cwd()), None, "수집 전에는 프로젝트 없음");
    assert!(status(&e.paths).projects.is_empty(), "조회는 등록하지 않음");
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    assert_eq!(current(&s2_cwd()), Some(only_project(&e.paths)));
    handle.shutdown();
}

/// 웜업이 아닌 임베딩 호출마다 허가 하나를 써야 진행한다.
struct SteppedEmbedder {
    inner: HashEmbedder,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    permits: Arc<std::sync::atomic::AtomicUsize>,
}

impl Embedder for SteppedEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, hippo::embedder::EmbedError> {
        use std::sync::atomic::Ordering::SeqCst;
        if texts.first().is_some_and(|t| t != "warmup") {
            self.calls.fetch_add(1, SeqCst);
            while self
                .permits
                .fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1))
                .is_err()
            {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.inner.embed(texts)
    }
}

#[test]
fn waiting_delete_preempts_another_projects_batch_after_one_job() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let e = env();
    let calls = Arc::new(AtomicUsize::new(0));
    let permits = Arc::new(AtomicUsize::new(0));
    let handle = start(
        e.paths.clone(),
        import_config(&e),
        Arc::new(SteppedEmbedder {
            inner: HashEmbedder::new(256, 8_000),
            calls: calls.clone(),
            permits: permits.clone(),
        }),
        memory_source(&e.paths, Arc::new(MemoryVectorStore::default())),
    )
    .unwrap();
    let a = mkdir(&e, "a");
    ingest_session(&e.paths, "sa", &past(&e, "slug-a", "sa", &a));
    wait_until(|| calls.load(SeqCst) == 1);
    let b = mkdir(&e, "b");
    ingest_session(&e.paths, "sb", &past(&e, "slug-b", "sb", &b));
    let jobs_of = |dir: &Path| project_at(&status(&e.paths), dir).embed_jobs;
    let a_jobs = jobs_of(&a);
    assert!(a_jobs > 1, "A의 묶음에 작업이 여럿 있어야 함");
    let socket = e.paths.socket();
    let forget = std::thread::spawn(move || {
        request_until_reply(
            &socket,
            &forget_req(ForgetTarget::Session("sb".into()), None, false),
        )
    });
    std::thread::sleep(Duration::from_millis(200));
    permits.store(1, SeqCst);
    let deadline = Instant::now() + TIMEOUT;
    while !forget.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let finished = forget.is_finished();
    let a_left = jobs_of(&a);
    permits.store(usize::MAX, SeqCst);
    assert!(finished, "작업 1건 뒤 묶음이 멈추고 삭제가 끝남");
    assert!(delete_report(forget.join().unwrap().unwrap()).is_some());
    assert_eq!(a_left, a_jobs - 1, "A의 남은 작업은 큐에 남음");
    wait_until(|| jobs_of(&a) == 0);
    handle.shutdown();
}

// ---- FR-GUI-13~17 (데몬 요청 단위) ----

use hippo::search::{InjectionPlan, SkipReason, Verdict};

fn ask(paths: &Paths, req: Request) -> Response {
    request(&paths.socket(), &req, TIMEOUT).unwrap()
}

#[test]
fn browse_requests_list_sessions_turns_and_turn_detail() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let id = only_project(&e.paths);
    let scope = Scope::Id(id.clone());
    let sessions = |filter: &str| match ask(
        &e.paths,
        Request::SessionList {
            scope: scope.clone(),
            cwd: "/".into(),
            filter: filter.into(),
            offset: 0,
            limit: 50,
        },
    ) {
        Response::Sessions(page) => page,
        other => panic!("{other:?}"),
    };
    let page = sessions("");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].session_id, S2);
    assert_eq!(page.items[0].iterations, 2);
    assert_eq!(sessions("병렬로").total, 1, "FTS 필터");
    assert_eq!(sessions("없는글자열입니다").total, 0);

    let turns = match ask(
        &e.paths,
        Request::TurnList {
            scope: scope.clone(),
            cwd: "/".into(),
            session_id: S2.into(),
            filter: String::new(),
            offset: 0,
            limit: 50,
        },
    ) {
        Response::Turns(page) => page,
        other => panic!("{other:?}"),
    };
    let turn = turns
        .items
        .iter()
        .find(|t| t.prompt_id.as_deref() == Some(S2_PROMPT))
        .unwrap();
    assert_eq!(turn.pending, 0, "임베딩 완료");

    let (project_id, iterations) = match ask(
        &e.paths,
        Request::TurnDetail {
            scope: scope.clone(),
            cwd: "/".into(),
            prompt_id: S2_PROMPT.into(),
        },
    ) {
        Response::Turn {
            project_id,
            iterations,
        } => (project_id, iterations),
        other => panic!("{other:?}"),
    };
    assert_eq!(project_id, id);
    let shown = text(ask(
        &e.paths,
        Request::Show {
            prompt_id: S2_PROMPT.into(),
            with_results: true,
            scope: Some(scope.clone()),
            cwd: "/".into(),
        },
    ));
    assert_eq!(iterations.len(), shown.matches("\n## [").count());
    assert!(iterations.iter().all(|it| shown.contains(&it.id)));
    assert!(
        iterations
            .iter()
            .flat_map(|i| &i.tools)
            .any(|t| t.result.is_some())
    );

    let unknown = Request::SessionList {
        scope: Scope::Id("0".repeat(32)),
        cwd: "/".into(),
        filter: String::new(),
        offset: 0,
        limit: 50,
    };
    assert!(matches!(ask(&e.paths, unknown), Response::Error { .. }));
    handle.shutdown();
}

#[test]
fn inject_preview_selects_the_turn_that_inject_injects() {
    let e = env();
    let handle = launch(&e.paths, Arc::new(MemoryVectorStore::default())).unwrap();
    ingest(&e.paths, &e.transcript, HookEvent::Stop);
    wait_until(|| totals(&status(&e.paths)).1 == 0);
    let id = only_project(&e.paths);
    let prompt = project_records(&e.paths)
        .iterations_for_prompt(S2_PROMPT)
        .unwrap()[1]
        .embed_text
        .clone();
    let preview = |scope: Scope, prompt: &str| match ask(
        &e.paths,
        Request::InjectPreview {
            scope,
            cwd: "/".into(),
            prompt: prompt.into(),
        },
    ) {
        Response::InjectPreview { plan, .. } => plan,
        other => panic!("{other:?}"),
    };
    let injected = match inject_with_budget(&e.paths, &prompt, Some(5_000)) {
        Response::Inject { context: Some(c) } => c,
        other => panic!("{other:?}"),
    };
    assert!(injected.contains(&format!("prompt=\"{S2_PROMPT}\"")));
    match preview(Scope::Id(id.clone()), &prompt) {
        InjectionPlan::Planned { candidates, text } => {
            let selected: Vec<_> = candidates
                .iter()
                .filter(|c| c.verdict == Verdict::Selected)
                .map(|c| c.prompt_id.clone())
                .collect();
            assert_eq!(selected, vec![Some(S2_PROMPT.to_string())]);
            assert_eq!(text.as_deref(), Some(injected.as_str()));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        preview(Scope::Id(id), "짧은 질문"),
        InjectionPlan::Skipped {
            reason: SkipReason::TooShort
        }
    );
    assert_eq!(
        preview(Scope::Id("0".repeat(32)), &prompt),
        InjectionPlan::Skipped {
            reason: SkipReason::NoProject
        }
    );
    handle.shutdown();
}

#[test]
fn structured_search_returns_project_and_prompt_of_each_hit() {
    let (e, handle, a, _b) = two_project_env();
    let ia = project_at(&status(&e.paths), &a).id.clone();
    let hits = |scope: Scope| match ask(
        &e.paths,
        Request::Search {
            query: "두 파일은 병렬로".into(),
            kind: SearchKind::Iteration,
            limit: 5,
            scope,
            cwd: "/".into(),
            structured: true,
        },
    ) {
        Response::Hits { hits, notes } => {
            assert!(notes.is_empty(), "{notes:?}");
            hits
        }
        other => panic!("{other:?}"),
    };
    let one = hits(Scope::Id(ia.clone()));
    assert!(!one.is_empty());
    assert!(
        one.iter()
            .all(|h| h.project_id == ia && h.kind == "iteration")
    );
    assert!(
        one.iter()
            .all(|h| h.prompt_id.as_deref() == Some(prompt_of("sa").as_str()))
    );
    assert!(one.iter().all(|h| h.similarity.is_some()));
    match ask(
        &e.paths,
        Request::Search {
            query: "병렬로".into(),
            kind: SearchKind::Iteration,
            limit: 5,
            scope: Scope::Project,
            cwd: "/no/such/project".into(),
            structured: true,
        },
    ) {
        Response::Hits { hits, notes } => {
            assert!(hits.is_empty());
            assert_eq!(
                notes,
                vec!["No project".to_string()],
                "텍스트 검색처럼 안내"
            );
        }
        other => panic!("{other:?}"),
    }
    let all = hits(Scope::All);
    let projects: std::collections::HashSet<_> = all.iter().map(|h| h.project_id.clone()).collect();
    assert_eq!(projects.len(), 2, "all 범위는 두 프로젝트");
    handle.shutdown();
}
