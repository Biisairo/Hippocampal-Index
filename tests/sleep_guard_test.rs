//! FR-DMN-13. 이 파일의 테스트는 이 테스트 프로세스의 `caffeinate` 자식 프로세스를 세므로,
//! 다른 데몬 테스트와 섞이지 않게 별도 바이너리로 두고 서로 직렬로 실행한다.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use hippo::client::request;
use hippo::config::{Config, Paths};
use hippo::daemon::{DaemonHandle, start};
use hippo::embedder::{EmbedError, Embedder};
use hippo::ingest::{HookEvent, IngestRequest};
use hippo::protocol::{Request, Response};
use hippo::testing::{HashEmbedder, MemoryVectorStore};
use hippo::vector::VectorStore;

const S2: &str = "afd070e6-4625-4b31-8c43-f1f553949222";
const S2_PROMPT: &str = "8f64252a-0037-484d-a451-35aa2c0076ed";
const TIMEOUT: Duration = Duration::from_secs(10);

static SERIAL: Mutex<()> = Mutex::new(());

/// `open`이 거짓이면 임베딩을 막고 기다린다. `failing`이 참이면 실패한다.
struct GateEmbedder {
    inner: HashEmbedder,
    open: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
}

impl Embedder for GateEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        while !self.open.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.failing.load(Ordering::SeqCst) {
            return Err(EmbedError::Unavailable("test".into()));
        }
        self.inner.embed(texts)
    }
}

struct Env {
    _dir: tempfile::TempDir,
    paths: Paths,
    transcript: PathBuf,
    open: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
}

fn env(open: bool, failing: bool) -> Env {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let paths = Paths::new(dir.path().join("root"));
    let transcript = dir.path().join(format!("{S2}.jsonl"));
    fs::write(&transcript, read_fixture("s2_tools.jsonl")).unwrap();
    Env {
        _dir: dir,
        paths,
        transcript,
        open: Arc::new(AtomicBool::new(open)),
        failing: Arc::new(AtomicBool::new(failing)),
    }
}

fn launch(e: &Env, extra: &str) -> DaemonHandle {
    let config = Config::parse(&format!(
        "[embedder]\nbackend = \"hash\"\ndimension = 256\nquery_prefix = \"\"\n[vector]\nbackend = \"memory\"\n[import]\ntranscripts_dir = \"/nonexistent/hippo-test-transcripts\"\n{extra}"
    ))
    .unwrap();
    let embedder: Arc<dyn Embedder> = Arc::new(GateEmbedder {
        inner: HashEmbedder::new(256, 8_000),
        open: Arc::clone(&e.open),
        failing: Arc::clone(&e.failing),
    });
    let source = hippo::backends::VectorSource {
        name: "default".into(),
        open: Arc::new(|_dir: &Path| {
            let store: Arc<dyn VectorStore> = Arc::new(MemoryVectorStore::default());
            Ok(store)
        }),
    };
    start(e.paths.clone(), config, embedder, source).unwrap()
}

fn ingest(e: &Env) {
    let req = Request::Ingest(IngestRequest {
        event: HookEvent::Stop,
        session_id: S2.into(),
        transcript_path: e.transcript.clone(),
        prompt_id: Some(S2_PROMPT.into()),
    });
    assert_eq!(
        request(&e.paths.socket(), &req, TIMEOUT).unwrap(),
        Response::Done
    );
}

/// 이 테스트 프로세스가 띄운 `caffeinate`의 인자 목록.
fn caffeinates() -> Vec<String> {
    let me = std::process::id().to_string();
    let out = Command::new("ps")
        .args(["-ax", "-o", "ppid=,command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (ppid, command) = line.split_once(' ')?;
            (ppid == me && command.trim_start().starts_with("caffeinate"))
                .then(|| command.trim().to_string())
        })
        .collect()
}

fn holds(expected: bool) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if caffeinates().is_empty() != expected {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// 잠깐 지켜봐도 `caffeinate`가 생기지 않으면 true.
fn never_holds(window: Duration) -> bool {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        if !caffeinates().is_empty() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

#[test]
fn pending_embeddings_prevent_sleep_until_drained() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let e = env(false, false);
    let handle = launch(&e, "");
    assert!(
        never_holds(Duration::from_millis(1_500)),
        "작업이 없으면 잡지 않음: {:?}",
        caffeinates()
    );
    ingest(&e);
    assert!(holds(true), "임베딩 대기 중에는 잠자기를 막아야 함");
    let me = std::process::id();
    assert_eq!(caffeinates(), vec![format!("caffeinate -i -w {me}")]);
    e.open.store(true, Ordering::SeqCst);
    assert!(holds(false), "큐가 비면 놓아야 함");
    handle.shutdown();
}

#[test]
fn disabled_setting_never_prevents_sleep() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let e = env(false, false);
    let handle = launch(&e, "[daemon]\nprevent_sleep = false\n");
    ingest(&e);
    assert!(never_holds(Duration::from_millis(2_000)));
    e.open.store(true, Ordering::SeqCst);
    handle.shutdown();
}

#[test]
fn failing_embedder_does_not_prevent_sleep() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let e = env(false, true);
    let handle = launch(&e, "");
    ingest(&e);
    assert!(holds(true));
    e.open.store(true, Ordering::SeqCst);
    assert!(holds(false), "재시도를 기다리는 동안에는 놓아야 함");
    handle.shutdown();
}

#[test]
fn shutdown_releases_hold() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let e = env(false, false);
    let handle = launch(&e, "");
    ingest(&e);
    assert!(holds(true));
    e.open.store(true, Ordering::SeqCst);
    handle.shutdown();
    assert!(caffeinates().is_empty());
}
