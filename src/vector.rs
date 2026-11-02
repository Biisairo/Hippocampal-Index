//! 벡터 저장소 추상화. 원본 저장소에서 재생성할 수 있는 파생 인덱스다. (FR-SWP-2)

use crate::store::RecordKind;

#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("dimension mismatch: store {expected}, vector {actual}")]
    DimensionMismatch { expected: usize, actual: usize },
    #[error("vector backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    pub kind: RecordKind,
    pub ref_id: String,
    pub chunk_index: u32,
    pub vector: Vec<f32>,
    pub embed_model: String,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub ts: String,
}

impl VectorRecord {
    pub fn id(&self) -> String {
        vector_id(self.kind, &self.ref_id, self.chunk_index)
    }
}

pub fn vector_id(kind: RecordKind, ref_id: &str, chunk_index: u32) -> String {
    format!("{}:{ref_id}#{chunk_index}", kind.as_str())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorFilter {
    pub kind: Option<RecordKind>,
    pub cwd: Option<String>,
    pub exclude_session: Option<String>,
    pub embed_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub kind: RecordKind,
    pub ref_id: String,
    pub chunk_index: u32,
    /// 코사인 유사도. 클수록 가깝다.
    pub score: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreInfo {
    pub embed_model: Option<String>,
    pub dimension: Option<usize>,
}

pub trait VectorStore: Send + Sync {
    fn info(&self) -> Result<StoreInfo, VectorError>;
    fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError>;
    /// `ref_id`의 모든 청크 벡터를 지운다.
    fn delete(&self, kind: RecordKind, ref_id: &str) -> Result<(), VectorError>;
    fn search(
        &self,
        vector: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorHit>, VectorError>;
    fn count(&self) -> Result<u64, VectorError>;
    fn ensure_index(&self) -> Result<(), VectorError>;
    /// 모든 벡터 레코드를 읽는다. 프로젝트 병합에 쓴다. (FR-PRJ-9)
    fn scan(&self) -> Result<Vec<VectorRecord>, VectorError>;
}

/// 길이가 다르거나 영벡터가 있으면 0을 돌려준다.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let denom = norm(a) * norm(b);
    if denom == 0.0 { 0.0 } else { dot / denom }
}
