//! 설정 파일과 데이터 경로. (DR-1, FR-SWP-3)

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::i18n::Lang;
use crate::redact::ExtraPattern;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config read failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("embedder url must point to localhost: {0}")]
    NonLocalUrl(String),
    #[error("chunking overlap_chars ({overlap}) must be smaller than the chunk limit ({limit})")]
    ChunkOverlap { overlap: usize, limit: usize },
    #[error("inject cold_budget_ms ({cold}) must not be smaller than budget_ms ({budget})")]
    ColdBudget { cold: u64, budget: u64 },
}

/// Qwen3-Embedding이 권장하는 검색 질의 지시문. (FR-RET-11)
pub const QWEN_QUERY_PREFIX: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbedderConfig {
    pub backend: String,
    pub model: String,
    pub dimension: usize,
    pub max_input_tokens: usize,
    pub url: String,
    /// Ollama가 모델을 메모리에 유지하는 시간. 첫 호출의 모델 로딩 지연을 피한다.
    pub keep_alive: String,
    /// 서버에 연결할 수 없으면 `ollama_command serve`를 띄운다. (FR-EMB-1)
    pub autostart: bool,
    pub ollama_command: String,
    /// 벡터 검색 질의 앞에 붙여 임베딩하는 지시문. 저장 텍스트에는 붙이지 않는다. (FR-RET-11)
    pub query_prefix: String,
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
    /// 이웃한 청크가 공유하는 문자 수. (FR-STO-5)
    pub overlap_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbedTextConfig {
    pub max_tool_input_chars: usize,
    /// 결과를 `[answer]`로 임베딩 텍스트에 넣는 사용자 응답 도구. (FR-STO-2)
    pub answer_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InjectSection {
    pub threshold: f32,
    pub top_k: usize,
    pub total_chars: usize,
    pub budget_ms: u64,
    /// hook이 데몬을 새로 띄운 요청의 예산. (FR-RET-5)
    pub cold_budget_ms: u64,
    /// 공백을 뺀 길이가 이보다 짧은 프롬프트는 주입하지 않는다. (FR-RET-1)
    pub min_prompt_chars: usize,
    /// false면 주입을 일시정지한다. (FR-RET-12)
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureSection {
    /// false면 저장을 일시정지한다. (FR-CAP-8)
    pub enabled: bool,
}

impl Default for CaptureSection {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    pub vector_index_threshold: u64,
    pub job_batch: usize,
    /// 벡터 저장소 수정이 이 수에 이르면 정리한다. 0이면 큐가 빌 때만 정리한다. (FR-STO-12)
    pub optimize_every: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonSection {
    /// 연결할 수 없으면 hook과 CLI가 데몬을 띄운다. (FR-DMN-7)
    pub autostart: bool,
    /// 마지막 요청 후 이 시간(초) 동안 요청이 없으면 종료한다. 0이면 종료하지 않는다. (FR-DMN-8)
    pub idle_timeout_secs: u64,
    /// 열어 둘 프로젝트 저장소 수. (FR-DMN-9)
    pub max_open_projects: usize,
    /// 처리할 작업이 있는 동안 시스템 유휴 잠자기를 막는다. (FR-DMN-13)
    pub prevent_sleep: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSection {
    /// git 실행 시간 제한(밀리초). 넘으면 git이 아닌 경로로 본다. (A-5)
    pub git_timeout_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSection {
    /// 사람이 읽는 출력의 언어. (FR-UI-1)
    pub language: Lang,
    /// GUI 테마. (FR-UI-3)
    pub theme: Theme,
    /// GUI 창을 열 때의 탭. (FR-UI-4)
    pub start_tab: StartTab,
}

/// GUI 창을 열 때 보여 줄 탭. `Last`는 마지막으로 본 탭이다. (FR-UI-4)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartTab {
    #[default]
    About,
    Status,
    Records,
    Projects,
    Search,
    Import,
    ClaudeSetup,
    Settings,
    Last,
}

impl StartTab {
    pub const ALL: [StartTab; 9] = [
        StartTab::About,
        StartTab::Status,
        StartTab::Records,
        StartTab::Projects,
        StartTab::Search,
        StartTab::Import,
        StartTab::ClaudeSetup,
        StartTab::Settings,
        StartTab::Last,
    ];

    /// 설정 파일에 쓰는 값.
    pub fn as_str(self) -> &'static str {
        match self {
            StartTab::About => "about",
            StartTab::Status => "status",
            StartTab::Records => "records",
            StartTab::Projects => "projects",
            StartTab::Search => "search",
            StartTab::Import => "import",
            StartTab::ClaudeSetup => "claude_setup",
            StartTab::Settings => "settings",
            StartTab::Last => "last",
        }
    }
}

/// GUI 테마. `System`은 운영 체제의 라이트·다크 모드를 따른다. (FR-UI-3)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    /// 설정 파일에 쓰는 값.
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImportSection {
    /// 과거 transcript를 찾는 디렉터리. (FR-IMP-2)
    pub transcripts_dir: PathBuf,
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
    pub capture: CaptureSection,
    pub index: IndexConfig,
    pub redact: RedactConfig,
    pub daemon: DaemonSection,
    pub project: ProjectSection,
    pub import: ImportSection,
    pub ui: UiSection,
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
        let c = &config.chunking;
        let limit = (c.max_tokens as f64 * c.chars_per_token).floor() as usize;
        if c.overlap_chars >= limit {
            return Err(ConfigError::ChunkOverlap {
                overlap: c.overlap_chars,
                limit,
            });
        }
        let i = &config.inject;
        if i.cold_budget_ms < i.budget_ms {
            return Err(ConfigError::ColdBudget {
                cold: i.cold_budget_ms,
                budget: i.budget_ms,
            });
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

    pub fn registry(&self) -> PathBuf {
        self.root.join("registry.sqlite")
    }
    pub fn projects(&self) -> PathBuf {
        self.root.join("projects")
    }
    pub fn project_dir(&self, id: &str) -> PathBuf {
        self.projects().join(id)
    }
    pub fn project_records(&self, id: &str) -> PathBuf {
        self.project_dir(id).join("records.sqlite")
    }
    pub fn project_vectors(&self, id: &str, name: &str) -> PathBuf {
        self.project_dir(id).join("vectors").join(name)
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
    /// GUI가 마지막으로 본 탭. (FR-GUI-25)
    pub fn gui_last_tab(&self) -> PathBuf {
        self.root.join("gui-last-tab")
    }
    pub fn socket(&self) -> PathBuf {
        self.root.join("hippo.sock")
    }
    pub fn lock(&self) -> PathBuf {
        self.root.join("hippo.lock")
    }
}

const LOCAL_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// URL을 파서로 해석해 host가 로컬 루프백인지 확인한다. userinfo, 쿼리, 프래그먼트가 있으면 거부한다. (C-4)
pub fn is_local_url(url: &str) -> bool {
    if url.contains('#') {
        return false;
    }
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.query().is_some() {
        return false;
    }
    let Some(authority) = uri.authority() else {
        return false;
    };
    if authority.as_str().contains('@') {
        return false;
    }
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
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
            keep_alive: "30m".into(),
            autostart: true,
            ollama_command: "ollama".into(),
            query_prefix: QWEN_QUERY_PREFIX.into(),
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
            max_tokens: 1_000,
            chars_per_token: 1.0,
            overlap_chars: 200,
        }
    }
}
impl Default for EmbedTextConfig {
    fn default() -> Self {
        Self {
            max_tool_input_chars: 500,
            answer_tools: vec!["AskUserQuestion".into(), "ExitPlanMode".into()],
        }
    }
}
impl Default for InjectSection {
    fn default() -> Self {
        Self {
            threshold: 0.50,
            top_k: 3,
            total_chars: 8_000,
            budget_ms: 800,
            cold_budget_ms: 2_500,
            min_prompt_chars: 11,
            enabled: true,
        }
    }
}
impl Default for DaemonSection {
    fn default() -> Self {
        Self {
            autostart: true,
            idle_timeout_secs: 1_800,
            max_open_projects: 8,
            prevent_sleep: true,
        }
    }
}

impl Default for ProjectSection {
    fn default() -> Self {
        Self {
            git_timeout_ms: 1_000,
        }
    }
}

impl Default for ImportSection {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            transcripts_dir: home.join(".claude").join("projects"),
        }
    }
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            vector_index_threshold: 100_000,
            job_batch: 64,
            optimize_every: 20,
        }
    }
}
