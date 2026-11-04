//! 화면과 분리한 GUI 상태 로직. (FR-GUI-7, FR-GUI-8, FR-GUI-10, FR-GUI-13~17)

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::commands::{ImportTally, absolute, session_line};
use crate::i18n::Lang;
use crate::import::{Candidate, ImportStatus};
use crate::protocol::{Request, Response, Scope, StatusInfo};
use crate::registry::is_project_id;
use crate::search::{SkipReason, Verdict};
use crate::store::ForgetTarget;
use crate::tr;

/// 가져오기 탭의 입력과 미리 보기 결과. (FR-GUI-7)
#[derive(Debug, Clone, Default)]
pub struct ImportForm {
    pub from: String,
    pub to: String,
    pub summary: String,
    pub candidates: Vec<Candidate>,
    pub selected: Vec<bool>,
    previewed: Option<(String, String)>,
}

impl ImportForm {
    pub fn new(cwd: &str) -> Self {
        Self {
            from: cwd.to_string(),
            to: cwd.to_string(),
            ..Self::default()
        }
    }

    /// 미리 보기 결과를 넣는다. `새 세션`과 `실시간만`을 처음에 선택한다.
    pub fn apply_preview(
        &mut self,
        from: String,
        to: String,
        summary: String,
        candidates: Vec<Candidate>,
    ) {
        self.selected = candidates
            .iter()
            .map(|c| matches!(c.status, ImportStatus::New | ImportStatus::LiveOnly))
            .collect();
        self.candidates = candidates;
        self.summary = summary;
        self.previewed = Some((from, to));
    }

    /// 지금 입력으로 미리 보기를 했는지. 아니면 가져오기를 할 수 없다.
    pub fn preview_current(&self) -> bool {
        self.previewed
            .as_ref()
            .is_some_and(|(from, to)| *from == self.from && *to == self.to)
    }

    pub fn set_all(&mut self, on: bool) {
        self.selected.iter_mut().for_each(|s| *s = on);
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.candidates
            .iter()
            .zip(&self.selected)
            .filter(|(_, on)| **on)
            .map(|(c, _)| c.session_id.clone())
            .collect()
    }
}

/// 비어 있으면 실행 경로, 프로젝트 id는 그대로, 상대 경로는 실행 경로 기준으로 바꾼다.
pub fn resolve_import_args(cwd: &str, from: &str, to: &str) -> (String, String) {
    let path = |arg: &str| {
        let arg = arg.trim();
        if arg.is_empty() {
            cwd.to_string()
        } else {
            absolute(cwd, arg)
        }
    };
    let from = if is_project_id(from.trim()) {
        from.trim().to_string()
    } else {
        path(from)
    };
    (from, path(to))
}

/// 가져오기 작업 스레드가 쓰고 화면이 주기마다 읽는 진행 상황. (FR-GUI-8)
#[derive(Debug, Clone, Default)]
pub struct ImportProgress {
    pub total: usize,
    pub done: usize,
    pub lines: Vec<String>,
    pub summary: Option<String>,
    pub project_id: Option<String>,
    pub error: Option<String>,
    pub finished: bool,
}

fn update(progress: &Mutex<ImportProgress>, f: impl FnOnce(&mut ImportProgress)) {
    if let Ok(mut p) = progress.lock() {
        f(&mut p);
    }
}

/// 실제 준비를 요청한 뒤 응답 순서대로 선택한 세션만 보낸다. `stop`이 켜지면 다음 세션을 보내지 않는다. (FR-GUI-7, FR-GUI-8)
pub fn run_import(
    send: &mut dyn FnMut(&Request) -> Result<Response, String>,
    from: &str,
    to: &str,
    selected: &[String],
    stop: &AtomicBool,
    progress: &Mutex<ImportProgress>,
    lang: Lang,
) {
    let prepare = Request::ImportPrepare {
        from: from.to_string(),
        to: to.to_string(),
        dry_run: false,
    };
    let (project_id, sessions) = match send(&prepare) {
        Ok(Response::ImportPlan {
            project_id,
            sessions,
            ..
        }) => (project_id, sessions),
        Ok(Response::Error { message }) | Err(message) => {
            update(progress, |p| {
                p.error = Some(message);
                p.finished = true;
            });
            return;
        }
        Ok(other) => {
            update(progress, |p| {
                p.error = Some(format!("unexpected response: {other:?}"));
                p.finished = true;
            });
            return;
        }
    };
    let chosen: Vec<Candidate> = sessions
        .into_iter()
        .filter(|c| selected.contains(&c.session_id))
        .collect();
    let total = chosen.len();
    update(progress, |p| {
        p.total = total;
        p.project_id = Some(project_id.clone());
    });
    let mut tally = ImportTally::default();
    let mut done = 0;
    for c in chosen {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let req = Request::ImportSession {
            project_id: project_id.clone(),
            session_id: c.session_id.clone(),
            transcript_path: c.transcript_path,
        };
        let line = match send(&req) {
            Ok(Response::ImportSession(r)) => {
                session_line(done + 1, total, &c.session_id, Ok(&r), &mut tally, lang)
            }
            Ok(Response::Error { message }) | Err(message) => session_line(
                done + 1,
                total,
                &c.session_id,
                Err(&message),
                &mut tally,
                lang,
            ),
            Ok(other) => {
                let message = format!("unexpected response: {other:?}");
                session_line(
                    done + 1,
                    total,
                    &c.session_id,
                    Err(&message),
                    &mut tally,
                    lang,
                )
            }
        };
        done += 1;
        update(progress, |p| {
            p.done = done;
            p.lines.push(line);
        });
    }
    let summary = tally.summary(done, lang);
    update(progress, |p| {
        p.summary = Some(summary);
        p.finished = true;
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAction {
    Install,
    Uninstall,
}

/// 미리 보기를 한 입력. 같은 입력이어야 적용·해제를 할 수 있다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewKey {
    root: String,
    claude_dir: String,
    action: InstallAction,
}

/// Claude 적용 탭의 입력. (FR-GUI-10, FR-GUI-22)
#[derive(Debug, Clone, Default)]
pub struct InstallForm {
    pub root: String,
    pub claude_dir: String,
    previewed: Option<PreviewKey>,
}

impl InstallForm {
    pub fn new(root: &Path, claude_dir: &Path) -> Self {
        Self {
            root: root.display().to_string(),
            claude_dir: claude_dir.display().to_string(),
            previewed: None,
        }
    }

    pub fn key(&self, action: InstallAction) -> PreviewKey {
        PreviewKey {
            root: self.root.clone(),
            claude_dir: self.claude_dir.clone(),
            action,
        }
    }

    pub fn mark_previewed(&mut self, key: PreviewKey) {
        self.previewed = Some(key);
    }

    pub fn clear_preview(&mut self) {
        self.previewed = None;
    }

    pub fn can_run(&self, action: InstallAction) -> bool {
        self.previewed.as_ref() == Some(&self.key(action))
    }
}

/// 모든 프로젝트의 임베딩 큐 합계. (FR-GUI-3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueSummary {
    pub live: u64,
    pub import: u64,
    /// 처리 속도 합계(분당 작업 수). 최근에 처리한 작업이 없으면 없다.
    pub per_min: Option<u64>,
}

pub fn queue_summary(st: &StatusInfo) -> QueueSummary {
    let mut sum = QueueSummary {
        live: 0,
        import: 0,
        per_min: None,
    };
    for p in &st.projects {
        let import = p.embed_jobs_import.min(p.embed_jobs);
        sum.import += import;
        sum.live += p.embed_jobs - import;
        if let Some(rate) = p.embed_per_min {
            sum.per_min = Some(sum.per_min.unwrap_or(0) + rate);
        }
    }
    sum
}

/// 상태 폴링을 막는 조건들. (FR-GUI-3, FR-GUI-10)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollGate {
    /// 설치 상태가 `설치됨`인지.
    pub installed: bool,
    /// 수동 중지나 완전 삭제 뒤의 멈춤 상태.
    pub paused: bool,
    /// 수동 중지가 진행 중인지.
    pub stopping: bool,
    /// Claude 적용 탭의 작업이 진행 중인지.
    pub install_busy: bool,
}

/// 폴링은 데몬을 띄우므로, 막는 조건이 하나라도 있으면 하지 않는다.
pub fn should_poll(gate: PollGate) -> bool {
    gate.installed && !gate.paused && !gate.stopping && !gate.install_busy
}

/// 완전 삭제는 확인을 받았고 진행 중인 상태 요청이 없을 때 시작한다. (FR-GUI-10)
pub fn purge_may_start(confirmed: bool, status_running: bool) -> bool {
    confirmed && !status_running
}

/// 기록 탭의 한 페이지 크기. (FR-GUI-13)
pub const PAGE_SIZE: usize = 50;

/// 기록 탭에서 지울 대상. (FR-GUI-15)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteTarget {
    Project(String),
    Session { project: String, session_id: String },
    Turn { project: String, prompt_id: String },
}

impl DeleteTarget {
    pub fn request(&self, cwd: &str, dry_run: bool) -> Request {
        let forget = |project: &str, target: ForgetTarget| Request::Forget {
            target,
            scope: Some(Scope::Id(project.to_string())),
            cwd: cwd.to_string(),
            dry_run,
        };
        match self {
            Self::Project(id) => Request::ProjectRemove {
                project: id.clone(),
                cwd: cwd.to_string(),
                dry_run,
            },
            Self::Session {
                project,
                session_id,
            } => forget(project, ForgetTarget::Session(session_id.clone())),
            Self::Turn { project, prompt_id } => {
                forget(project, ForgetTarget::Prompt(prompt_id.clone()))
            }
        }
    }
}

/// 다시 불러올 목록.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reload {
    pub sessions: bool,
    pub turns: bool,
    pub detail: bool,
}

/// 기록 탭의 선택, 필터, 페이지. 위 단계를 바꾸면 아래 단계의 선택을 지운다. (FR-GUI-13~17)
#[derive(Debug, Clone, Default)]
pub struct RecordsView {
    pub project: Option<String>,
    pub session: Option<String>,
    pub prompt: Option<String>,
    pub session_filter: String,
    pub turn_filter: String,
    pub session_page: usize,
    pub turn_page: usize,
    pub reload: Reload,
}

impl RecordsView {
    /// 다른 프로젝트를 골랐으면 참이다. (FR-GUI-16)
    pub fn select_project(&mut self, id: &str) -> bool {
        if self.project.as_deref() == Some(id) {
            return false;
        }
        self.project = Some(id.to_string());
        self.session = None;
        self.prompt = None;
        self.session_page = 0;
        self.turn_page = 0;
        self.reload = Reload {
            sessions: true,
            ..Reload::default()
        };
        true
    }

    pub fn select_session(&mut self, id: &str) {
        self.session = Some(id.to_string());
        self.prompt = None;
        self.turn_page = 0;
        self.reload.turns = true;
    }

    pub fn select_turn(&mut self, prompt_id: &str) {
        self.prompt = Some(prompt_id.to_string());
        self.reload.detail = true;
    }

    pub fn set_session_filter(&mut self, filter: &str) {
        self.session_filter = filter.to_string();
        self.session_page = 0;
        self.reload.sessions = true;
    }

    pub fn set_turn_filter(&mut self, filter: &str) {
        self.turn_filter = filter.to_string();
        self.turn_page = 0;
        self.reload.turns = true;
    }

    /// 마지막 페이지를 넘지 않게 바꾼다.
    pub fn session_page_to(&mut self, page: usize, total: u64) {
        self.session_page = page.min(page_count(total) - 1);
        self.reload.sessions = true;
    }

    pub fn turn_page_to(&mut self, page: usize, total: u64) {
        self.turn_page = page.min(page_count(total) - 1);
        self.reload.turns = true;
    }

    pub fn session_request(&self, cwd: &str) -> Option<Request> {
        Some(Request::SessionList {
            scope: Scope::Id(self.project.clone()?),
            cwd: cwd.to_string(),
            filter: self.session_filter.clone(),
            offset: self.session_page * PAGE_SIZE,
            limit: PAGE_SIZE,
        })
    }

    pub fn turn_request(&self, cwd: &str) -> Option<Request> {
        Some(Request::TurnList {
            scope: Scope::Id(self.project.clone()?),
            cwd: cwd.to_string(),
            session_id: self.session.clone()?,
            filter: self.turn_filter.clone(),
            offset: self.turn_page * PAGE_SIZE,
            limit: PAGE_SIZE,
        })
    }

    pub fn detail_request(&self, cwd: &str) -> Option<Request> {
        Some(Request::TurnDetail {
            scope: Scope::Id(self.project.clone()?),
            cwd: cwd.to_string(),
            prompt_id: self.prompt.clone()?,
        })
    }

    /// 지운 대상의 선택을 지우고, 수가 바뀐 목록을 다시 불러오게 한다. (FR-GUI-15)
    pub fn after_delete(&mut self, target: &DeleteTarget) {
        match target {
            DeleteTarget::Project(id) => {
                if self.project.as_deref() == Some(id) {
                    *self = Self {
                        session_filter: std::mem::take(&mut self.session_filter),
                        turn_filter: std::mem::take(&mut self.turn_filter),
                        ..Self::default()
                    };
                }
            }
            DeleteTarget::Session { session_id, .. } => {
                if self.session.as_deref() == Some(session_id) {
                    self.session = None;
                    self.prompt = None;
                }
                self.reload.sessions = true;
            }
            DeleteTarget::Turn { prompt_id, .. } => {
                if self.prompt.as_deref() == Some(prompt_id) {
                    self.prompt = None;
                }
                self.reload.sessions = true;
                self.reload.turns = self.session.is_some();
            }
        }
    }

    /// 검색 결과의 턴을 연다. 세션 선택은 지운다. (FR-GUI-17)
    pub fn open_turn(&mut self, project: &str, prompt_id: &str) {
        self.project = None;
        self.select_project(project);
        self.select_turn(prompt_id);
    }
}

/// 전체 개수에 필요한 페이지 수. 최소 1.
pub fn page_count(total: u64) -> usize {
    usize::try_from(total.div_ceil(PAGE_SIZE as u64))
        .unwrap_or(usize::MAX)
        .max(1)
}

/// 기록 탭이 고를 프로젝트. 현재 프로젝트가 목록에 있으면 그것, 없으면 첫 항목. (FR-GUI-18)
pub fn default_project(ids: &[String], current: Option<&str>) -> Option<String> {
    current
        .and_then(|c| ids.iter().find(|id| id.as_str() == c))
        .or_else(|| ids.first())
        .cloned()
}

/// 성공 토스트가 보이는 시간. (FR-GUI-19)
pub const SUCCESS_TOAST_FOR: Duration = Duration::from_secs(5);

/// 작업 결과 메시지. 성공은 잠시 뒤 사라지고 오류는 닫을 때까지 남는다. (FR-GUI-19)
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    error: bool,
    shown_at: Instant,
}

impl Toast {
    pub fn success(text: String, now: Instant) -> Self {
        Self {
            text,
            error: false,
            shown_at: now,
        }
    }

    pub fn error(text: String, now: Instant) -> Self {
        Self {
            text,
            error: true,
            shown_at: now,
        }
    }

    pub fn is_error(&self) -> bool {
        self.error
    }

    pub fn visible(&self, now: Instant) -> bool {
        self.error || now.duration_since(self.shown_at) < SUCCESS_TOAST_FOR
    }
}

/// 떠 있는 토스트들. 새 메시지는 따로 더하고, 오류는 닫을 때만 지운다. (FR-GUI-19)
#[derive(Debug, Default)]
pub struct Toasts(Vec<Toast>);

impl Toasts {
    pub fn push(&mut self, toast: Toast) {
        self.0.push(toast);
    }

    /// 시간이 지난 성공 메시지를 지운다.
    pub fn expire(&mut self, now: Instant) {
        self.0.retain(|t| t.visible(now));
    }

    pub fn close(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }

    pub fn items(&self) -> &[Toast] {
        &self.0
    }
}

/// 턴의 임베딩 상태. (FR-GUI-13)
pub fn embed_state(pending: u64, lang: Lang) -> String {
    if pending == 0 {
        tr!(lang, "done", "완료")
    } else {
        tr!(lang, "pending {pending}", "대기 {pending}")
    }
}

pub fn verdict_label(verdict: Verdict, lang: Lang) -> &'static str {
    match (verdict, lang) {
        (Verdict::Selected, Lang::En) => "selected",
        (Verdict::BelowThreshold, Lang::En) => "below threshold",
        (Verdict::FtsOnly, Lang::En) => "FTS only",
        (Verdict::SameTurn, Lang::En) => "same turn",
        (Verdict::OverLength, Lang::En) => "over length limit",
        (Verdict::OverCount, Lang::En) => "over count",
        (Verdict::Selected, Lang::Ko) => "선택",
        (Verdict::BelowThreshold, Lang::Ko) => "임계값 미만",
        (Verdict::FtsOnly, Lang::Ko) => "FTS 전용",
        (Verdict::SameTurn, Lang::Ko) => "같은 턴",
        (Verdict::OverLength, Lang::Ko) => "길이 한도 초과",
        (Verdict::OverCount, Lang::Ko) => "개수 초과",
    }
}

pub fn skip_label(reason: SkipReason, lang: Lang) -> &'static str {
    match (reason, lang) {
        (SkipReason::EmptyPrompt, Lang::En) => "empty prompt",
        (SkipReason::TaskNotification, Lang::En) => "task notification",
        (SkipReason::TooShort, Lang::En) => "shorter than min_prompt_chars",
        (SkipReason::NoProject, Lang::En) => "no project",
        (SkipReason::VectorUnavailable, Lang::En) => "vector search unavailable",
        (SkipReason::EmptyPrompt, Lang::Ko) => "빈 프롬프트",
        (SkipReason::TaskNotification, Lang::Ko) => "완료 알림",
        (SkipReason::TooShort, Lang::Ko) => "min_prompt_chars 미만",
        (SkipReason::NoProject, Lang::Ko) => "프로젝트 없음",
        (SkipReason::VectorUnavailable, Lang::Ko) => "벡터 검색 불가",
    }
}
