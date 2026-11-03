//! 임베더 추상화. (FR-SWP-1)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const GATE_POLL: Duration = Duration::from_millis(5);

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding backend unavailable: {0}")]
    Unavailable(String),
    #[error("invalid embedding response: {0}")]
    InvalidResponse(String),
    #[error("invalid embedder config: {0}")]
    Config(String),
}

pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn dimension(&self) -> usize;
    fn max_input_tokens(&self) -> usize;
    /// 입력 순서대로 `dimension()` 길이의 벡터를 돌려준다.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// 처리 중인 자동 주입 요청 수. (FR-DMN-6 (a))
#[derive(Debug, Default)]
pub struct InjectGate {
    active: AtomicUsize,
}

impl InjectGate {
    pub fn enter(&self) -> InjectGuard<'_> {
        self.active.fetch_add(1, Ordering::SeqCst);
        InjectGuard(self)
    }

    pub fn is_busy(&self) -> bool {
        self.active.load(Ordering::SeqCst) > 0
    }
}

pub struct InjectGuard<'a>(&'a InjectGate);

impl Drop for InjectGuard<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 자동 주입이 처리 중이면 끝날 때까지 기다린 뒤 요청을 보낸다. (FR-DMN-6 (a))
pub struct YieldingEmbedder<'a> {
    pub inner: &'a dyn Embedder,
    pub gate: &'a InjectGate,
}

impl Embedder for YieldingEmbedder<'_> {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        while self.gate.is_busy() {
            thread::sleep(GATE_POLL);
        }
        self.inner.embed(texts)
    }
}
