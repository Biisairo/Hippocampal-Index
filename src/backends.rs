//! 설정에 따라 임베더와 벡터 저장소 백엔드를 만든다. (FR-SWP-3)

use std::sync::Arc;

use crate::config::{Config, Paths};
use crate::embedder::Embedder;
use crate::testing::{HashEmbedder, MemoryVectorStore};
use crate::vector::VectorStore;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("backend not available yet: {0}")]
    NotAvailable(String),
    #[error("unknown backend: {0}")]
    Unknown(String),
}

pub fn build_embedder(config: &Config) -> Result<Arc<dyn Embedder>, BackendError> {
    let c = &config.embedder;
    match c.backend.as_str() {
        "hash" => Ok(Arc::new(HashEmbedder::new(c.dimension, c.max_input_tokens))),
        "ollama" => Err(BackendError::NotAvailable(c.backend.clone())),
        other => Err(BackendError::Unknown(other.to_string())),
    }
}

pub fn build_vector_store(
    config: &Config,
    _paths: &Paths,
) -> Result<Arc<dyn VectorStore>, BackendError> {
    match config.vector.backend.as_str() {
        "memory" => Ok(Arc::new(MemoryVectorStore::default())),
        "lancedb" => Err(BackendError::NotAvailable(config.vector.backend.clone())),
        other => Err(BackendError::Unknown(other.to_string())),
    }
}
