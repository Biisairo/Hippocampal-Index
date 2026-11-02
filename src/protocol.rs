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
