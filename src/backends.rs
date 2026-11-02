//! 설정에 따라 임베더와 벡터 저장소 백엔드를 만든다. 백엔드를 추가할 때는 이 파일만 바꾼다. (FR-SWP-3)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{Config, Paths};
use crate::embedder::{EmbedError, Embedder};
use crate::lance::LanceVectorStore;
use crate::ollama::OllamaEmbedder;
use crate::testing::{HashEmbedder, MemoryVectorStore};
use crate::vector::{VectorError, VectorStore};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("unknown backend: {0}")]
    Unknown(String),
    #[error(transparent)]
    Embedder(#[from] EmbedError),
}

pub fn build_embedder(config: &Config) -> Result<Arc<dyn Embedder>, BackendError> {
    let c = &config.embedder;
    match c.backend.as_str() {
        "ollama" => Ok(Arc::new(OllamaEmbedder::try_new(c)?)),
        "hash" => Ok(Arc::new(HashEmbedder::new(c.dimension, c.max_input_tokens))),
        other => Err(BackendError::Unknown(other.to_string())),
    }
}

/// 세대 디렉터리를 받아 벡터 저장소를 연다.
pub type OpenStore = Arc<dyn Fn(&Path) -> Result<Arc<dyn VectorStore>, VectorError> + Send + Sync>;

/// 벡터 저장소의 기준 디렉터리(`vectors/<이름>`)와 여는 방법. 세대는 그 아래에 둔다. (FR-SWP-5)
#[derive(Clone)]
pub struct VectorSource {
    pub base: PathBuf,
    pub open: OpenStore,
}

pub fn vector_source(config: &Config, paths: &Paths) -> Result<VectorSource, BackendError> {
    let base = paths.vectors(&config.vector.name);
    let open: OpenStore =
        match config.vector.backend.as_str() {
            "lancedb" => Arc::new(|dir: &Path| {
                Ok(Arc::new(LanceVectorStore::open(dir)?) as Arc<dyn VectorStore>)
            }),
            "memory" => Arc::new(|_: &Path| {
                Ok(Arc::new(MemoryVectorStore::default()) as Arc<dyn VectorStore>)
            }),
            other => return Err(BackendError::Unknown(other.to_string())),
        };
    Ok(VectorSource { base, open })
}
