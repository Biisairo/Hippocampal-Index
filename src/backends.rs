//! 설정에 따라 임베더와 벡터 저장소 백엔드를 만든다. 백엔드를 추가할 때는 이 파일만 바꾼다. (FR-SWP-3)

use std::sync::Arc;

use crate::config::{Config, Paths};
use crate::embedder::Embedder;
use crate::lance::LanceVectorStore;
use crate::ollama::OllamaEmbedder;
use crate::testing::{HashEmbedder, MemoryVectorStore};
use crate::vector::{VectorError, VectorStore};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("unknown backend: {0}")]
    Unknown(String),
    #[error("vector store open failed: {0}")]
    Open(#[from] VectorError),
}

pub fn build_embedder(config: &Config) -> Result<Arc<dyn Embedder>, BackendError> {
    let c = &config.embedder;
    match c.backend.as_str() {
        "ollama" => Ok(Arc::new(OllamaEmbedder::new(c))),
        "hash" => Ok(Arc::new(HashEmbedder::new(c.dimension, c.max_input_tokens))),
        other => Err(BackendError::Unknown(other.to_string())),
    }
}

pub fn build_vector_store(
    config: &Config,
    paths: &Paths,
) -> Result<Arc<dyn VectorStore>, BackendError> {
    match config.vector.backend.as_str() {
        "lancedb" => Ok(Arc::new(LanceVectorStore::open(
            &paths.vectors(&config.vector.name),
        )?)),
        "memory" => Ok(Arc::new(MemoryVectorStore::default())),
        other => Err(BackendError::Unknown(other.to_string())),
    }
}
