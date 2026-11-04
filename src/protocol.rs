//! 데몬 소켓 요청과 응답. 연결 하나에 JSON 한 줄씩 주고받는다. (IF-SK-1, IF-SK-2)

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::import::Candidate;
use crate::ingest::{ImportOutcome, IngestRequest};
use crate::search::{HitView, InjectionPlan, IterationView, SearchKind, escape_control};
use crate::store::{ForgetCounts, ForgetTarget, Page, SessionSummary, TurnSummary};
use crate::tr;

/// 요청이 다룰 프로젝트. `Project`는 요청의 `cwd`가 속한 프로젝트, `Path`는 그 경로가 속한 프로젝트,
/// `Id`는 그 id의 프로젝트이다. (IF-CLI-1, IF-SK-2)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Scope {
    Project,
    All,
    Path(String),
    Id(String),
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
        /// 참이면 텍스트 대신 결과 목록(`hits`)을 돌려준다. (FR-GUI-16, FR-GUI-17)
        #[serde(default)]
        structured: bool,
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
        /// 목록 DB와 원본 저장소를 바꾸지 않고 결과만 돌려준다. (FR-IMP-9)
        #[serde(default)]
        dry_run: bool,
    },
    /// 세션 하나를 가져온다. (FR-IMP-3)
    ImportSession {
        project_id: String,
        session_id: String,
        transcript_path: PathBuf,
    },
    /// 데몬을 유휴 종료와 같은 경로로 끝낸다. (FR-DMN-11)
    Shutdown,
    /// 프로젝트를 통째로 지운다. `project`는 프로젝트 id 또는 경로이다. (FR-DEL-1, IF-CLI-11)
    ProjectRemove {
        project: String,
        cwd: String,
        #[serde(default)]
        dry_run: bool,
    },
    /// 세션 또는 턴을 지운다. 프로젝트를 찾는 순서는 `show`와 같다. (FR-DEL-2, FR-DEL-3, IF-CLI-12)
    Forget {
        target: ForgetTarget,
        scope: Option<Scope>,
        cwd: String,
        #[serde(default)]
        dry_run: bool,
    },
    /// 프로젝트의 세션 목록 한 페이지. (FR-GUI-13, FR-GUI-14)
    SessionList {
        scope: Scope,
        cwd: String,
        filter: String,
        offset: usize,
        limit: usize,
    },
    /// 세션의 턴 목록 한 페이지. (FR-GUI-13, FR-GUI-14)
    TurnList {
        scope: Scope,
        cwd: String,
        session_id: String,
        filter: String,
        offset: usize,
        limit: usize,
    },
    /// 턴 상세. (FR-GUI-13)
    TurnDetail {
        scope: Scope,
        cwd: String,
        prompt_id: String,
    },
    /// 주입 미리 보기. 세션 제외와 시간 예산을 적용하지 않는다. (FR-GUI-16)
    InjectPreview {
        scope: Scope,
        cwd: String,
        prompt: String,
    },
    /// `cwd`의 프로젝트. 조회 전용 판정이다. (FR-GUI-18)
    CurrentProject {
        cwd: String,
    },
}

/// 삭제 요청의 응답 내용. (FR-DEL-4)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteReport {
    pub project_id: String,
    pub location: String,
    pub counts: ForgetCounts,
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
        /// 미리 보기에서 원본 프로젝트. (FR-IMP-9)
        #[serde(default)]
        source_id: Option<String>,
        summary: String,
        sessions: Vec<Candidate>,
    },
    ImportSession(ImportSessionReport),
    /// 삭제 미리 보기 또는 결과. 대상이 없으면 `None`. (FR-DEL-4)
    Delete {
        report: Option<DeleteReport>,
    },
    /// 구조화 검색 결과. `notes`는 벡터 검색을 쓰지 못한 안내이다. (FR-GUI-16, FR-RET-8)
    Hits {
        hits: Vec<HitView>,
        notes: Vec<String>,
    },
    Sessions(Page<SessionSummary>),
    Turns(Page<TurnSummary>),
    /// 턴 상세. 반복이 없으면 `iterations`가 비어 있다. (FR-GUI-13)
    Turn {
        project_id: String,
        iterations: Vec<IterationView>,
    },
    /// 주입 미리 보기와 걸린 시간. (FR-GUI-16)
    InjectPreview {
        plan: InjectionPlan,
        elapsed_ms: u64,
    },
    /// 프로젝트가 없으면 `None`. (FR-GUI-18)
    CurrentProject {
        id: Option<String>,
    },
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
