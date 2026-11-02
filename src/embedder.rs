//! 임베더 추상화. (FR-SWP-1)

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding backend unavailable: {0}")]
    Unavailable(String),
    #[error("invalid embedding response: {0}")]
    InvalidResponse(String),
}

pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn dimension(&self) -> usize;
    fn max_input_tokens(&self) -> usize;
    /// 입력 순서대로 `dimension()` 길이의 벡터를 돌려준다.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}
