mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use hippo::embed_text::build_embed_text;
use hippo::embedder::{EmbedError, Embedder};
use hippo::indexer::{ChunkConfig, Compatibility, compatibility, process_embed_jobs, reindex};
use hippo::store::{Batch, NewIteration, RecordKind, RecordStore};
use hippo::testing::{HashEmbedder, MemoryVectorStore};
use hippo::transcript::{ExtractorState, Iteration, IterationInput, ToolCall, ToolResultData};
use hippo::vector::{VectorFilter, VectorRecord, VectorStore};
use serde_json::json;

const CHUNKING: ChunkConfig = ChunkConfig {
    max_tokens: 8_000,
    chars_per_token: 1.0,
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
        reindex(&store, &new_embedder, &target, CHUNKING, 2).unwrap(),
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
