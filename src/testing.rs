//! 테스트용 결정론적 임베더와 메모리 벡터 저장소. (FR-SWP-8)

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use crate::embedder::{EmbedError, Embedder};
use crate::store::RecordKind;
use crate::vector::{
    StoreInfo, VectorError, VectorFilter, VectorHit, VectorRecord, VectorStore, cosine,
};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 단어를 해시해 차원에 누적한 뒤 정규화한다. 같은 단어를 공유할수록 가깝다.
pub struct HashEmbedder {
    model_id: String,
    dimension: usize,
    max_input_tokens: usize,
}

impl HashEmbedder {
    pub fn new(dimension: usize, max_input_tokens: usize) -> Self {
        Self {
            model_id: format!("test-hash-{dimension}"),
            dimension,
            max_input_tokens,
        }
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dimension];
        if self.dimension == 0 {
            return v;
        }
        for word in text.split_whitespace() {
            let hash = word.to_lowercase().bytes().fold(FNV_OFFSET, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
            });
            v[(hash % self.dimension as u64) as usize] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn max_input_tokens(&self) -> usize {
        self.max_input_tokens
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}

#[derive(Default)]
struct MemoryState {
    records: BTreeMap<String, VectorRecord>,
    info: StoreInfo,
}

#[derive(Default)]
pub struct MemoryVectorStore {
    state: Mutex<MemoryState>,
}

impl MemoryVectorStore {
    fn lock(&self) -> Result<MutexGuard<'_, MemoryState>, VectorError> {
        self.state
            .lock()
            .map_err(|e| VectorError::Backend(e.to_string()))
    }
}

impl VectorStore for MemoryVectorStore {
    fn info(&self) -> Result<StoreInfo, VectorError> {
        Ok(self.lock()?.info.clone())
    }

    fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError> {
        let mut state = self.lock()?;
        let Some(first) = records.first() else {
            return Ok(());
        };
        let expected = state.info.dimension.unwrap_or(first.vector.len());
        if let Some(bad) = records.iter().find(|r| r.vector.len() != expected) {
            return Err(VectorError::DimensionMismatch {
                expected,
                actual: bad.vector.len(),
            });
        }
        if state.info.dimension.is_none() {
            state.info = StoreInfo {
                embed_model: Some(first.embed_model.clone()),
                dimension: Some(expected),
            };
        }
        for r in records {
            state.records.insert(r.id(), r.clone());
        }
        Ok(())
    }

    fn delete(&self, kind: RecordKind, ref_id: &str) -> Result<(), VectorError> {
        self.lock()?
            .records
            .retain(|_, r| !(r.kind == kind && r.ref_id == ref_id));
        Ok(())
    }

    fn search(
        &self,
        vector: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorHit>, VectorError> {
        let state = self.lock()?;
        let mut hits: Vec<VectorHit> = state
            .records
            .values()
            .filter(|r| filter.kind.is_none_or(|k| r.kind == k))
            .filter(|r| filter.cwd.is_none() || r.cwd == filter.cwd)
            .filter(|r| filter.exclude_session.is_none() || r.session_id != filter.exclude_session)
            .filter(|r| {
                filter
                    .embed_model
                    .as_ref()
                    .is_none_or(|m| &r.embed_model == m)
            })
            .map(|r| VectorHit {
                kind: r.kind,
                ref_id: r.ref_id.clone(),
                chunk_index: r.chunk_index,
                score: cosine(vector, &r.vector),
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
    }

    fn count(&self) -> Result<u64, VectorError> {
        Ok(self.lock()?.records.len() as u64)
    }

    fn ensure_index(&self) -> Result<(), VectorError> {
        Ok(())
    }
}
