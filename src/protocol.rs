//! 데몬 소켓 요청과 응답. 연결 하나에 JSON 한 줄씩 주고받는다. (IF-SK-1, IF-SK-2)

use serde::{Deserialize, Serialize};

use crate::ingest::IngestRequest;
use crate::search::SearchRequest;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ingest(IngestRequest),
    Inject {
        prompt: String,
        cwd: String,
        session_id: String,
    },
    Search(SearchRequest),
    Show {
        prompt_id: String,
        with_results: bool,
    },
    Status,
    Reindex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub iterations: u64,
    pub results: u64,
    pub embed_jobs: u64,
    pub spool_pending: u64,
    pub vectors: u64,
    pub embed_model: String,
    /// `empty`, `compatible`, `mismatch`
    pub compatibility: String,
    /// `idle`, `running`, `done`, `failed: <사유>`
    pub reindex: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Done,
    Text { text: String },
    Inject { context: Option<String> },
    Status(StatusInfo),
    Error { message: String },
}

impl StatusInfo {
    /// `smem status` 출력. 재색인이 필요하면 안내를 붙인다. (FR-SWP-10)
    pub fn render(&self) -> String {
        let mut out = format!(
            "iterations: {}\nresults: {}\nembed_jobs: {}\nspool_pending: {}\nvectors: {}\nembed_model: {}\ncompatibility: {}\nreindex: {}\n",
            self.iterations,
            self.results,
            self.embed_jobs,
            self.spool_pending,
            self.vectors,
            self.embed_model,
            self.compatibility,
            self.reindex
        );
        let idle = self.reindex != REINDEX_RUNNING;
        if idle && self.compatibility == COMPAT_MISMATCH {
            out.push_str("벡터 저장소의 모델 또는 차원이 설정과 다릅니다. `smem reindex`로 다시 색인하십시오. 그동안 키워드 검색만 동작합니다.\n");
        } else if idle && self.vectors == 0 && self.iterations > 0 {
            out.push_str("저장된 기록은 있지만 벡터가 없습니다. `smem reindex`로 색인하십시오.\n");
        }
        out
    }
}

pub const COMPAT_MISMATCH: &str = "mismatch";
pub const REINDEX_RUNNING: &str = "running";
