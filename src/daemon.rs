//! 상주 데몬. 쓰기 스레드, 임베딩 스레드, 연결 처리 스레드로 구성한다. (SRS 3.2.7, 3.2.10)

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::backends::VectorSource;
use crate::config::{Config, Paths};
use crate::embed_text::chunk_limit;
use crate::embedder::{Embedder, InjectGate, YieldingEmbedder};
use crate::fsutil::{ensure_private_dir, open_private_append, set_private_file};
use crate::generations::{activate, active_dir, cleanup_inactive, new_generation};
use crate::i18n::{Lang, local_time};
use crate::import::{Candidate, candidates, import_status};
use crate::indexer::{
    ChunkConfig, Compatibility, compatibility, optimize_if_due, process_jobs_until, reindex,
};
use crate::ingest::{
    ImportOutcome, ImportReport, IngestOptions, IngestRequest, import_session, ingest,
    session_start_cwd,
};
use crate::project::{Location, locate, normalize};
use crate::protocol::{
    COMPAT_MISMATCH, ImportSessionReport, ProjectStatus, REINDEX_RUNNING, Request, Response, Scope,
    StatusInfo, missing_location,
};
use crate::queue::{EmbedQueue, Lane, LaneQueue};
use crate::redact::Redactor;
use crate::registry::{GIT_DIR_NAME, Registry, RegistryError, is_project_id};
use crate::search::{
    InjectConfig, ProjectSearch, SearchError, SearchKind, SearchRequest, VectorBackends,
    build_injection, escape_control, missing_turn, render_hits, render_project_hits, render_turn,
    search, search_projects,
};
use crate::spool;
use crate::store::{EmbedJob, RecordStore};
use crate::tr;
use crate::vector::VectorStore;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("daemon already running")]
    AlreadyRunning,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid redact pattern {kind}: {message}")]
    Redact { kind: String, message: String },
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

const POLL: Duration = Duration::from_millis(20);
const WRITE_POLL: Duration = Duration::from_millis(100);
const EMBED_IDLE: Duration = Duration::from_secs(1);
const EMBED_RETRY: Duration = Duration::from_secs(5);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const REINDEX_PAGE: usize = 256;
const LOG_FILE: &str = "hippo.log";
const WARMUP_TEXT: &str = "warmup";
/// 처리 속도를 셀 때 보는 시간. (IF-CLI-5)
const RATE_WINDOW: Duration = Duration::from_secs(300);
/// 처리를 막 시작했을 때 속도가 부풀지 않게 하는 최소 시간.
const RATE_MIN_SPAN: Duration = Duration::from_secs(30);

fn prune_rate_window(entries: &mut VecDeque<(Instant, usize)>) {
    while entries
        .front()
        .is_some_and(|(at, _)| at.elapsed() > RATE_WINDOW)
    {
        entries.pop_front();
    }
}
const REINDEX_IDLE: &str = "idle";
const REINDEX_DONE: &str = "done";
fn no_project(lang: Lang) -> String {
    tr!(lang, "No project\n", "프로젝트 없음\n")
}

fn reindex_busy(lang: Lang) -> String {
    tr!(
        lang,
        "Cannot import while reindexing",
        "재색인 중에는 가져올 수 없습니다"
    )
}

fn shutdown_busy(lang: Lang) -> String {
    tr!(
        lang,
        "Cannot shut down while reindexing",
        "재색인 중에는 데몬을 종료할 수 없습니다"
    )
}

type Reply = Sender<Response>;

/// 쓰기 스레드가 처리하는 작업. 세션 가져오기는 다른 작업보다 뒤에 처리한다. (FR-DMN-6, FR-IMP-6)
enum WriteJob {
    Ingest(IngestRequest, Reply),
    ImportPrepare {
        from: String,
        to: String,
        reply: Reply,
    },
    ImportSession {
        project_id: String,
        session_id: String,
        transcript_path: PathBuf,
        reply: Reply,
    },
}

impl WriteJob {
    fn lane(&self) -> Lane {
        match self {
            Self::ImportSession { .. } => Lane::Bulk,
            Self::Ingest(..) | Self::ImportPrepare { .. } => Lane::Normal,
        }
    }
}

/// 프로젝트 하나의 벡터 저장소와 호환성 상태. (FR-DMN-9, FR-SWP-6)
struct ProjectHandle {
    vectors: Arc<dyn VectorStore>,
    compat: Mutex<Compatibility>,
}

impl ProjectHandle {
    fn compat(&self) -> Compatibility {
        self.compat.lock().map_or(
            Compatibility::Mismatch {
                store_model: None,
                store_dimension: None,
            },
            |c| c.clone(),
        )
    }

    fn is_mismatch(&self) -> bool {
        matches!(self.compat(), Compatibility::Mismatch { .. })
    }

    /// 호환되지 않으면 벡터 검색을 끄고 FTS 전용으로 동작한다. (FR-SWP-6)
    fn backends<'a>(
        &'a self,
        embedder: &'a dyn Embedder,
        query_chars: usize,
        query_prefix: &'a str,
    ) -> Option<VectorBackends<'a>> {
        (!self.is_mismatch()).then(|| VectorBackends {
            embedder,
            vectors: self.vectors.as_ref(),
            query_chars,
            query_prefix,
        })
    }

    fn refresh(&self, embedder: &dyn Embedder) {
        if let Ok(c) = compatibility(embedder, self.vectors.as_ref())
            && let Ok(mut current) = self.compat.lock()
        {
            *current = c;
        }
    }
}

/// 열어 둔 프로젝트. 뒤가 가장 최근에 쓴 것이다. (FR-DMN-9)
struct OpenProjects {
    cap: usize,
    entries: VecDeque<(String, Arc<ProjectHandle>)>,
}

impl OpenProjects {
    fn get(&mut self, id: &str) -> Option<Arc<ProjectHandle>> {
        let pos = self.entries.iter().position(|(k, _)| k == id)?;
        let entry = self.entries.remove(pos)?;
        let handle = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(handle)
    }

    fn insert(&mut self, id: &str, handle: Arc<ProjectHandle>) {
        self.remove(id);
        self.entries.push_back((id.to_string(), handle));
        while self.entries.len() > self.cap.max(1) {
            self.entries.pop_front();
        }
    }

    fn remove(&mut self, id: &str) {
        self.entries.retain(|(k, _)| k != id);
    }
}

struct Shared {
    paths: Paths,
    config: Config,
    embedder: Arc<dyn Embedder>,
    source: VectorSource,
    registry: Mutex<Registry>,
    open: Mutex<OpenProjects>,
    queue: Mutex<EmbedQueue>,
    /// 처리 중인 자동 주입. 임베딩 스레드는 이것이 비어야 청크 요청을 보낸다. (FR-DMN-6 (a))
    inject_gate: InjectGate,
    /// 프로젝트별 최근 임베딩 처리 기록 (완료 시각, 작업 수). (IF-CLI-5)
    embed_log: Mutex<HashMap<String, VecDeque<(Instant, usize)>>>,
    /// 임베딩 처리와 프로젝트 병합을 서로 막는다. 병합이 지운 프로젝트를 임베딩 스레드가 다시 만들지 않게 한다.
    embed_lock: Mutex<()>,
    write_tx: Sender<WriteJob>,
    notify_tx: Sender<()>,
    reindexing: AtomicBool,
    /// 재색인 중인 프로젝트. 이 프로젝트를 다시 열 때는 세대를 정리하지 않는다. (FR-SWP-9)
    reindex_project: Mutex<Option<String>>,
    reindex_state: Mutex<String>,
    last_activity: Mutex<Instant>,
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    m.lock().map_err(|e| e.to_string())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl Shared {
    fn locate(&self, path: &str) -> Location {
        locate(
            Path::new(path),
            Duration::from_millis(self.config.project.git_timeout_ms),
        )
    }

    /// 조회 전용 판정. (FR-PRJ-5)
    fn lookup(&self, path: &str) -> Result<Option<String>, String> {
        let loc = self.locate(path);
        lock(&self.registry)?
            .lookup(&loc)
            .map_err(|e| e.to_string())
    }

    fn project_ids(&self) -> Result<Vec<String>, String> {
        Ok(lock(&self.registry)?
            .projects()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| p.id)
            .collect())
    }

    /// 범위에 해당하는 프로젝트. `Project`/`Path`는 많아야 하나이다.
    fn scope_projects(&self, scope: &Scope, cwd: &str) -> Result<Vec<String>, String> {
        match scope {
            Scope::All => self.project_ids(),
            Scope::Project => Ok(self.lookup(cwd)?.into_iter().collect()),
            Scope::Path(path) => Ok(self.lookup(path)?.into_iter().collect()),
        }
    }

    /// 열려 있고 마지막 정리 뒤 수정된 벡터 저장소를 정리한다. 실패는 로그에 남긴다. (FR-STO-12 (b))
    fn optimize_open(&self) {
        let handles: Vec<(String, Arc<ProjectHandle>)> = match lock(&self.open) {
            Ok(open) => open.entries.iter().cloned().collect(),
            Err(e) => {
                self.log(&e);
                return;
            }
        };
        for (id, handle) in handles {
            if handle.vectors.pending_changes() == 0 {
                continue;
            }
            if let Err(e) = handle.vectors.optimize() {
                self.log(&format!("project {id}: vector optimize failed: {e}"));
            }
        }
    }

    fn records(&self, id: &str) -> Result<RecordStore, String> {
        RecordStore::open(&self.paths.project_records(id)).map_err(|e| e.to_string())
    }

    /// 프로젝트의 벡터 저장소를 연다. 열어 둔 수가 상한을 넘으면 가장 오래 쓰지 않은 것을 닫는다. (FR-DMN-9)
    fn project(&self, id: &str) -> Result<Arc<ProjectHandle>, String> {
        let mut open = lock(&self.open)?;
        if let Some(handle) = open.get(id) {
            return Ok(handle);
        }
        let base = self.paths.project_vectors(id, &self.source.name);
        let active = active_dir(&base).map_err(|e| e.to_string())?;
        let reindexing = lock(&self.reindex_project)?.as_deref() == Some(id);
        if !reindexing {
            for dir in cleanup_inactive(&base).map_err(|e| e.to_string())? {
                self.log(&format!(
                    "removed inactive vector generation {}",
                    dir.display()
                ));
            }
        }
        let vectors = (self.source.open)(&active).map_err(|e| e.to_string())?;
        let compat =
            compatibility(self.embedder.as_ref(), vectors.as_ref()).map_err(|e| e.to_string())?;
        if let Compatibility::Mismatch { .. } = compat {
            self.log(&format!(
                "project {id}: vector store does not match the embedder, keyword search only until `hippo reindex`"
            ));
        }
        let handle = Arc::new(ProjectHandle {
            vectors,
            compat: Mutex::new(compat),
        });
        open.insert(id, Arc::clone(&handle));
        Ok(handle)
    }

    fn install(&self, id: &str, vectors: Arc<dyn VectorStore>) -> Result<(), String> {
        let handle = Arc::new(ProjectHandle {
            vectors,
            compat: Mutex::new(Compatibility::Empty),
        });
        handle.refresh(self.embedder.as_ref());
        lock(&self.open)?.insert(id, handle);
        Ok(())
    }

    /// 프로젝트의 미완료 작업을 메모리 큐에 넣고 임베딩 스레드를 깨운다.
    fn enqueue_pending(&self, id: &str, records: &RecordStore) {
        match (records.pending_jobs(), self.queue.lock()) {
            (Ok(jobs), Ok(mut queue)) => {
                for p in jobs {
                    queue.push(id, p.job, p.bulk);
                }
            }
            (Err(e), _) => self.log(&format!("project {id}: reading embed jobs failed: {e}")),
            (_, Err(e)) => self.log(&format!("embed queue unavailable: {e}")),
        }
        let _ = self.notify_tx.send(());
    }

    /// 시작할 때 모든 프로젝트의 작업을 등록 시각 순으로 큐에 채운다. (FR-DMN-6)
    fn fill_queue(&self) -> Result<(), String> {
        let mut all = Vec::new();
        for id in self.project_ids()? {
            match self
                .records(&id)
                .and_then(|r| r.pending_jobs().map_err(|e| e.to_string()))
            {
                Ok(jobs) => all.extend(
                    jobs.into_iter()
                        .map(|p| (p.enqueued_at, id.clone(), p.job, p.bulk)),
                ),
                Err(e) => self.log(&format!("project {id}: reading embed jobs failed: {e}")),
            }
        }
        all.sort_by_key(|(at, _, _, _)| *at);
        let mut queue = lock(&self.queue)?;
        for (_, id, job, bulk) in all {
            queue.push(&id, job, bulk);
        }
        Ok(())
    }

    /// 사람이 읽는 출력의 언어. 요청마다 설정 파일에서 다시 읽는다. (FR-UI-1)
    fn lang(&self) -> Lang {
        let path = self.paths.config();
        if !path.exists() {
            return self.config.ui.language;
        }
        Config::load(&path).map_or(self.config.ui.language, |c| c.ui.language)
    }

    fn record_embedded(&self, project: &str, jobs: usize) {
        if let Ok(mut log) = self.embed_log.lock() {
            let entries = log.entry(project.to_string()).or_default();
            entries.push_back((Instant::now(), jobs));
            prune_rate_window(entries);
        }
    }

    /// 최근 5분 동안 처리한 작업 수로 구한 분당 속도. 기록이 없으면 `None`. (IF-CLI-5)
    fn embed_per_min(&self, project: &str) -> Option<u64> {
        let mut log = self.embed_log.lock().ok()?;
        let entries = log.get_mut(project)?;
        prune_rate_window(entries);
        let first = entries.front()?.0;
        let jobs: usize = entries.iter().map(|(_, n)| n).sum();
        let elapsed = first.elapsed().max(RATE_MIN_SPAN).as_secs_f64();
        Some((jobs as f64 * 60.0 / elapsed).round() as u64)
    }

    fn chunking(&self) -> ChunkConfig {
        ChunkConfig {
            max_tokens: self.config.chunking.max_tokens,
            chars_per_token: self.config.chunking.chars_per_token,
            overlap_chars: self.config.chunking.overlap_chars,
        }
    }

    /// 검색 질의에서 임베딩할 최대 문자 수. 청크 한도와 같다. (FR-STO-5)
    fn query_chars(&self) -> usize {
        let c = &self.config.chunking;
        chunk_limit(
            self.embedder.max_input_tokens(),
            c.max_tokens,
            c.chars_per_token,
        )
        .get()
    }

    fn set_reindex_state(&self, state: &str) {
        if let Ok(mut s) = self.reindex_state.lock() {
            *s = state.to_string();
        }
    }

    fn touch(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    /// 마지막 요청 후 유휴 시간이 지났고 재색인 중이 아니면 true. (FR-DMN-8)
    fn idle_expired(&self) -> bool {
        let timeout = self.config.daemon.idle_timeout_secs;
        if timeout == 0 || self.reindexing.load(Ordering::SeqCst) {
            return false;
        }
        self.last_activity
            .lock()
            .is_ok_and(|last| last.elapsed() >= Duration::from_secs(timeout))
    }

    fn log(&self, message: &str) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Ok(mut f) = open_private_append(&self.paths.logs().join(LOG_FILE)) {
            let _ = writeln!(f, "{ts} {message}");
        }
    }
}

pub struct DaemonHandle {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    _lock: File,
}

impl DaemonHandle {
    /// 스레드를 멈추고 잠금을 푼다.
    pub fn shutdown(self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for t in self.threads {
            let _ = t.join();
        }
        let _ = fs::remove_file(self.shared.paths.socket());
    }

    /// 데몬이 멈출 때까지 기다린다.
    pub fn wait(self) {
        for t in self.threads {
            let _ = t.join();
        }
    }
}

pub fn start(
    paths: Paths,
    config: Config,
    embedder: Arc<dyn Embedder>,
    source: VectorSource,
) -> Result<DaemonHandle, DaemonError> {
    for dir in [
        paths.root.clone(),
        paths.spool(),
        paths.logs(),
        paths.projects(),
    ] {
        ensure_private_dir(&dir)?;
    }
    let lock_file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock())?;
    set_private_file(&paths.lock())?;
    match lock_file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Err(DaemonError::AlreadyRunning),
        Err(fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let redactor = Redactor::new(&config.redact_patterns()).map_err(|e| DaemonError::Redact {
        kind: e.kind,
        message: e.message,
    })?;
    let registry = Registry::open(&paths.registry())?;
    let socket = paths.socket();
    if fs::symlink_metadata(&socket).is_ok() {
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    set_private_file(&socket)?;
    listener.set_nonblocking(true)?;

    let (write_tx, write_rx) = mpsc::channel();
    let (notify_tx, notify_rx) = mpsc::channel();
    let cap = config.daemon.max_open_projects;
    let shared = Arc::new(Shared {
        paths,
        config,
        embedder,
        source,
        registry: Mutex::new(registry),
        open: Mutex::new(OpenProjects {
            cap,
            entries: VecDeque::new(),
        }),
        queue: Mutex::new(EmbedQueue::default()),
        inject_gate: InjectGate::default(),
        embed_log: Mutex::new(HashMap::new()),
        embed_lock: Mutex::new(()),
        write_tx,
        notify_tx,
        reindexing: AtomicBool::new(false),
        reindex_project: Mutex::new(None),
        reindex_state: Mutex::new(REINDEX_IDLE.to_string()),
        last_activity: Mutex::new(Instant::now()),
        stop: AtomicBool::new(false),
    });
    if let Err(e) = shared.fill_queue() {
        shared.log(&format!("filling embed queue failed: {e}"));
    }
    spawn_warmup(Arc::clone(&shared));

    let threads = vec![
        spawn_writer(Arc::clone(&shared), redactor, write_rx),
        spawn_embed(Arc::clone(&shared), notify_rx),
        spawn_accept(Arc::clone(&shared), listener),
    ];
    Ok(DaemonHandle {
        shared,
        threads,
        _lock: lock_file,
    })
}

/// 임베딩 모델을 미리 메모리에 올린다. 끝을 기다리지 않는다. (FR-DMN-10)
fn spawn_warmup(shared: Arc<Shared>) {
    thread::spawn(move || {
        if let Err(e) = shared.embedder.embed(&[WARMUP_TEXT.to_string()]) {
            shared.log(&format!("embedder warmup failed: {e}"));
        }
    });
}

/// 수집과 프로젝트 가져오기는 이 스레드 하나가 요청 순서대로 처리한다. 시작할 때 스풀을 먼저 비운다. (FR-DMN-6, FR-CAP-6)
fn spawn_writer(shared: Arc<Shared>, redactor: Redactor, rx: Receiver<WriteJob>) -> JoinHandle<()> {
    thread::spawn(move || {
        let options = IngestOptions {
            max_tool_input_chars: shared.config.embed_text.max_tool_input_chars,
            answer_tools: shared.config.embed_text.answer_tools.clone(),
        };
        let run = |req: &IngestRequest| -> Response {
            match ingest_session(&shared, &redactor, &options, req) {
                Ok(()) => Response::Done,
                Err(message) => {
                    shared.log(&format!("ingest {} failed: {message}", req.session_id));
                    Response::Error { message }
                }
            }
        };
        match spool::take_all(&shared.paths.spool()) {
            Ok(files) => {
                for (file, requests) in files {
                    for req in &requests {
                        run(req);
                    }
                    if let Err(e) = spool::finish(&file) {
                        shared.log(&format!("spool cleanup failed: {e}"));
                    }
                }
            }
            Err(e) => shared.log(&format!("spool read failed: {e}")),
        }
        let mut pending = LaneQueue::default();
        while !shared.stop.load(Ordering::SeqCst) {
            let mut disconnected = false;
            if pending.is_empty() {
                match rx.recv_timeout(WRITE_POLL) {
                    Ok(job) => {
                        let lane = job.lane();
                        pending.push(job, lane);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => disconnected = true,
                }
            }
            loop {
                match rx.try_recv() {
                    Ok(job) => {
                        let lane = job.lane();
                        pending.push(job, lane);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            match pending.pop() {
                Some(WriteJob::Ingest(req, reply)) => {
                    let _ = reply.send(run(&req));
                }
                Some(WriteJob::ImportPrepare { from, to, reply }) => {
                    let resp = import_prepare(&shared, &from, &to).unwrap_or_else(|message| {
                        shared.log(&format!("import prepare {from} failed: {message}"));
                        Response::Error { message }
                    });
                    let _ = reply.send(resp);
                }
                Some(WriteJob::ImportSession {
                    project_id,
                    session_id,
                    transcript_path,
                    reply,
                }) => {
                    let resp = import_one(
                        &shared,
                        &redactor,
                        &options,
                        &project_id,
                        &session_id,
                        &transcript_path,
                    )
                    .unwrap_or_else(|message| {
                        shared.log(&format!("import {session_id} failed: {message}"));
                        Response::Error { message }
                    });
                    let _ = reply.send(resp);
                }
                None if disconnected => break,
                None => {}
            }
        }
    })
}

/// 세션이 고정된 프로젝트. 고정값이 없으면 세션 시작 경로로 정해 기록한다.
/// 시작 경로를 아직 알 수 없으면 `None`. (FR-PRJ-1, FR-PRJ-4)
fn session_project(shared: &Shared, req: &IngestRequest) -> Result<Option<String>, String> {
    if let Some(id) = lock(&shared.registry)?
        .session_project(&req.session_id)
        .map_err(|e| e.to_string())?
    {
        return Ok(Some(id));
    }
    let bytes = fs::read(&req.transcript_path).map_err(|e| {
        format!(
            "transcript read failed ({}): {e}",
            req.transcript_path.display()
        )
    })?;
    let Some(cwd) = session_start_cwd(&bytes) else {
        return Ok(None);
    };
    let loc = shared.locate(&cwd);
    let mut registry = lock(&shared.registry)?;
    let id = registry
        .resolve(&loc, now_secs())
        .map_err(|e| e.to_string())?;
    registry
        .bind_session(&req.session_id, &id)
        .map_err(|e| e.to_string())?;
    Ok(Some(id))
}

fn ingest_session(
    shared: &Shared,
    redactor: &Redactor,
    options: &IngestOptions,
    req: &IngestRequest,
) -> Result<(), String> {
    let Some(id) = session_project(shared, req)? else {
        return Ok(());
    };
    let mut records = shared.records(&id)?;
    ingest(&mut records, redactor, options, req).map_err(|e| e.to_string())?;
    lock(&shared.registry)?
        .touch(&id, now_secs())
        .map_err(|e| e.to_string())?;
    shared.enqueue_pending(&id, &records);
    Ok(())
}

/// 가져오기의 원본 경로와 그 프로젝트 위치. `from`이 프로젝트 id이면 그 프로젝트의 위치에서 정한다. (FR-IMP-2)
fn import_origin(shared: &Shared, from: &str) -> Result<(PathBuf, String), String> {
    let row = if is_project_id(from) {
        lock(&shared.registry)?
            .project(from)
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    Ok(match row {
        Some(row) => {
            let location = PathBuf::from(&row.location);
            let path = match location.file_name() {
                Some(name) if name == GIT_DIR_NAME => location.parent().map(Path::to_path_buf),
                _ => None,
            }
            .unwrap_or(location);
            (path, row.location)
        }
        None => (normalize(Path::new(from)), shared.locate(from).path),
    })
}

/// 원본·대상 프로젝트와 대상 위치를 정한다. 목록 DB를 바꾸지 않는다. (FR-PRJ-7, FR-IMP-9)
type ImportTargets = (Option<String>, Option<String>, Location);

fn import_targets(shared: &Shared, from: &str, to: &str) -> Result<ImportTargets, String> {
    let to_loc = shared.locate(to);
    let found = lock(&shared.registry)?
        .find_source(from)
        .map_err(|e| e.to_string())?;
    let source = match found {
        Some(id) => Some(id),
        None if is_project_id(from) => None,
        None => {
            let from_loc = shared.locate(from);
            lock(&shared.registry)?
                .lookup(&from_loc)
                .map_err(|e| e.to_string())?
        }
    };
    let target = lock(&shared.registry)?
        .lookup(&to_loc)
        .map_err(|e| e.to_string())?;
    Ok((source, target, to_loc))
}

/// 원본·대상 프로젝트를 정해 위치를 바꾸거나 병합한다. 후보 세션은 연결 스레드가 찾는다. (FR-PRJ-7)
fn import_prepare(shared: &Shared, from: &str, to: &str) -> Result<Response, String> {
    if shared.reindexing.load(Ordering::SeqCst) {
        return Err(reindex_busy(shared.lang()));
    }
    let lang = shared.lang();
    let (source, target, to_loc) = import_targets(shared, from, to)?;
    let (project_id, summary) = match (source, target) {
        (None, None) => {
            let id = lock(&shared.registry)?
                .resolve(&to_loc, now_secs())
                .map_err(|e| e.to_string())?;
            let path = &to_loc.path;
            let text = tr!(
                lang,
                "Created project {id} at {path}.\n",
                "프로젝트 {id}을(를) {path}에 만들었습니다.\n"
            );
            (id, text)
        }
        (Some(source), None) => {
            lock(&shared.registry)?
                .relocate(&source, &to_loc)
                .map_err(|e| e.to_string())?;
            let path = &to_loc.path;
            let text = tr!(
                lang,
                "Moved project {source} to {path}.\n",
                "프로젝트 {source}의 위치를 {path}(으)로 바꿨습니다.\n"
            );
            (source, text)
        }
        (Some(source), Some(target)) if source != target => {
            let text = merge(shared, &source, &target)?;
            (target, text)
        }
        (_, Some(target)) => (target, tr!(lang, "No change\n", "변경 없음\n")),
    };
    Ok(Response::ImportPlan {
        project_id,
        source_id: None,
        summary,
        sessions: Vec::new(),
    })
}

/// 쓰기 스레드에서 프로젝트를 정한 뒤, 이 스레드에서 transcript를 훑어 후보를 붙인다. (FR-IMP-1, FR-IMP-2)
fn import_plan(shared: &Shared, from: String, to: String) -> Result<Response, String> {
    let (from_path, from_location) = import_origin(shared, &from)?;
    let resp = send_write(
        shared,
        |reply| WriteJob::ImportPrepare { from, to, reply },
        "import",
    );
    let Response::ImportPlan {
        project_id,
        summary,
        ..
    } = resp
    else {
        return Ok(resp);
    };
    let sessions = annotated_candidates(shared, &from_path, &from_location, &[&project_id])?;
    Ok(Response::ImportPlan {
        project_id,
        source_id: None,
        summary,
        sessions,
    })
}

/// 실행했을 때의 처리 경우와 후보를 돌려준다. 목록 DB와 원본 저장소를 바꾸지 않는다. (FR-IMP-9)
fn import_preview(shared: &Shared, from: &str, to: &str) -> Result<Response, String> {
    let (from_path, from_location) = import_origin(shared, from)?;
    let (source, target, to_loc) = import_targets(shared, from, to)?;
    let lang = shared.lang();
    let path = &to_loc.path;
    let (project_id, summary, owners) = match (&source, &target) {
        (None, None) => (
            String::new(),
            tr!(
                lang,
                "Would create a project at {path}.\n",
                "실행하면 {path}에 새 프로젝트를 만듭니다.\n"
            ),
            Vec::new(),
        ),
        (Some(s), None) => (
            s.clone(),
            tr!(
                lang,
                "Would move project {s} to {path}.\n",
                "실행하면 프로젝트 {s}의 위치를 {path}(으)로 바꿉니다.\n"
            ),
            vec![s.as_str()],
        ),
        (Some(s), Some(t)) if s != t => (
            t.clone(),
            tr!(
                lang,
                "Would merge project {s} into {t}.\n",
                "실행하면 프로젝트 {s}을(를) {t}에 병합합니다.\n"
            ),
            vec![t.as_str(), s.as_str()],
        ),
        (_, Some(t)) => (
            t.clone(),
            tr!(lang, "No change\n", "변경 없음\n"),
            vec![t.as_str()],
        ),
    };
    let sessions = annotated_candidates(shared, &from_path, &from_location, &owners)?;
    Ok(Response::ImportPlan {
        project_id,
        source_id: source.clone(),
        summary,
        sessions,
    })
}

/// 후보를 찾고, `owners`를 대상 프로젝트로 보아 가져오기 상태를 붙인다. (FR-IMP-8)
fn annotated_candidates(
    shared: &Shared,
    from_path: &Path,
    from_location: &str,
    owners: &[&str],
) -> Result<Vec<Candidate>, String> {
    let redactor = Redactor::new(&shared.config.redact_patterns()).map_err(|e| format!("{e:?}"))?;
    let locate = |p: &Path| shared.locate(&p.to_string_lossy());
    let redact = |text: &str| redactor.redact(text);
    let mut sessions = candidates(
        &shared.config.import.transcripts_dir,
        from_path,
        from_location,
        &locate,
        &redact,
    );
    let stores: Vec<RecordStore> = owners
        .iter()
        .filter_map(|id| shared.records(id).ok())
        .collect();
    for c in &mut sessions {
        let pin = lock(&shared.registry)?
            .session_project(&c.session_id)
            .map_err(|e| e.to_string())?;
        let state = stores
            .iter()
            .find_map(|r| r.session_state(&c.session_id).ok().flatten());
        c.status = import_status(pin.as_deref(), owners, state.as_ref());
    }
    Ok(sessions)
}

/// 세션 하나를 대상 프로젝트로 가져온다. 다른 프로젝트에 고정된 세션은 건너뛴다. (FR-IMP-3, FR-IMP-7)
fn import_one(
    shared: &Shared,
    redactor: &Redactor,
    options: &IngestOptions,
    project_id: &str,
    session_id: &str,
    transcript_path: &Path,
) -> Result<Response, String> {
    if shared.reindexing.load(Ordering::SeqCst) {
        return Err(reindex_busy(shared.lang()));
    }
    let bound = {
        let mut registry = lock(&shared.registry)?;
        if registry
            .project(project_id)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err(tr!(
                shared.lang(),
                "Project not found: {project_id}",
                "프로젝트가 없습니다: {project_id}"
            ));
        }
        let bound = registry
            .session_project(session_id)
            .map_err(|e| e.to_string())?;
        if bound.is_none() {
            registry
                .bind_session(session_id, project_id)
                .map_err(|e| e.to_string())?;
        }
        bound
    };
    let mut records = shared.records(project_id)?;
    let report = match bound {
        Some(other) if other != project_id => ImportReport {
            outcome: ImportOutcome::OtherProject,
            iterations: 0,
            completions: 0,
            fork_skipped: 0,
        },
        _ => import_session(&mut records, redactor, options, session_id, transcript_path)
            .map_err(|e| e.to_string())?,
    };
    shared.enqueue_pending(project_id, &records);
    let pending_embeds = records.counts().map_err(|e| e.to_string())?.embed_jobs;
    Ok(Response::ImportSession(ImportSessionReport {
        outcome: report.outcome,
        iterations: report.iterations,
        completions: report.completions,
        fork_skipped: report.fork_skipped,
        pending_embeds,
    }))
}

/// 원본의 기록을 대상으로 병합하고 원본을 지운다. (FR-PRJ-9)
fn merge(shared: &Shared, source: &str, target: &str) -> Result<String, String> {
    let lang = shared.lang();
    let _embedding = lock(&shared.embed_lock)?;
    let src = shared.project(source)?;
    let dst = shared.project(target)?;
    shared.records(source)?;
    let copy = !src.is_mismatch() && !dst.is_mismatch();
    let mut records = shared.records(target)?;
    let report = records
        .merge_from(&shared.paths.project_records(source), !copy)
        .map_err(|e| e.to_string())?;
    let copied = if copy {
        let vectors = src.vectors.scan().map_err(|e| e.to_string())?;
        dst.vectors.upsert(&vectors).map_err(|e| e.to_string())?;
        dst.refresh(shared.embedder.as_ref());
        vectors.len()
    } else {
        0
    };
    lock(&shared.registry)?
        .absorb(source, target)
        .map_err(|e| e.to_string())?;
    lock(&shared.queue)?.remove_project(source);
    lock(&shared.open)?.remove(source);
    drop(src);
    fs::remove_dir_all(shared.paths.project_dir(source)).map_err(|e| e.to_string())?;
    shared.enqueue_pending(target, &records);
    let vectors = if copy {
        tr!(
            lang,
            "Copied {copied} vectors.",
            "벡터 {copied}개를 복사했습니다."
        )
    } else {
        let jobs = report.jobs;
        tr!(
            lang,
            "The embedding model differs; queued {jobs} embedding jobs.",
            "모델이 달라 임베딩 작업 {jobs}개를 등록했습니다."
        )
    };
    Ok(tr!(
        lang,
        "Merged project {source} into {target}: {} iterations, {} results. {vectors}\n",
        "프로젝트 {source}의 기록을 {target}(으)로 가져왔습니다. 반복 {}개, 결과 {}개. {vectors}\n",
        report.iterations,
        report.results
    ))
}

enum EmbedFailure {
    Incompatible,
    Failed(String),
}

/// 임베딩 스레드. 메모리 큐 앞에서부터 같은 프로젝트의 작업을 묶어 처리한다.
/// 재색인 중이면 처리하지 않는다. (FR-STO-7, FR-DMN-6, FR-SWP-5)
fn spawn_embed(shared: Arc<Shared>, notify: Receiver<()>) -> JoinHandle<()> {
    thread::spawn(move || {
        let batch = shared.config.index.job_batch;
        let mut next_index_at: HashMap<String, u64> = HashMap::new();
        while !shared.stop.load(Ordering::SeqCst) {
            let mut wait = EMBED_IDLE;
            loop {
                if shared.stop.load(Ordering::SeqCst) || shared.reindexing.load(Ordering::SeqCst) {
                    break;
                }
                let Some((project, jobs, from_bulk)) = shared.queue.lock().ok().and_then(|q| {
                    let from_bulk = q.lane_counts().0 == 0;
                    q.front_batch(batch).map(|(p, j)| (p, j, from_bulk))
                }) else {
                    shared.optimize_open();
                    break;
                };
                match embed_batch(&shared, &project, &jobs, from_bulk, &mut next_index_at) {
                    Ok(done) => {
                        shared.record_embedded(&project, done);
                        if let Ok(mut q) = shared.queue.lock() {
                            q.remove(&project, &jobs[..done]);
                        }
                    }
                    Err(EmbedFailure::Incompatible) => {
                        if let Ok(mut q) = shared.queue.lock() {
                            q.remove_project(&project);
                        }
                    }
                    Err(EmbedFailure::Failed(message)) => {
                        shared.log(&format!("embedding failed: {message}"));
                        wait = EMBED_RETRY;
                        break;
                    }
                }
            }
            let deadline = Instant::now() + wait;
            while Instant::now() < deadline && !shared.stop.load(Ordering::SeqCst) {
                match notify.recv_timeout(WRITE_POLL) {
                    Ok(()) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            while notify.try_recv().is_ok() {}
        }
    })
}

/// 처리한 작업 수를 돌려준다. `bulk` 작업을 처리하는 중에 일반 작업이 들어오면 작업 1건 뒤 멈춘다. (FR-DMN-6 (b))
fn embed_batch(
    shared: &Shared,
    project: &str,
    jobs: &[EmbedJob],
    from_bulk: bool,
    next_index_at: &mut HashMap<String, u64>,
) -> Result<usize, EmbedFailure> {
    let failed = EmbedFailure::Failed;
    let _embedding = lock(&shared.embed_lock).map_err(failed)?;
    let handle = shared.project(project).map_err(failed)?;
    if handle.is_mismatch() {
        return Err(EmbedFailure::Incompatible);
    }
    let mut records = shared.records(project).map_err(failed)?;
    let embedder = YieldingEmbedder {
        inner: shared.embedder.as_ref(),
        gate: &shared.inject_gate,
    };
    let live_waiting = || from_bulk && shared.queue.lock().is_ok_and(|q| q.lane_counts().0 > 0);
    let done = process_jobs_until(
        &mut records,
        &embedder,
        handle.vectors.as_ref(),
        shared.chunking(),
        jobs,
        &live_waiting,
    )
    .map_err(|e| failed(e.to_string()))?;
    if let Err(e) = optimize_if_due(handle.vectors.as_ref(), shared.config.index.optimize_every) {
        shared.log(&format!("project {project}: vector optimize failed: {e}"));
    }
    handle.refresh(shared.embedder.as_ref());
    let threshold = shared.config.index.vector_index_threshold;
    let next = next_index_at
        .entry(project.to_string())
        .or_insert(threshold);
    if let Ok(count) = handle.vectors.count()
        && count >= *next
    {
        match handle.vectors.ensure_index() {
            Ok(()) => *next = count.saturating_mul(2),
            Err(e) => shared.log(&format!("project {project}: vector index failed: {e}")),
        }
    }
    Ok(done)
}

/// 범위의 프로젝트마다 새 세대에 전체를 다시 색인하고 ACTIVE를 바꿔 전환한다. (FR-SWP-5)
fn start_reindex(shared: &Arc<Shared>, scope: &Scope, cwd: &str) -> Response {
    let targets = match shared.scope_projects(scope, cwd) {
        Ok(t) if t.is_empty() => {
            return Response::Error {
                message: no_project(shared.lang()).trim_end().into(),
            };
        }
        Ok(t) => t,
        Err(message) => return Response::Error { message },
    };
    if shared.reindexing.swap(true, Ordering::SeqCst) {
        return Response::Error {
            message: "reindex already running".into(),
        };
    }
    shared.set_reindex_state(REINDEX_RUNNING);
    let shared = Arc::clone(shared);
    thread::spawn(move || {
        let mut failures = Vec::new();
        for id in &targets {
            if let Ok(mut current) = shared.reindex_project.lock() {
                *current = Some(id.clone());
            }
            if let Err(message) = reindex_project(&shared, id) {
                shared.log(&format!("reindex {id} failed: {message}"));
                failures.push(format!("{id}: {message}"));
            }
        }
        if let Ok(mut current) = shared.reindex_project.lock() {
            *current = None;
        }
        if failures.is_empty() {
            shared.set_reindex_state(REINDEX_DONE);
            shared.log("reindex done");
        } else {
            shared.set_reindex_state(&format!("failed: {}", failures.join("; ")));
        }
        shared.reindexing.store(false, Ordering::SeqCst);
        for id in &targets {
            match shared.records(id) {
                Ok(records) => shared.enqueue_pending(id, &records),
                Err(e) => shared.log(&format!("project {id}: {e}")),
            }
        }
    });
    Response::Done
}

fn reindex_project(shared: &Shared, id: &str) -> Result<(), String> {
    let base = shared.paths.project_vectors(id, &shared.source.name);
    let generation = new_generation(&base).map_err(|e| e.to_string())?;
    let target = (shared.source.open)(&generation).map_err(|e| e.to_string())?;
    let records = shared.records(id)?;
    reindex(
        &records,
        shared.embedder.as_ref(),
        target.as_ref(),
        shared.chunking(),
        REINDEX_PAGE,
        shared.config.index.optimize_every,
    )
    .map_err(|e| e.to_string())?;
    activate(&base, &generation).map_err(|e| e.to_string())?;
    shared.install(id, target)
}

/// 새 연결을 받지 않게 하고 소켓 파일을 지운다. 스레드는 처리 중인 작업을 마친 뒤 멈춘다. (FR-DMN-11)
fn request_shutdown(shared: &Shared) -> Result<Response, String> {
    if shared.reindexing.load(Ordering::SeqCst) {
        return Err(shutdown_busy(shared.lang()));
    }
    shared.log("shutdown requested");
    shared.stop.store(true, Ordering::SeqCst);
    let _ = fs::remove_file(shared.paths.socket());
    Ok(Response::Done)
}

fn spawn_accept(shared: Arc<Shared>, listener: UnixListener) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shared.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let shared = Arc::clone(&shared);
                    thread::spawn(move || handle_connection(&shared, stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if shared.idle_expired() {
                        shared.log("idle timeout, shutting down");
                        shared.stop.store(true, Ordering::SeqCst);
                        let _ = fs::remove_file(shared.paths.socket());
                        break;
                    }
                    thread::sleep(POLL);
                }
                Err(e) => {
                    shared.log(&format!("accept failed: {e}"));
                    thread::sleep(POLL);
                }
            }
        }
    })
}

fn handle_connection(shared: &Arc<Shared>, stream: UnixStream) {
    shared.touch();
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(CONNECTION_TIMEOUT)).is_err()
    {
        return;
    }
    let mut line = String::new();
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    if BufReader::new(reader_stream).read_line(&mut line).is_err() {
        return;
    }
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(req) => handle_request(shared, req),
        Err(e) => Response::Error {
            message: format!("invalid request: {e}"),
        },
    };
    if let Ok(mut out) = serde_json::to_string(&response) {
        out.push('\n');
        let mut stream = stream;
        let _ = stream.write_all(out.as_bytes());
    }
    shared.touch();
}

/// 쓰기 스레드에 작업을 넘기고 응답을 기다린다.
fn send_write(shared: &Shared, make: impl FnOnce(Reply) -> WriteJob, what: &str) -> Response {
    let (tx, rx) = mpsc::channel();
    if shared.write_tx.send(make(tx)).is_err() {
        return Response::Error {
            message: "write worker stopped".into(),
        };
    }
    rx.recv_timeout(CONNECTION_TIMEOUT)
        .unwrap_or_else(|_| Response::Error {
            message: format!("{what} timed out"),
        })
}

fn handle_request(shared: &Arc<Shared>, req: Request) -> Response {
    let result: Result<Response, String> = match req {
        Request::Ingest(ingest_req) => {
            return send_write(shared, |tx| WriteJob::Ingest(ingest_req, tx), "ingest");
        }
        Request::ImportPrepare { from, to, dry_run } => {
            if dry_run {
                import_preview(shared, &from, &to)
            } else {
                import_plan(shared, from, to)
            }
        }
        Request::ImportSession {
            project_id,
            session_id,
            transcript_path,
        } => {
            return send_write(
                shared,
                |reply| WriteJob::ImportSession {
                    project_id,
                    session_id,
                    transcript_path,
                    reply,
                },
                "import",
            );
        }
        Request::Reindex { scope, cwd } => return start_reindex(shared, &scope, &cwd),
        Request::Inject {
            prompt,
            cwd,
            session_id,
            budget_ms,
        } => {
            let budget_ms = budget_ms.unwrap_or(shared.config.inject.budget_ms);
            inject(shared, &prompt, &cwd, &session_id, budget_ms)
                .map(|context| Response::Inject { context })
        }
        Request::Search {
            query,
            kind,
            limit,
            scope,
            cwd,
        } => search_scope(shared, query, kind, limit, &scope, &cwd)
            .map(|text| Response::Text { text }),
        Request::Show {
            prompt_id,
            with_results,
            scope,
            cwd,
        } => show(shared, &prompt_id, with_results, scope.as_ref(), &cwd)
            .map(|text| Response::Text { text }),
        Request::Status => status(shared).map(Response::Status),
        Request::ProjectList => project_list(shared).map(|text| Response::Text { text }),
        Request::Shutdown => request_shutdown(shared),
    };
    result.unwrap_or_else(|message| {
        shared.log(&format!("request failed: {message}"));
        Response::Error { message }
    })
}

/// 현재 세션의 프로젝트에서만 찾는다. 프로젝트가 없으면 주입하지 않는다. (FR-RET-1)
/// 처리 시간이 예산 이상이면 로그를 남긴다. (FR-RET-10)
fn inject(
    shared: &Shared,
    prompt: &str,
    cwd: &str,
    session_id: &str,
    budget_ms: u64,
) -> Result<Option<String>, String> {
    let started = Instant::now();
    let _active = shared.inject_gate.enter();
    let cold_ms = shared.config.inject.cold_budget_ms;
    let budget_ms = if budget_ms < cold_ms && !shared.embedder.is_loaded() {
        cold_ms
    } else {
        budget_ms
    };
    let bound = lock(&shared.registry)?
        .session_project(session_id)
        .map_err(|e| e.to_string())?;
    let Some(id) = bound.map_or_else(|| shared.lookup(cwd), |id| Ok(Some(id)))? else {
        return Ok(None);
    };
    let handle = shared.project(&id)?;
    let records = shared.records(&id)?;
    let c = &shared.config.inject;
    let config = InjectConfig {
        threshold: c.threshold,
        top_k: c.top_k,
        total_chars: c.total_chars,
        min_prompt_chars: c.min_prompt_chars,
    };
    let budget = Duration::from_millis(budget_ms);
    let deadline = Instant::now() + budget;
    let result = build_injection(
        &records,
        handle.backends(
            shared.embedder.as_ref(),
            shared.query_chars(),
            &shared.config.embedder.query_prefix,
        ),
        prompt,
        session_id,
        config,
        deadline,
    )
    .map_err(|e| e.to_string());
    let elapsed = started.elapsed();
    if elapsed >= budget {
        let (normal, bulk) = lock(&shared.queue)?.lane_counts();
        shared.log(&format!(
            "project {id}: inject budget exceeded: elapsed={}ms budget={}ms queue normal={normal} bulk={bulk}",
            elapsed.as_millis(),
            budget_ms
        ));
    }
    result
}

fn search_scope(
    shared: &Shared,
    query: String,
    kind: SearchKind,
    limit: usize,
    scope: &Scope,
    cwd: &str,
) -> Result<String, String> {
    let req = SearchRequest {
        query,
        kind,
        exclude_session: None,
        limit,
    };
    let ids = shared.scope_projects(scope, cwd)?;
    if *scope != Scope::All {
        let Some(id) = ids.first() else {
            return Ok(no_project(shared.lang()));
        };
        let handle = shared.project(id)?;
        let records = shared.records(id)?;
        return search_with_fallback(
            shared,
            &records,
            handle.backends(
                shared.embedder.as_ref(),
                shared.query_chars(),
                &shared.config.embedder.query_prefix,
            ),
            &req,
        );
    }
    let mut opened = Vec::new();
    for id in ids {
        let handle = shared.project(&id)?;
        let records = shared.records(&id)?;
        opened.push((id, records, handle));
    }
    let projects: Vec<ProjectSearch> = opened
        .iter()
        .map(|(id, records, handle)| ProjectSearch {
            id,
            records,
            backends: handle.backends(
                shared.embedder.as_ref(),
                shared.query_chars(),
                &shared.config.embedder.query_prefix,
            ),
        })
        .collect();
    let result = search_projects(&projects, &req).map_err(|e| e.to_string())?;
    for (id, reason) in &result.fallbacks {
        shared.log(&format!(
            "project {id}: vector search failed, falling back to keyword search: {reason}"
        ));
    }
    render_project_hits(&projects, &result, shared.lang()).map_err(|e| e.to_string())
}

/// 임베딩이나 벡터 검색이 실패하면 FTS 결과만으로 응답하고 첫 줄에 안내한다. (FR-RET-8)
fn search_with_fallback(
    shared: &Shared,
    records: &RecordStore,
    backends: Option<VectorBackends>,
    req: &SearchRequest,
) -> Result<String, String> {
    match search(records, backends, req, None) {
        Ok(hits) => render_hits(records, &hits, shared.lang()).map_err(|e| e.to_string()),
        Err(e @ (SearchError::Embed(_) | SearchError::Vector(_))) => {
            shared.log(&format!(
                "vector search failed, falling back to keyword search: {e}"
            ));
            let hits = search(records, None, req, None).map_err(|e| e.to_string())?;
            let lang = shared.lang();
            let body = render_hits(records, &hits, lang).map_err(|e| e.to_string())?;
            Ok(tr!(
                lang,
                "(vector search unavailable, showing keyword results only: {e})\n{body}",
                "(벡터 검색을 쓰지 못해 키워드 검색 결과만 표시합니다: {e})\n{body}"
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `scope`가 없으면 현재 프로젝트를 먼저, 그다음 나머지를 최근 활동 순으로 찾는다. (FR-RET-7)
fn show(
    shared: &Shared,
    prompt_id: &str,
    with_results: bool,
    scope: Option<&Scope>,
    cwd: &str,
) -> Result<String, String> {
    let candidates = match scope {
        Some(scope @ (Scope::Project | Scope::Path(_))) => shared.scope_projects(scope, cwd)?,
        None | Some(Scope::All) => {
            let mut ids: Vec<String> = shared.lookup(cwd)?.into_iter().collect();
            for id in shared.project_ids()? {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        }
    };
    for id in candidates {
        let records = shared.records(&id)?;
        let found = !records
            .iterations_for_prompt(prompt_id)
            .map_err(|e| e.to_string())?
            .is_empty();
        if found {
            return render_turn(&records, prompt_id, with_results, shared.lang())
                .map_err(|e| e.to_string());
        }
    }
    Ok(missing_turn(prompt_id, shared.lang()))
}

fn status(shared: &Shared) -> Result<StatusInfo, String> {
    let rows = lock(&shared.registry)?
        .projects()
        .map_err(|e| e.to_string())?;
    let mut projects = Vec::new();
    for row in rows {
        let handle = shared.project(&row.id)?;
        let counts = shared
            .records(&row.id)?
            .counts()
            .map_err(|e| e.to_string())?;
        let compatibility = match handle.compat() {
            Compatibility::Empty => "empty",
            Compatibility::Compatible => "compatible",
            Compatibility::Mismatch { .. } => COMPAT_MISMATCH,
        };
        projects.push(ProjectStatus {
            exists: Path::new(&row.location).exists(),
            embed_per_min: shared.embed_per_min(&row.id),
            id: row.id,
            location: row.location,
            iterations: counts.iterations,
            results: counts.results,
            embed_jobs: counts.embed_jobs,
            vectors: handle.vectors.count().map_err(|e| e.to_string())?,
            compatibility: compatibility.to_string(),
        });
    }
    Ok(StatusInfo {
        spool_pending: spool::pending_count(&shared.paths.spool()),
        embed_model: shared.embedder.model_id().to_string(),
        reindex: shared
            .reindex_state
            .lock()
            .map_or_else(|_| REINDEX_IDLE.to_string(), |s| s.clone()),
        projects,
    })
}

/// `hippo project list` 출력. (FR-PRJ-6)
fn project_list(shared: &Shared) -> Result<String, String> {
    let rows = lock(&shared.registry)?
        .projects()
        .map_err(|e| e.to_string())?;
    let lang = shared.lang();
    if rows.is_empty() {
        return Ok(no_project(lang));
    }
    let mut out = String::new();
    for row in rows {
        let iterations = shared
            .records(&row.id)?
            .counts()
            .map_err(|e| e.to_string())?
            .iterations;
        let missing = if Path::new(&row.location).exists() {
            String::new()
        } else {
            missing_location(lang)
        };
        out.push_str(&format!(
            "{}  {}  {}{missing}  iterations={iterations}  last_active={}\n",
            row.id,
            row.kind.as_str(),
            row.location,
            local_time(row.last_active_at)
        ));
    }
    Ok(escape_control(&out))
}
