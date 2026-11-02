//! 상주 데몬. 수집 스레드, 임베딩 스레드, 연결 처리 스레드로 구성한다. (SRS 3.2.7)

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::backends::VectorSource;
use crate::config::{Config, Paths};
use crate::embedder::Embedder;
use crate::fsutil::{ensure_private_dir, open_private_append, set_private_file};
use crate::generations::{activate, active_dir, cleanup_inactive, new_generation};
use crate::indexer::{ChunkConfig, Compatibility, compatibility, process_embed_jobs, reindex};
use crate::ingest::{IngestOptions, IngestRequest, ingest};
use crate::protocol::{COMPAT_MISMATCH, REINDEX_RUNNING, Request, Response, StatusInfo};
use crate::redact::Redactor;
use crate::search::{
    InjectConfig, VectorBackends, build_injection, render_hits, render_turn, search,
};
use crate::spool;
use crate::store::RecordStore;
use crate::vector::{VectorError, VectorStore};

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("daemon already running")]
    AlreadyRunning,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid redact pattern {kind}: {message}")]
    Redact { kind: String, message: String },
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    #[error(transparent)]
    Index(#[from] crate::indexer::IndexError),
    #[error(transparent)]
    Vector(#[from] VectorError),
}

const POLL: Duration = Duration::from_millis(20);
const INGEST_POLL: Duration = Duration::from_millis(100);
const EMBED_IDLE: Duration = Duration::from_secs(1);
const EMBED_RETRY: Duration = Duration::from_secs(5);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const REINDEX_PAGE: usize = 256;
const LOG_FILE: &str = "hippo.log";
const REINDEX_IDLE: &str = "idle";
const REINDEX_DONE: &str = "done";

type IngestJob = (IngestRequest, Sender<Response>);

struct Shared {
    paths: Paths,
    config: Config,
    embedder: Arc<dyn Embedder>,
    source: VectorSource,
    vectors: RwLock<Arc<dyn VectorStore>>,
    compat: Mutex<Compatibility>,
    ingest_tx: Sender<IngestJob>,
    notify_tx: Sender<()>,
    reindexing: AtomicBool,
    reindex_state: Mutex<String>,
    stop: AtomicBool,
}

impl Shared {
    fn vectors(&self) -> Option<Arc<dyn VectorStore>> {
        self.vectors.read().ok().map(|v| Arc::clone(&v))
    }

    fn compat(&self) -> Compatibility {
        self.compat.lock().map_or(
            Compatibility::Mismatch {
                store_model: None,
                store_dimension: None,
            },
            |c| c.clone(),
        )
    }

    /// 호환되지 않으면 벡터 검색을 끄고 FTS 전용으로 동작한다. (FR-SWP-6)
    fn backends<'a>(&'a self, vectors: &'a dyn VectorStore) -> Option<VectorBackends<'a>> {
        match self.compat() {
            Compatibility::Mismatch { .. } => None,
            Compatibility::Empty | Compatibility::Compatible => Some(VectorBackends {
                embedder: self.embedder.as_ref(),
                vectors,
            }),
        }
    }

    fn refresh_compat(&self) {
        let Some(vectors) = self.vectors() else {
            return;
        };
        if let Ok(c) = compatibility(self.embedder.as_ref(), vectors.as_ref())
            && let Ok(mut current) = self.compat.lock()
        {
            *current = c;
        }
    }

    fn chunking(&self) -> ChunkConfig {
        ChunkConfig {
            max_tokens: self.config.chunking.max_tokens,
            chars_per_token: self.config.chunking.chars_per_token,
        }
    }

    fn set_reindex_state(&self, state: &str) {
        if let Ok(mut s) = self.reindex_state.lock() {
            *s = state.to_string();
        }
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
    for dir in [paths.root.clone(), paths.spool(), paths.logs()] {
        ensure_private_dir(&dir)?;
    }
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Err(DaemonError::AlreadyRunning),
        Err(fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let redactor = Redactor::new(&config.redact_patterns()).map_err(|e| DaemonError::Redact {
        kind: e.kind,
        message: e.message,
    })?;
    let active = active_dir(&source.base)?;
    let removed = cleanup_inactive(&source.base)?;
    let vectors = (source.open)(&active)?;
    let socket = paths.socket();
    if fs::symlink_metadata(&socket).is_ok() {
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    set_private_file(&socket)?;
    listener.set_nonblocking(true)?;

    let compat = compatibility(embedder.as_ref(), vectors.as_ref())?;
    let (ingest_tx, ingest_rx) = mpsc::channel();
    let (notify_tx, notify_rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        paths,
        config,
        embedder,
        source,
        vectors: RwLock::new(vectors),
        compat: Mutex::new(compat),
        ingest_tx,
        notify_tx,
        reindexing: AtomicBool::new(false),
        reindex_state: Mutex::new(REINDEX_IDLE.to_string()),
        stop: AtomicBool::new(false),
    });
    for dir in removed {
        shared.log(&format!(
            "removed inactive vector generation {}",
            dir.display()
        ));
    }
    let ingest_store = RecordStore::open(&shared.paths.records())?;
    let embed_store = RecordStore::open(&shared.paths.records())?;

    let threads = vec![
        spawn_ingest(Arc::clone(&shared), ingest_store, redactor, ingest_rx),
        spawn_embed(Arc::clone(&shared), embed_store, notify_rx),
        spawn_accept(Arc::clone(&shared), listener),
    ];
    Ok(DaemonHandle {
        shared,
        threads,
        _lock: lock,
    })
}

/// 수집은 이 스레드 하나가 요청 순서대로 처리한다. 시작할 때 스풀을 먼저 비운다. (FR-DMN-6, FR-CAP-6)
fn spawn_ingest(
    shared: Arc<Shared>,
    mut store: RecordStore,
    redactor: Redactor,
    rx: Receiver<IngestJob>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let options = IngestOptions {
            max_tool_input_chars: shared.config.embed_text.max_tool_input_chars,
        };
        let mut run = |req: &IngestRequest| -> Response {
            let resp = match ingest(&mut store, &redactor, options, req) {
                Ok(_) => Response::Done,
                Err(e) => {
                    shared.log(&format!("ingest {} failed: {e}", req.session_id));
                    Response::Error {
                        message: e.to_string(),
                    }
                }
            };
            let _ = shared.notify_tx.send(());
            resp
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
        while !shared.stop.load(Ordering::SeqCst) {
            match rx.recv_timeout(INGEST_POLL) {
                Ok((req, reply)) => {
                    let _ = reply.send(run(&req));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

/// 임베딩 작업 스레드. 수집이 끝날 때마다 깨어나고, 실패하면 일정 시간 뒤 재시도한다.
/// 호환되지 않거나 재색인 중이면 작업을 처리하지 않는다. (FR-STO-7, FR-DMN-6, FR-SWP-5)
fn spawn_embed(
    shared: Arc<Shared>,
    mut store: RecordStore,
    notify: Receiver<()>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let chunking = shared.chunking();
        let batch = shared.config.index.job_batch;
        let threshold = shared.config.index.vector_index_threshold;
        let mut next_index_at = threshold;
        while !shared.stop.load(Ordering::SeqCst) {
            let mut wait = EMBED_IDLE;
            let paused = shared.reindexing.load(Ordering::SeqCst)
                || matches!(shared.compat(), Compatibility::Mismatch { .. });
            if let (false, Some(vectors)) = (paused, shared.vectors()) {
                loop {
                    if shared.stop.load(Ordering::SeqCst)
                        || shared.reindexing.load(Ordering::SeqCst)
                    {
                        break;
                    }
                    match process_embed_jobs(
                        &mut store,
                        shared.embedder.as_ref(),
                        vectors.as_ref(),
                        chunking,
                        batch,
                    ) {
                        Ok(0) => break,
                        Ok(_) => shared.refresh_compat(),
                        Err(e) => {
                            shared.log(&format!("embedding failed: {e}"));
                            wait = EMBED_RETRY;
                            break;
                        }
                    }
                }
                if let Ok(count) = vectors.count()
                    && count >= next_index_at
                {
                    match vectors.ensure_index() {
                        Ok(()) => next_index_at = count.saturating_mul(2),
                        Err(e) => shared.log(&format!("vector index failed: {e}")),
                    }
                }
            }
            let deadline = Instant::now() + wait;
            while Instant::now() < deadline && !shared.stop.load(Ordering::SeqCst) {
                match notify.recv_timeout(INGEST_POLL) {
                    Ok(()) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            while notify.try_recv().is_ok() {}
        }
    })
}

/// 새 세대에 전체를 다시 색인하고 ACTIVE를 바꿔 전환한다. (FR-SWP-5)
fn start_reindex(shared: &Arc<Shared>) -> Response {
    if shared.reindexing.swap(true, Ordering::SeqCst) {
        return Response::Error {
            message: "reindex already running".into(),
        };
    }
    shared.set_reindex_state(REINDEX_RUNNING);
    let shared = Arc::clone(shared);
    thread::spawn(move || {
        let outcome = (|| -> Result<Arc<dyn VectorStore>, String> {
            let generation = new_generation(&shared.source.base).map_err(|e| e.to_string())?;
            let target = (shared.source.open)(&generation).map_err(|e| e.to_string())?;
            let records = RecordStore::open(&shared.paths.records()).map_err(|e| e.to_string())?;
            reindex(
                &records,
                shared.embedder.as_ref(),
                target.as_ref(),
                shared.chunking(),
                REINDEX_PAGE,
            )
            .map_err(|e| e.to_string())?;
            activate(&shared.source.base, &generation).map_err(|e| e.to_string())?;
            Ok(target)
        })();
        match outcome {
            Ok(target) => {
                if let Ok(mut current) = shared.vectors.write() {
                    *current = target;
                }
                shared.refresh_compat();
                shared.set_reindex_state(REINDEX_DONE);
                shared.log("reindex done");
            }
            Err(message) => {
                shared.log(&format!("reindex failed: {message}"));
                shared.set_reindex_state(&format!("failed: {message}"));
            }
        }
        shared.reindexing.store(false, Ordering::SeqCst);
        let _ = shared.notify_tx.send(());
    });
    Response::Done
}

fn spawn_accept(shared: Arc<Shared>, listener: UnixListener) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shared.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let shared = Arc::clone(&shared);
                    thread::spawn(move || handle_connection(&shared, stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(e) => {
                    shared.log(&format!("accept failed: {e}"));
                    thread::sleep(POLL);
                }
            }
        }
    })
}

fn handle_connection(shared: &Arc<Shared>, stream: UnixStream) {
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
}

fn handle_request(shared: &Arc<Shared>, req: Request) -> Response {
    match req {
        Request::Ingest(ingest_req) => {
            let (tx, rx) = mpsc::channel();
            if shared.ingest_tx.send((ingest_req, tx)).is_err() {
                return Response::Error {
                    message: "ingest worker stopped".into(),
                };
            }
            return rx
                .recv_timeout(CONNECTION_TIMEOUT)
                .unwrap_or_else(|_| Response::Error {
                    message: "ingest timed out".into(),
                });
        }
        Request::Reindex => return start_reindex(shared),
        Request::Inject { .. } | Request::Search(_) | Request::Show { .. } | Request::Status => {}
    }
    let (records, vectors) = match (RecordStore::open(&shared.paths.records()), shared.vectors()) {
        (Ok(r), Some(v)) => (r, v),
        (Err(e), _) => {
            return Response::Error {
                message: e.to_string(),
            };
        }
        (_, None) => {
            return Response::Error {
                message: "vector store unavailable".into(),
            };
        }
    };
    let backends = shared.backends(vectors.as_ref());
    let result: Result<Response, String> = match req {
        Request::Inject {
            prompt,
            cwd,
            session_id,
        } => {
            let c = &shared.config.inject;
            let config = InjectConfig {
                threshold: c.threshold,
                top_k: c.top_k,
                per_item_chars: c.per_item_chars,
                total_chars: c.total_chars,
            };
            let deadline = Instant::now() + Duration::from_millis(c.budget_ms);
            build_injection(
                &records,
                backends,
                &prompt,
                &cwd,
                &session_id,
                config,
                deadline,
            )
            .map(|context| Response::Inject { context })
            .map_err(|e| e.to_string())
        }
        Request::Search(search_req) => search(&records, backends, &search_req, None)
            .map_err(|e| e.to_string())
            .and_then(|hits| render_hits(&records, &hits).map_err(|e| e.to_string()))
            .map(|text| Response::Text { text }),
        Request::Show {
            prompt_id,
            with_results,
        } => render_turn(&records, &prompt_id, with_results)
            .map(|text| Response::Text { text })
            .map_err(|e| e.to_string()),
        Request::Status => status(shared, &records, vectors.as_ref()).map(Response::Status),
        Request::Ingest(_) | Request::Reindex => Ok(Response::Done),
    };
    result.unwrap_or_else(|message| {
        shared.log(&format!("request failed: {message}"));
        Response::Error { message }
    })
}

fn status(
    shared: &Shared,
    records: &RecordStore,
    vectors: &dyn VectorStore,
) -> Result<StatusInfo, String> {
    let counts = records.counts().map_err(|e| e.to_string())?;
    let compatibility = match shared.compat() {
        Compatibility::Empty => "empty",
        Compatibility::Compatible => "compatible",
        Compatibility::Mismatch { .. } => COMPAT_MISMATCH,
    };
    Ok(StatusInfo {
        iterations: counts.iterations,
        results: counts.results,
        embed_jobs: counts.embed_jobs,
        spool_pending: spool::pending_count(&shared.paths.spool()),
        vectors: vectors.count().map_err(|e| e.to_string())?,
        embed_model: shared.embedder.model_id().to_string(),
        compatibility: compatibility.to_string(),
        reindex: shared
            .reindex_state
            .lock()
            .map_or_else(|_| REINDEX_IDLE.to_string(), |s| s.clone()),
    })
}
