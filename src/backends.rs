//! 설정에 따라 임베더와 벡터 저장소 백엔드를 만든다. 백엔드를 추가할 때는 이 파일만 바꾼다. (FR-SWP-3)

use std::path::Path;
use std::sync::Arc;

use crate::config::{Config, Paths};
use crate::embedder::{EmbedError, Embedder};
use crate::lance::LanceVectorStore;
use crate::ollama::{OllamaEmbedder, ServerLauncher, spawn_ollama};

const OLLAMA_LOG: &str = "ollama.log";
use crate::testing::{HashEmbedder, MemoryVectorStore};
use crate::vector::{VectorError, VectorStore};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("unknown backend: {0}")]
    Unknown(String),
    #[error(transparent)]
    Embedder(#[from] EmbedError),
}

/// 설정에 따라 임베더를 만든다. Ollama는 `autostart`가 켜져 있으면 서버를 띄우는 방법을 함께 넘긴다. (FR-EMB-1)
pub fn build_embedder(config: &Config, paths: &Paths) -> Result<Arc<dyn Embedder>, BackendError> {
    let c = &config.embedder;
    match c.backend.as_str() {
        "ollama" => {
            let launcher = c.autostart.then(|| {
                let command = c.ollama_command.clone();
                let url = c.url.clone();
                let log = paths.logs().join(OLLAMA_LOG);
                Box::new(move || spawn_ollama(&command, &url, &log).map(|_| ())) as ServerLauncher
            });
            Ok(Arc::new(OllamaEmbedder::try_new_with_launcher(
                c, launcher,
            )?))
        }
        "hash" => Ok(Arc::new(HashEmbedder::new(c.dimension, c.max_input_tokens))),
        other => Err(BackendError::Unknown(other.to_string())),
    }
}

/// 세대 디렉터리를 받아 벡터 저장소를 연다.
pub type OpenStore = Arc<dyn Fn(&Path) -> Result<Arc<dyn VectorStore>, VectorError> + Send + Sync>;

/// 벡터 저장소 이름과 여는 방법. 세대는 프로젝트마다 `projects/<id>/vectors/<이름>/` 아래에 둔다. (FR-SWP-5, DR-1)
#[derive(Clone)]
pub struct VectorSource {
    pub name: String,
    pub open: OpenStore,
}

pub fn vector_source(config: &Config) -> Result<VectorSource, BackendError> {
    let name = config.vector.name.clone();
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
    Ok(VectorSource { name, open })
}
