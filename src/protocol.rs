//! 데몬 소켓 요청과 응답. 연결 하나에 JSON 한 줄씩 주고받는다. (IF-SK-1, IF-SK-2)

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::import::Candidate;
use crate::ingest::{ImportOutcome, IngestRequest};
use crate::search::{SearchKind, escape_control};
use crate::tr;

/// 요청이 다룰 프로젝트. `Project`는 요청의 `cwd`가 속한 프로젝트, `Path`는 그 경로가 속한 프로젝트이다. (IF-CLI-1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Scope {
    Project,
    All,
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ingest(IngestRequest),
    Inject {
        prompt: String,
        cwd: String,
        session_id: String,
        /// 클라이언트가 적용한 시간 예산. 없으면 데몬 설정의 `budget_ms`. (FR-RET-5)
        #[serde(default)]
        budget_ms: Option<u64>,
    },
    Search {
        query: String,
        kind: SearchKind,
        limit: usize,
        scope: Scope,
        cwd: String,
    },
    /// `scope`가 없으면 현재 프로젝트를 먼저 찾고 나머지를 찾는다. (FR-RET-7)
    Show {
        prompt_id: String,
        with_results: bool,
        scope: Option<Scope>,
        cwd: String,
    },
    Status,
    Reindex {
        scope: Scope,
        cwd: String,
    },
    ProjectList,
    /// 원본·대상 프로젝트를 정하고 후보 세션을 돌려준다. (FR-PRJ-7, FR-IMP-2)
    ImportPrepare {
        from: String,
        to: String,
    },
    /// 세션 하나를 가져온다. (FR-IMP-3)
    ImportSession {
        project_id: String,
        session_id: String,
        transcript_path: PathBuf,
    },
}

/// `import_session`의 응답. (FR-IMP-7)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSessionReport {
    pub outcome: ImportOutcome,
    pub iterations: usize,
    pub completions: usize,
    pub fork_skipped: usize,
    pub pending_embeds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectStatus {
    pub id: String,
    pub location: String,
    pub exists: bool,
    pub iterations: u64,
    pub results: u64,
    pub embed_jobs: u64,
    pub vectors: u64,
    /// `empty`, `compatible`, `mismatch`
    pub compatibility: String,
    /// 최근 5분 동안의 임베딩 처리 속도(분당 작업 수). 처리한 작업이 없으면 없다. (IF-CLI-5)
    pub embed_per_min: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub spool_pending: u64,
    pub embed_model: String,
    /// `idle`, `running`, `done`, `failed: <프로젝트 id>: <사유>; …`
    pub reindex: String,
    pub projects: Vec<ProjectStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Done,
    Text {
        text: String,
    },
    Inject {
        context: Option<String>,
    },
    Status(StatusInfo),
    ImportPlan {
        project_id: String,
        summary: String,
        sessions: Vec<Candidate>,
    },
    ImportSession(ImportSessionReport),
    Error {
        message: String,
    },
}

impl StatusInfo {
    /// `hippo status` 출력. 재색인이 필요한 프로젝트가 있으면 안내를 붙인다. (IF-CLI-5, FR-SWP-10)
    pub fn render(&self, lang: Lang) -> String {
        let mut out = format!(
            "spool_pending: {}\nembed_model: {}\nreindex: {}\nprojects: {}\n",
            self.spool_pending,
            self.embed_model,
            self.reindex,
            self.projects.len()
        );
        for p in &self.projects {
            let missing = if p.exists {
                String::new()
            } else {
                missing_location(lang)
            };
            out.push_str(&format!(
                "[{}] {}{missing}\n  iterations: {}  results: {}  embed_jobs: {}{}  vectors: {}  compatibility: {}\n",
                p.id,
                p.location,
                p.iterations,
                p.results,
                p.embed_jobs,
                embed_progress(p.embed_jobs, p.embed_per_min, lang),
                p.vectors,
                p.compatibility
            ));
        }
        if self.reindex == REINDEX_RUNNING {
            return escape_control(&out);
        }
        if self
            .projects
            .iter()
            .any(|p| p.compatibility == COMPAT_MISMATCH)
        {
            out.push_str(&tr!(
                lang,
                "Some projects have a vector store whose model or dimension differs from the config. Run `hippo reindex`. Until then those projects use keyword search only.\n",
                "벡터 저장소의 모델 또는 차원이 설정과 다른 프로젝트가 있습니다. `hippo reindex`로 다시 색인하십시오. 그동안 그 프로젝트는 키워드 검색만 동작합니다.\n"
            ));
        } else if self
            .projects
            .iter()
            .any(|p| p.vectors == 0 && p.iterations > 0)
        {
            out.push_str(&tr!(
                lang,
                "Some projects have records but no vectors. Run `hippo reindex`.\n",
                "저장된 기록은 있지만 벡터가 없는 프로젝트가 있습니다. `hippo reindex`로 색인하십시오.\n"
            ));
        }
        escape_control(&out)
    }
}

pub const COMPAT_MISMATCH: &str = "mismatch";
/// 대기 건수 뒤에 붙이는 속도와 남은 예상 시간. 대기 건수나 속도가 없으면 빈 문자열. (IF-CLI-5)
pub fn embed_progress(pending: u64, per_min: Option<u64>, lang: Lang) -> String {
    let Some(rate) = per_min.filter(|r| *r > 0 && pending > 0) else {
        return String::new();
    };
    let minutes = pending.div_ceil(rate);
    tr!(
        lang,
        " ({rate}/min, about {minutes} min left)",
        " (분당 {rate}건, 약 {minutes}분 남음)"
    )
}

/// 위치가 사라진 프로젝트 표시. (IF-CLI-5, FR-PRJ-6)
pub fn missing_location(lang: Lang) -> String {
    tr!(lang, "  (location missing)", "  (위치 없음)")
}
pub const REINDEX_RUNNING: &str = "running";
