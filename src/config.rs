//! 설정 파일과 데이터 경로. (DR-1, FR-SWP-3)

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::redact::ExtraPattern;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config read failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("embedder url must point to localhost: {0}")]
    NonLocalUrl(String),
    #[error("HOME is not set")]
    NoHome,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbedderConfig {
    pub backend: String,
    pub model: String,
    pub dimension: usize,
    pub max_input_tokens: usize,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VectorConfig {
    pub backend: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChunkingConfig {
    pub max_tokens: usize,
    pub chars_per_token: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbedTextConfig {
    pub max_tool_input_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InjectSection {
    pub threshold: f32,
    pub top_k: usize,
    pub per_item_chars: usize,
    pub total_chars: usize,
    pub budget_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    pub vector_index_threshold: u64,
    pub job_batch: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternEntry {
    pub kind: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RedactConfig {
    pub patterns: Vec<PatternEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub embedder: EmbedderConfig,
    pub vector: VectorConfig,
    pub chunking: ChunkingConfig,
    pub embed_text: EmbedTextConfig,
    pub inject: InjectSection,
    pub index: IndexConfig,
    pub redact: RedactConfig,
}

impl Config {
    /// 파일이 없으면 기본값을 쓴다.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text)?;
        if !is_local_url(&config.embedder.url) {
            return Err(ConfigError::NonLocalUrl(config.embedder.url));
        }
        Ok(config)
    }

    pub fn redact_patterns(&self) -> Vec<ExtraPattern> {
        self.redact
            .patterns
            .iter()
            .map(|p| ExtraPattern {
                kind: p.kind.clone(),
                pattern: p.pattern.clone(),
            })
            .collect()
    }
}

/// 데이터 루트 아래 경로 배치. (DR-1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `$HOME/.session-memory`
    pub fn default_root() -> Result<PathBuf, ConfigError> {
        let home = std::env::var_os("HOME").ok_or(ConfigError::NoHome)?;
        Ok(PathBuf::from(home).join(DATA_DIR))
    }

    pub fn records(&self) -> PathBuf {
        self.root.join("records.sqlite")
    }
    pub fn vectors(&self, name: &str) -> PathBuf {
        self.root.join("vectors").join(name)
    }
    pub fn spool(&self) -> PathBuf {
        self.root.join("spool")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn socket(&self) -> PathBuf {
        self.root.join("smem.sock")
    }
    pub fn lock(&self) -> PathBuf {
        self.root.join("smem.lock")
    }
}

const DATA_DIR: &str = ".session-memory";
const LOCAL_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// `http(s)://<host>[:port][/...]`의 host가 로컬 루프백인지 확인한다. (C-4)
fn is_local_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    LOCAL_HOSTS.contains(&host)
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            backend: "ollama".into(),
            model: "qwen3-embedding:0.6b".into(),
            dimension: 1024,
            max_input_tokens: 32_000,
            url: "http://127.0.0.1:11434".into(),
        }
    }
}
impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            backend: "lancedb".into(),
            name: "default".into(),
        }
    }
}
impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_tokens: 8_000,
            chars_per_token: 1.0,
        }
    }
}
impl Default for EmbedTextConfig {
    fn default() -> Self {
        Self {
            max_tool_input_chars: 500,
        }
    }
}
impl Default for InjectSection {
    fn default() -> Self {
        Self {
            threshold: 0.55,
            top_k: 3,
            per_item_chars: 800,
            total_chars: 4_000,
            budget_ms: 800,
        }
    }
}
impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            vector_index_threshold: 100_000,
            job_batch: 64,
        }
    }
}
