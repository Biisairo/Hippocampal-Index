//! Ollama 임베더 백엔드. localhost의 `/api/embed`를 호출한다. (C-3, C-4)

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::{EmbedderConfig, is_local_url};
use crate::embedder::{EmbedError, Embedder};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const EMBED_PATH: &str = "/api/embed";

pub struct OllamaEmbedder {
    endpoint: String,
    model: String,
    dimension: usize,
    max_input_tokens: usize,
    keep_alive: String,
    agent: ureq::Agent,
}

impl OllamaEmbedder {
    /// URL이 localhost가 아니면 거부한다. 설정 파싱을 거치지 않는 경로도 막기 위해 여기서 다시 검사한다. (C-4)
    pub fn try_new(config: &EmbedderConfig) -> Result<Self, EmbedError> {
        if !is_local_url(&config.url) {
            return Err(EmbedError::Config(format!(
                "embedder url must point to localhost: {}",
                config.url
            )));
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into();
        Ok(Self {
            endpoint: format!("{}{EMBED_PATH}", config.url.trim_end_matches('/')),
            model: config.model.clone(),
            dimension: config.dimension,
            max_input_tokens: config.max_input_tokens,
            keep_alive: config.keep_alive.clone(),
            agent,
        })
    }
}

impl Embedder for OllamaEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn max_input_tokens(&self) -> usize {
        self.max_input_tokens
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({
            "model": self.model,
            "input": texts,
            "dimensions": self.dimension,
            "keep_alive": self.keep_alive,
        });
        let mut response = self
            .agent
            .post(&self.endpoint)
            .send_json(&body)
            .map_err(|e| EmbedError::Unavailable(e.to_string()))?;
        let status = response.status().as_u16();
        let payload: Value = response
            .body_mut()
            .read_json()
            .map_err(|e| EmbedError::InvalidResponse(format!("status {status}: {e}")))?;
        if !(200..300).contains(&status) {
            let message = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(EmbedError::InvalidResponse(format!(
                "status {status}: {message}"
            )));
        }
        let vectors: Vec<Vec<f32>> = payload
            .get("embeddings")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| EmbedError::InvalidResponse(e.to_string()))?
            .ok_or_else(|| EmbedError::InvalidResponse("missing embeddings".into()))?;
        if vectors.len() != texts.len() {
            return Err(EmbedError::InvalidResponse(format!(
                "expected {} vectors, got {}",
                texts.len(),
                vectors.len()
            )));
        }
        if let Some(bad) = vectors.iter().find(|v| v.len() != self.dimension) {
            return Err(EmbedError::InvalidResponse(format!(
                "expected dimension {}, got {}",
                self.dimension,
                bad.len()
            )));
        }
        Ok(vectors)
    }
}
