mod common;

use common::probe::{Probe, lance_versions};

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use hippo::embed_text::build_embed_text;
use hippo::embedder::{EmbedError, Embedder, InjectGate, YieldingEmbedder};
use hippo::indexer::{
    ChunkConfig, Compatibility, compatibility, optimize_if_due, process_embed_jobs,
    process_jobs_until, reindex,
};
use hippo::lance::LanceVectorStore;
use hippo::store::{Batch, NewIteration, RecordKind, RecordStore};
use hippo::testing::{HashEmbedder, MemoryVectorStore};
use hippo::transcript::{ExtractorState, Iteration, IterationInput, ToolCall, ToolResultData};
use hippo::vector::{VectorFilter, VectorRecord, VectorStore};
use serde_json::json;

const CHUNKING: ChunkConfig = ChunkConfig {
    max_tokens: 8_000,
    chars_per_token: 1.0,
    overlap_chars: 0,
};

fn iteration(mid: &str, response: &str, result: Option<&str>) -> NewIteration {
    let it = Iteration {
        session_id: "s".into(),
        prompt_id: Some("p".into()),
        message_id: mid.into(),
        iteration_index: 0,
        cwd: "/w".into(),
        ts: "2026-11-02T00:00:00Z".into(),
        inputs: vec![IterationInput::Prompt {
            text: "질문".into(),
        }],
        texts: if response.is_empty() {
            vec![]
        } else {
            vec![response.into()]
        },
        tool_calls: result
            .map(|r| ToolCall {
                id: format!("c-{mid}"),
                name: "Read".into(),
                input: json!({}),
                result: Some(ToolResultData {
                    text: r.into(),
                    is_error: false,
                    structured: None,
                }),
            })
            .into_iter()
            .collect(),
    };
    let embed_text = build_embed_text(&it, 500, &[]);
    NewIteration {
        iteration: it,
        embed_text,
    }
}

fn store_with(iterations: Vec<NewIteration>) -> RecordStore {
    let mut store = RecordStore::open_in_memory().unwrap();
    store
        .apply(&Batch {
            session_id: "s".into(),
            transcript_path: "/t".into(),
            iterations,
            completions: vec![],
            state: ExtractorState::default(),
            bulk: false,
            source: hippo::store::Source::Live,
        })
        .unwrap();
    store
}

struct FailingEmbedder {
    inner: HashEmbedder,
    fail: AtomicBool,
}

impl Embedder for FailingEmbedder {
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
        if self.fail.load(Ordering::SeqCst) {
            return Err(EmbedError::Unavailable("down".into()));
        }
        self.inner.embed(texts)
    }
}

// ---- FR-STO-7 ----

#[test]
fn processes_jobs_into_vectors_with_metadata() {
    let mut store = store_with(vec![iteration(
        "m1",
        "토큰 만료 버그",
        Some("fn main() {}"),
    )]);
    let e = HashEmbedder::new(32, 8_000);
    let v = MemoryVectorStore::default();
    assert_eq!(
        process_embed_jobs(&mut store, &e, &v, CHUNKING, 10).unwrap(),
        2
    );
    assert_eq!(store.counts().unwrap().embed_jobs, 0);
    assert_eq!(v.count().unwrap(), 2);

    let it_filter = VectorFilter {
        kind: Some(RecordKind::Iteration),
        ..Default::default()
    };
    let q = e
        .embed(&[store.iteration("s:m1").unwrap().unwrap().embed_text])
        .unwrap()
        .remove(0);
    let hits = v.search(&q, &it_filter, 5).unwrap();
    assert_eq!(hits[0].ref_id, "s:m1");
    assert!((hits[0].score - 1.0).abs() < 1e-4);
    assert_eq!(
        v.info().unwrap().embed_model.as_deref(),
        Some("test-hash-32")
    );
}

#[test]
fn respects_job_limit() {
    let mut store = store_with(vec![
        iteration("m1", "a b c", None),
        iteration("m2", "d e f", None),
    ]);
    let v = MemoryVectorStore::default();
    assert_eq!(
        process_embed_jobs(&mut store, &HashEmbedder::new(8, 8_000), &v, CHUNKING, 1).unwrap(),
        1
    );
    assert_eq!(store.counts().unwrap().embed_jobs, 1);
}

#[test]
fn long_text_is_chunked_by_embedder_limit() {
    let long = "가".repeat(25);
    let mut store = store_with(vec![iteration("m1", "", Some(&long))]);
    let v = MemoryVectorStore::default();
    process_embed_jobs(&mut store, &HashEmbedder::new(8, 10), &v, CHUNKING, 10).unwrap();
    let result_vectors = v
        .search(
            &[1.0; 8],
            &VectorFilter {
                kind: Some(RecordKind::Result),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(result_vectors.len(), 3, "25자를 10자 한도로 나누면 3청크");
}

#[test]
fn stale_chunks_are_replaced() {
    let mut store = store_with(vec![iteration("m1", "짧은 응답", None)]);
    let v = MemoryVectorStore::default();
    let stale = VectorRecord {
        kind: RecordKind::Iteration,
        ref_id: "s:m1".into(),
        chunk_index: 5,
        vector: vec![1.0; 8],
        embed_model: "test-hash-8".into(),
        cwd: None,
        session_id: None,
        ts: String::new(),
    };
    v.upsert(&[stale]).unwrap();
    process_embed_jobs(&mut store, &HashEmbedder::new(8, 8_000), &v, CHUNKING, 10).unwrap();
    assert_eq!(v.count().unwrap(), 1);
}

#[test]
fn empty_embed_text_completes_without_vectors() {
    let mut it = iteration("m1", "", None);
    it.iteration.inputs.clear();
    it.embed_text = String::new();
    let mut no_text = store_with(vec![it]);
    let v = MemoryVectorStore::default();
    assert_eq!(
        process_embed_jobs(&mut no_text, &HashEmbedder::new(8, 8_000), &v, CHUNKING, 10).unwrap(),
        1
    );
    assert_eq!(v.count().unwrap(), 0);
    assert_eq!(no_text.counts().unwrap().embed_jobs, 0);
}

#[test]
fn failed_embedding_keeps_jobs_for_retry() {
    let mut store = store_with(vec![iteration("m1", "응답", Some("결과"))]);
    let e = FailingEmbedder {
        inner: HashEmbedder::new(8, 8_000),
        fail: AtomicBool::new(true),
    };
    let v = MemoryVectorStore::default();
    assert!(process_embed_jobs(&mut store, &e, &v, CHUNKING, 10).is_err());
    assert_eq!(store.counts().unwrap().embed_jobs, 2);
    assert_eq!(v.count().unwrap(), 0);

    e.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        process_embed_jobs(&mut store, &e, &v, CHUNKING, 10).unwrap(),
        2
    );
    assert_eq!(store.counts().unwrap().embed_jobs, 0);
}

// ---- FR-SWP-5 ----

#[test]
fn reindex_rebuilds_everything_into_new_store() {
    let mut store = store_with(vec![
        iteration("m1", "하나", Some("결과 하나")),
        iteration("m2", "둘", Some("결과 하나")),
        iteration("m3", "셋", None),
    ]);
    let old = MemoryVectorStore::default();
    process_embed_jobs(
        &mut store,
        &HashEmbedder::new(8, 8_000),
        &old,
        CHUNKING,
        100,
    )
    .unwrap();

    let new_embedder = HashEmbedder::new(16, 8_000);
    let target = MemoryVectorStore::default();
    assert_eq!(
        reindex(&store, &new_embedder, &target, CHUNKING, 2, 20).unwrap(),
        4,
        "반복 3 + 중복 제거된 결과 1"
    );
    assert_eq!(target.count().unwrap(), 4);
    assert_eq!(target.info().unwrap().dimension, Some(16));
    assert_eq!(
        old.info().unwrap().dimension,
        Some(8),
        "기존 저장소는 그대로"
    );
}

#[test]
fn record_store_pages_follow_rowid_order() {
    let store = store_with(vec![
        iteration("m1", "a", Some("r1")),
        iteration("m2", "b", Some("r2")),
        iteration("m3", "c", None),
    ]);
    let first = store.iteration_page(0, 2).unwrap();
    assert_eq!(
        first.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>(),
        ["s:m1", "s:m2"]
    );
    let rest = store.iteration_page(first[1].0, 2).unwrap();
    assert_eq!(
        rest.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>(),
        ["s:m3"]
    );
    assert!(store.iteration_page(rest[0].0, 2).unwrap().is_empty());
    assert_eq!(store.result_page(0, 10).unwrap().len(), 2);
}

// ---- FR-SWP-6 ----

#[test]
fn compatibility_detects_model_and_dimension() {
    let mut store = store_with(vec![iteration("m1", "응답", None)]);
    let e8 = HashEmbedder::new(8, 8_000);
    let v = MemoryVectorStore::default();
    assert_eq!(compatibility(&e8, &v).unwrap(), Compatibility::Empty);
    process_embed_jobs(&mut store, &e8, &v, CHUNKING, 10).unwrap();
    assert_eq!(compatibility(&e8, &v).unwrap(), Compatibility::Compatible);
    assert_eq!(
        compatibility(&HashEmbedder::new(16, 8_000), &v).unwrap(),
        Compatibility::Mismatch {
            store_model: Some("test-hash-8".into()),
            store_dimension: Some(8)
        }
    );
}

// ---- FR-STO-5, FR-STO-7: 청크마다 요청 1개 ----

struct RecordingEmbedder {
    inner: HashEmbedder,
    calls: Mutex<Vec<Vec<String>>>,
}

impl RecordingEmbedder {
    fn new(max_input_tokens: usize) -> Self {
        Self {
            inner: HashEmbedder::new(8, max_input_tokens),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl Embedder for RecordingEmbedder {
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
        self.calls.lock().unwrap().push(texts.to_vec());
        self.inner.embed(texts)
    }
}

#[test]
fn each_chunk_is_embedded_in_its_own_request_with_overlap() {
    let long = "가나다라마바사아자차카타파하".repeat(2);
    let mut store = store_with(vec![iteration("m1", "", Some(&long))]);
    let v = MemoryVectorStore::default();
    let embedder = RecordingEmbedder::new(10);
    let chunking = ChunkConfig {
        overlap_chars: 2,
        ..CHUNKING
    };
    process_embed_jobs(&mut store, &embedder, &v, chunking, 10).unwrap();
    let calls = embedder.calls.lock().unwrap();
    assert!(calls.iter().all(|c| c.len() == 1), "{calls:?}");
    let result_chunks: Vec<&String> = calls
        .iter()
        .map(|c| &c[0])
        .filter(|t| long.contains(t.as_str()))
        .collect();
    assert_eq!(
        result_chunks.len(),
        4,
        "28자, L=10, O=2: [0,10) [8,18) [16,26) [24,28)"
    );
    assert_eq!(v.count().unwrap() as usize, calls.len());
}

// ---- FR-DMN-6 (b): 작업 1건마다 중단 확인 ----

#[test]
fn processing_stops_after_the_job_when_asked() {
    let mut store = store_with(vec![
        iteration("m1", "a b c", None),
        iteration("m2", "d e f", None),
        iteration("m3", "g h i", None),
    ]);
    let jobs = store.embed_jobs(10).unwrap();
    assert_eq!(jobs.len(), 3);
    let v = MemoryVectorStore::default();
    let checks = Mutex::new(0);
    let stop = || {
        let mut n = checks.lock().unwrap();
        *n += 1;
        true
    };
    let done = process_jobs_until(
        &mut store,
        &HashEmbedder::new(8, 8_000),
        &v,
        CHUNKING,
        &jobs,
        &stop,
    )
    .unwrap();
    assert_eq!(done, 1, "첫 작업을 마친 뒤 멈춘다");
    assert_eq!(*checks.lock().unwrap(), 1);
    assert_eq!(
        store.counts().unwrap().embed_jobs,
        2,
        "남은 작업은 큐에 남는다"
    );

    let rest = store.embed_jobs(10).unwrap();
    let done = process_jobs_until(
        &mut store,
        &HashEmbedder::new(8, 8_000),
        &v,
        CHUNKING,
        &rest,
        &|| false,
    )
    .unwrap();
    assert_eq!(done, 2);
    assert_eq!(store.counts().unwrap().embed_jobs, 0);
}

// ---- FR-DMN-6 (a): 자동 주입 처리 중에는 청크 요청을 미룬다 ----

#[test]
fn yielding_embedder_waits_while_inject_is_active() {
    let gate = InjectGate::default();
    let inner = HashEmbedder::new(8, 8_000);
    let yielding = YieldingEmbedder {
        inner: &inner,
        gate: &gate,
    };
    assert_eq!(yielding.model_id(), inner.model_id());
    let done = AtomicBool::new(false);
    let guard = gate.enter();
    assert!(gate.is_busy());
    std::thread::scope(|s| {
        let worker = s.spawn(|| {
            let out = yielding.embed(&["x".to_string()]).unwrap();
            done.store(true, Ordering::SeqCst);
            out
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !done.load(Ordering::SeqCst),
            "주입 중에는 요청을 보내지 않는다"
        );
        drop(guard);
        assert_eq!(worker.join().unwrap().len(), 1);
    });
    assert!(done.load(Ordering::SeqCst));
    assert!(!gate.is_busy());
}

#[test]
fn inject_gate_counts_nested_entries() {
    let gate = InjectGate::default();
    let a = gate.enter();
    let b = gate.enter();
    drop(a);
    assert!(gate.is_busy());
    drop(b);
    assert!(!gate.is_busy());
}

// ---- FR-STO-7, FR-STO-12: 묶음 저장과 정리 ----

fn many(n: usize) -> RecordStore {
    store_with(
        (0..n)
            .map(|i| iteration(&format!("m{i}"), &format!("응답 {i} 내용"), None))
            .collect(),
    )
}

#[test]
fn batch_is_saved_with_one_replace() {
    let mut store = many(5);
    let v = Probe::new(Box::new(MemoryVectorStore::default()));
    let jobs = store.embed_jobs(10).unwrap();
    let done = process_jobs_until(
        &mut store,
        &HashEmbedder::new(8, 8_000),
        &v,
        CHUNKING,
        &jobs,
        &|| false,
    )
    .unwrap();
    assert_eq!(done, 5);
    assert_eq!(v.replaces.load(Ordering::SeqCst), 1);
    assert_eq!(v.other_writes.load(Ordering::SeqCst), 0);
    assert_eq!(v.count().unwrap(), 5);
}

#[test]
fn stopped_batch_saves_only_finished_jobs() {
    let mut store = many(3);
    let v = Probe::new(Box::new(MemoryVectorStore::default()));
    let jobs = store.embed_jobs(10).unwrap();
    let done = process_jobs_until(
        &mut store,
        &HashEmbedder::new(8, 8_000),
        &v,
        CHUNKING,
        &jobs,
        &|| true,
    )
    .unwrap();
    assert_eq!(done, 1);
    assert_eq!(v.replaces.load(Ordering::SeqCst), 1);
    assert_eq!(v.count().unwrap(), 1);
    assert_eq!(store.counts().unwrap().embed_jobs, 2);
}

/// 두 번째 청크 요청부터 실패하는 임베더.
struct FailAfter {
    inner: HashEmbedder,
    left: AtomicUsize,
}

impl Embedder for FailAfter {
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
        if self.left.load(Ordering::SeqCst) == 0 {
            return Err(EmbedError::Unavailable("down".into()));
        }
        self.left.fetch_sub(1, Ordering::SeqCst);
        self.inner.embed(texts)
    }
}

#[test]
fn failure_mid_batch_saves_and_completes_earlier_jobs() {
    let mut store = many(3);
    let v = MemoryVectorStore::default();
    let e = FailAfter {
        inner: HashEmbedder::new(8, 8_000),
        left: AtomicUsize::new(1),
    };
    assert!(process_embed_jobs(&mut store, &e, &v, CHUNKING, 10).is_err());
    assert_eq!(v.count().unwrap(), 1, "첫 작업의 벡터는 저장");
    assert_eq!(store.counts().unwrap().embed_jobs, 2, "첫 작업만 완료");
}

#[test]
fn optimize_if_due_follows_the_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let s = LanceVectorStore::open(dir.path()).unwrap();
    s.upsert(&[VectorRecord {
        kind: RecordKind::Iteration,
        ref_id: "a".into(),
        chunk_index: 0,
        vector: vec![1.0, 0.0],
        embed_model: "m".into(),
        cwd: None,
        session_id: None,
        ts: String::new(),
    }])
    .unwrap();
    s.delete(RecordKind::Iteration, "zzz").unwrap();
    assert!(!optimize_if_due(&s, 3).unwrap(), "2 < 3");
    assert!(!optimize_if_due(&s, 0).unwrap(), "0이면 하지 않음");
    assert_eq!(s.pending_changes(), 2);
    assert!(optimize_if_due(&s, 2).unwrap());
    assert_eq!(s.pending_changes(), 0);
    assert_eq!(lance_versions(dir.path()), 1);
}

#[test]
fn reindex_replaces_per_page_and_stays_bounded() {
    let store = many(12);
    let dir = tempfile::tempdir().unwrap();
    let target = Probe::new(Box::new(LanceVectorStore::open(dir.path()).unwrap()));
    assert_eq!(
        reindex(
            &store,
            &HashEmbedder::new(8, 8_000),
            &target,
            CHUNKING,
            2,
            3
        )
        .unwrap(),
        12
    );
    assert_eq!(
        target.replaces.load(Ordering::SeqCst),
        6,
        "페이지 2개씩 6번"
    );
    assert_eq!(target.other_writes.load(Ordering::SeqCst), 0);
    assert!(
        target.max_pending.load(Ordering::SeqCst) <= 3 + 1,
        "max pending {}",
        target.max_pending.load(Ordering::SeqCst)
    );
    assert_eq!(target.pending_changes(), 0, "끝나면 한 번 더 정리");
    assert_eq!(lance_versions(dir.path()), 1);
    assert_eq!(target.count().unwrap(), 12);
}
