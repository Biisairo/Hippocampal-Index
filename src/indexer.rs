//! 임베딩 작업 처리, 재색인, 호환성 판정. (FR-STO-7, FR-SWP-5, FR-SWP-6)

use crate::embed_text::{chunk, chunk_limit};
use crate::embedder::{EmbedError, Embedder};
use crate::store::{EmbedJob, RecordKind, RecordStore, StoreError};
use crate::vector::{VectorError, VectorRecord, VectorStore};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkConfig {
    pub max_tokens: usize,
    pub chars_per_token: f64,
    pub overlap_chars: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Embed(#[from] EmbedError),
    #[error(transparent)]
    Vector(#[from] VectorError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compatibility {
    /// 아직 벡터가 없다.
    Empty,
    Compatible,
    Mismatch {
        store_model: Option<String>,
        store_dimension: Option<usize>,
    },
}

/// 큐에서 최대 `limit`개 작업을 처리하고 완료한 작업 수를 돌려준다.
/// 실패하면 그때까지 끝난 작업만 완료 처리하고 오류를 돌려준다. 남은 작업은 다음에 재시도한다.
pub fn process_embed_jobs(
    records: &mut RecordStore,
    embedder: &dyn Embedder,
    vectors: &dyn VectorStore,
    chunking: ChunkConfig,
    limit: usize,
) -> Result<usize, IndexError> {
    let jobs = records.embed_jobs(limit)?;
    process_jobs_until(records, embedder, vectors, chunking, &jobs, &|| false)
}

/// 주어진 작업을 차례로 처리한다. 작업 1건을 마칠 때마다 `stop()`이 참이면 남은 작업을 두고 멈춘다.
/// 실패하면 그때까지 끝난 작업만 완료 처리하고 오류를 돌려준다. (FR-DMN-6)
pub fn process_jobs_until(
    records: &mut RecordStore,
    embedder: &dyn Embedder,
    vectors: &dyn VectorStore,
    chunking: ChunkConfig,
    jobs: &[EmbedJob],
    stop: &dyn Fn() -> bool,
) -> Result<usize, IndexError> {
    let mut done: Vec<EmbedJob> = Vec::new();
    let mut failure = None;
    for job in jobs {
        match index_one(records, embedder, vectors, chunking, job.kind, &job.ref_id) {
            Ok(()) => done.push(job.clone()),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
        if stop() {
            break;
        }
    }
    records.complete_embed_jobs(&done)?;
    match failure {
        Some(e) => Err(e),
        None => Ok(done.len()),
    }
}

/// 원본 저장소 전체를 대상 벡터 저장소에 다시 색인한다. 색인한 레코드(반복+결과) 수를 돌려준다.
pub fn reindex(
    records: &RecordStore,
    embedder: &dyn Embedder,
    target: &dyn VectorStore,
    chunking: ChunkConfig,
    page_size: usize,
) -> Result<usize, IndexError> {
    let mut total = 0;
    for kind in [RecordKind::Iteration, RecordKind::Result] {
        let mut after = 0;
        loop {
            let page = match kind {
                RecordKind::Iteration => records.iteration_page(after, page_size)?,
                RecordKind::Result => records.result_page(after, page_size)?,
            };
            let Some((last, _)) = page.last() else {
                break;
            };
            after = *last;
            for (_, ref_id) in &page {
                index_one(records, embedder, target, chunking, kind, ref_id)?;
                total += 1;
            }
        }
    }
    target.ensure_index()?;
    Ok(total)
}

pub fn compatibility(
    embedder: &dyn Embedder,
    vectors: &dyn VectorStore,
) -> Result<Compatibility, IndexError> {
    let info = vectors.info()?;
    Ok(match (&info.embed_model, info.dimension) {
        (None, None) => Compatibility::Empty,
        (Some(model), Some(dim)) if model == embedder.model_id() && dim == embedder.dimension() => {
            Compatibility::Compatible
        }
        _ => Compatibility::Mismatch {
            store_model: info.embed_model,
            store_dimension: info.dimension,
        },
    })
}

struct Source {
    text: String,
    cwd: Option<String>,
    session_id: Option<String>,
    ts: String,
}

fn load_source(
    records: &RecordStore,
    kind: RecordKind,
    ref_id: &str,
) -> Result<Option<Source>, StoreError> {
    Ok(match kind {
        RecordKind::Iteration => records.iteration(ref_id)?.map(|it| Source {
            text: it.embed_text,
            cwd: Some(it.cwd),
            session_id: Some(it.session_id),
            ts: it.ts,
        }),
        RecordKind::Result => records.result(ref_id)?.map(|r| Source {
            text: r.text,
            cwd: None,
            session_id: None,
            ts: r.first_seen_ts,
        }),
    })
}

fn index_one(
    records: &RecordStore,
    embedder: &dyn Embedder,
    vectors: &dyn VectorStore,
    chunking: ChunkConfig,
    kind: RecordKind,
    ref_id: &str,
) -> Result<(), IndexError> {
    let Some(source) = load_source(records, kind, ref_id)? else {
        return Ok(());
    };
    let limit = chunk_limit(
        embedder.max_input_tokens(),
        chunking.max_tokens,
        chunking.chars_per_token,
    );
    let chunks = chunk(&source.text, limit, chunking.overlap_chars);
    let mut embedded = Vec::with_capacity(chunks.len());
    for text in &chunks {
        let mut vectors = embedder.embed(std::slice::from_ref(text))?;
        if vectors.len() != 1 {
            return Err(EmbedError::InvalidResponse(format!(
                "expected 1 vector, got {}",
                vectors.len()
            ))
            .into());
        }
        embedded.append(&mut vectors);
    }
    let new_records: Vec<VectorRecord> = embedded
        .into_iter()
        .enumerate()
        .map(|(i, vector)| VectorRecord {
            kind,
            ref_id: ref_id.to_string(),
            chunk_index: i as u32,
            vector,
            embed_model: embedder.model_id().to_string(),
            cwd: source.cwd.clone(),
            session_id: source.session_id.clone(),
            ts: source.ts.clone(),
        })
        .collect();
    vectors.delete(kind, ref_id)?;
    vectors.upsert(&new_records)?;
    Ok(())
}
