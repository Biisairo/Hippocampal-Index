//! GUI 화면. 데몬 요청과 파일 작업은 작업 스레드에서 하고, 화면은 2초마다 갱신한다. (FR-GUI-1~12)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::client::request;
use crate::commands::render_delete_report;
use crate::commands::{SESSION_ID_SHORT, project_scope, query, run_install, scope_arg};
use crate::config::PatternEntry;
use crate::config::{Config, Paths};
use crate::config_edit::{
    Field, FieldValue, SaveOutcome, changed, fields, needs_reindex, needs_restart, save,
};
use crate::gui::state::{
    DeleteTarget, ImportForm, ImportProgress, InstallAction, InstallForm, PreviewKey, RecordsView,
    Reload, embed_state, page_count, resolve_import_args, run_import, skip_label, verdict_label,
};
use crate::gui::text::describe;
use crate::gui::widgets::{
    FIELD_HEIGHT, apply_style, banner, button, card, cell, form, mono_cell, output, primary, stat,
    text_field,
};
use crate::i18n::{Lang, local_time};
use crate::import::{Candidate, ImportStatus, size_label, started_label};
use crate::install::{Scope, is_installed, target_file};
use crate::protocol::{
    DeleteReport, Request, Response, Scope as QueryScope, StatusInfo, embed_progress,
    missing_location,
};
use crate::search::{HitView, InjectionPlan, IterationView, SearchKind};
use crate::store::{Page, RecordKind, SessionSummary, TurnSummary};
use crate::tr;

/// 상태 폴링과 화면 갱신 주기. (FR-GUI-3, FR-GUI-8)
const POLL: Duration = Duration::from_secs(2);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const WINDOW_SIZE: [f32; 2] = [1180.0, 820.0];
const MIN_WINDOW_SIZE: [f32; 2] = [900.0, 600.0];
const LOG_HEIGHT: f32 = 180.0;
const ROW_HEIGHT: f32 = 26.0;
const SETTING_KEY_WIDTH: f32 = 300.0;
const TEXT_FIELD_WIDTH: f32 = 420.0;
const LIST_BUTTON_WIDTH: f32 = 40.0;
const NUMBER_WIDTH: f32 = 120.0;
const PATTERN_KIND_WIDTH: f32 = 110.0;
const MODAL_WIDTH: f32 = 420.0;
const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 190, 120);
const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 170, 60);
const CHANGED_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 170, 240);
const DEFAULT_LIMIT: usize = 10;
const RECORDS_SEARCH_LIMIT: usize = 20;
const QUERY_HEIGHT: f32 = 220.0;
const TS_CHARS: usize = 19;
const TOOL_INPUT_CHARS: usize = 200;
const README_EN: &str = include_str!("../../README.md");
const README_KO: &str = include_str!("../../README.ko.md");
const LICENSE: &str = include_str!("../../LICENSE");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 창을 열고 닫힐 때까지 돌아온다. (IF-CLI-10)
pub fn run(paths: Paths, cwd: String) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("hippo")
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
        ..Default::default()
    };
    eframe::run_native(
        "hippo",
        options,
        Box::new(move |cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(App::new(paths, cwd)))
        }),
    )
    .map_err(|e| e.to_string())
}

/// 작업 스레드 하나의 결과 칸.
struct Task<T> {
    slot: Arc<Mutex<Option<T>>>,
    running: bool,
}

impl<T> Default for Task<T> {
    fn default() -> Self {
        Self {
            slot: Arc::new(Mutex::new(None)),
            running: false,
        }
    }
}

impl<T: Send + 'static> Task<T> {
    fn start(&mut self, ctx: &egui::Context, job: impl FnOnce() -> T + Send + 'static) {
        if self.running {
            return;
        }
        self.running = true;
        let slot = Arc::clone(&self.slot);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let out = job();
            if let Ok(mut s) = slot.lock() {
                *s = Some(out);
            }
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) -> Option<T> {
        let out = self.slot.lock().ok()?.take();
        if out.is_some() {
            self.running = false;
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Records,
    Settings,
    Import,
    Search,
    Projects,
    Install,
    About,
}

impl Tab {
    const ALL: [Tab; 8] = [
        Tab::Status,
        Tab::Records,
        Tab::Settings,
        Tab::Import,
        Tab::Search,
        Tab::Projects,
        Tab::Install,
        Tab::About,
    ];

    fn label(self, lang: Lang) -> String {
        match self {
            Tab::Status => tr!(lang, "Status", "상태"),
            Tab::Records => tr!(lang, "Records", "기록"),
            Tab::Settings => tr!(lang, "Settings", "설정"),
            Tab::Import => tr!(lang, "Import", "가져오기"),
            Tab::Search => tr!(lang, "Search", "검색"),
            Tab::Projects => tr!(lang, "Projects", "프로젝트"),
            Tab::Install => tr!(lang, "Claude setup", "Claude 적용"),
            Tab::About => tr!(lang, "About", "정보"),
        }
    }
}

type Reply = Result<Response, String>;
/// (저장한 편집값, 바꾼 키, 결과)
type SaveResult = (Vec<Field>, Vec<Field>, Result<SaveOutcome, String>);

fn text_of(reply: Reply) -> String {
    match reply {
        Ok(Response::Text { text }) => text,
        Ok(Response::Done) => "OK\n".into(),
        Ok(Response::Error { message }) | Err(message) => format!("error: {message}\n"),
        Ok(other) => format!("unexpected response: {other:?}\n"),
    }
}

#[derive(Default)]
struct Settings {
    loaded: Vec<Field>,
    edited: Vec<Field>,
    defaults: Vec<Field>,
    error: Option<String>,
    message: Option<String>,
    restart: bool,
    reindex: bool,
    load: Task<Result<Vec<Field>, String>>,
    save: Task<SaveResult>,
    daemon: Task<Reply>,
}

struct Search {
    query: String,
    scope: String,
    kind: SearchKind,
    limit: usize,
    prompt_id: String,
    show_scope: String,
    with_results: bool,
    output: String,
    /// 구조화 검색 결과와 안내. 있으면 `output` 대신 보여 준다. (FR-GUI-17)
    hits: Option<(Vec<HitView>, Vec<String>)>,
    task: Task<Reply>,
}

/// 기록 탭의 질의 결과. (FR-GUI-16)
enum QueryOut {
    Hits(Vec<HitView>, Vec<String>),
    Preview(InjectionPlan, u64),
}

/// 확인 창에 보여 줄 삭제 미리 보기. (FR-GUI-15)
type DeletePreview = (DeleteTarget, Result<Option<DeleteReport>, String>);

#[derive(Default)]
struct Records {
    view: RecordsView,
    sessions: Option<Result<Page<SessionSummary>, String>>,
    turns: Option<Result<Page<TurnSummary>, String>>,
    detail: Option<Result<Vec<IterationView>, String>>,
    session_filter: String,
    turn_filter: String,
    query: String,
    preview_mode: bool,
    query_out: Option<Result<QueryOut, String>>,
    confirm: Option<DeletePreview>,
    message: Option<String>,
    sessions_task: Task<Reply>,
    turns_task: Task<Reply>,
    detail_task: Task<Reply>,
    query_task: Task<Reply>,
    preview_task: Task<(DeleteTarget, Reply)>,
    delete_task: Task<(DeleteTarget, Reply)>,
}

#[derive(Default)]
struct Projects {
    output: String,
    reindex_scope: String,
    message: String,
    list: Task<Reply>,
    reindex: Task<Reply>,
}

type PreviewOutcome = (String, String, Result<(String, Vec<Candidate>), String>);
type InstallOutcome = (Option<PreviewKey>, Result<String, String>);

struct Install {
    form: InstallForm,
    output: String,
    installed: Option<bool>,
    checked: Option<(bool, String)>,
    confirm: Option<InstallAction>,
    check: Task<Option<bool>>,
    task: Task<InstallOutcome>,
}

impl Records {
    /// 끝난 작업의 결과를 받는다. 프로젝트를 지웠으면 참이다(상태를 바로 다시 묻는다).
    fn poll(&mut self, lang: Lang) -> bool {
        if let Some(reply) = self.sessions_task.poll() {
            self.sessions = Some(match reply {
                Ok(Response::Sessions(page)) => Ok(page),
                other => Err(error_of(other)),
            });
        }
        if let Some(reply) = self.turns_task.poll() {
            self.turns = Some(match reply {
                Ok(Response::Turns(page)) => Ok(page),
                other => Err(error_of(other)),
            });
        }
        if let Some(reply) = self.detail_task.poll() {
            self.detail = Some(match reply {
                Ok(Response::Turn { iterations, .. }) => Ok(iterations),
                other => Err(error_of(other)),
            });
        }
        if let Some(reply) = self.query_task.poll() {
            self.query_out = Some(match reply {
                Ok(Response::Hits { hits, notes }) => Ok(QueryOut::Hits(hits, notes)),
                Ok(Response::InjectPreview { plan, elapsed_ms }) => {
                    Ok(QueryOut::Preview(plan, elapsed_ms))
                }
                other => Err(error_of(other)),
            });
        }
        if let Some((target, reply)) = self.preview_task.poll() {
            self.confirm = Some((
                target,
                match reply {
                    Ok(Response::Delete { report }) => Ok(report),
                    other => Err(error_of(other)),
                },
            ));
        }
        if let Some((target, reply)) = self.delete_task.poll() {
            match reply {
                Ok(Response::Delete { .. }) => {
                    self.view.after_delete(&target);
                    self.message = Some(tr!(lang, "Deleted.", "지웠습니다."));
                    return matches!(target, DeleteTarget::Project(_));
                }
                other => self.message = Some(error_of(other)),
            }
        }
        false
    }

    /// 선택이 없어진 단계의 목록을 비우고, 다시 불러올 목록을 요청한다.
    fn start_loads(&mut self, ctx: &egui::Context, paths: &Paths, cwd: &str) {
        if self.view.project.is_none() {
            self.sessions = None;
        }
        if self.view.session.is_none() {
            self.turns = None;
        }
        if self.view.prompt.is_none() {
            self.detail = None;
        }
        let start = |task: &mut Task<Reply>, req: Option<Request>| {
            if let Some(req) = req {
                let paths = paths.clone();
                task.start(ctx, move || query(&paths, &req, QUERY_TIMEOUT));
            }
        };
        if self.view.reload.sessions && !self.sessions_task.running {
            self.view.reload.sessions = false;
            start(&mut self.sessions_task, self.view.session_request(cwd));
        }
        if self.view.reload.turns && !self.turns_task.running {
            self.view.reload.turns = false;
            start(&mut self.turns_task, self.view.turn_request(cwd));
        }
        if self.view.reload.detail && !self.detail_task.running {
            self.view.reload.detail = false;
            start(&mut self.detail_task, self.view.detail_request(cwd));
        }
    }

    /// `dry_run` 요청으로 미리 보기를 받아 확인 창을 연다. (FR-GUI-15)
    fn ask_delete(&mut self, ctx: &egui::Context, paths: &Paths, cwd: &str, target: DeleteTarget) {
        if self.delete_task.running {
            return;
        }
        self.message = None;
        let req = target.request(cwd, true);
        let paths = paths.clone();
        self.preview_task
            .start(ctx, move || (target, query(&paths, &req, QUERY_TIMEOUT)));
    }
}

pub struct App {
    paths: Paths,
    cwd: String,
    lang: Lang,
    tab: Tab,
    status: Option<Result<StatusInfo, String>>,
    status_at: String,
    status_task: Task<Result<StatusInfo, String>>,
    next_poll: Instant,
    settings: Settings,
    import: ImportForm,
    preview: Task<PreviewOutcome>,
    import_error: Option<String>,
    progress: Arc<Mutex<ImportProgress>>,
    stop: Arc<AtomicBool>,
    import_started: bool,
    search: Search,
    records: Records,
    projects: Projects,
    install: Install,
    about_license: bool,
}

impl App {
    fn new(paths: Paths, cwd: String) -> Self {
        let lang = Config::load(&paths.config()).map_or(Lang::En, |c| c.ui.language);
        Self {
            lang,
            tab: Tab::Status,
            status: None,
            status_at: String::new(),
            status_task: Task::default(),
            next_poll: Instant::now(),
            settings: Settings {
                defaults: fields(&Config::default()),
                ..Settings::default()
            },
            import: ImportForm::new(&cwd),
            preview: Task::default(),
            import_error: None,
            progress: Arc::new(Mutex::new(ImportProgress::default())),
            stop: Arc::new(AtomicBool::new(false)),
            import_started: false,
            search: Search {
                query: String::new(),
                scope: "project".into(),
                kind: SearchKind::Iteration,
                limit: DEFAULT_LIMIT,
                prompt_id: String::new(),
                show_scope: String::new(),
                with_results: false,
                output: String::new(),
                hits: None,
                task: Task::default(),
            },
            records: Records::default(),
            projects: Projects {
                reindex_scope: "all".into(),
                ..Projects::default()
            },
            install: Install {
                form: InstallForm::new(&cwd),
                output: String::new(),
                installed: None,
                checked: None,
                confirm: None,
                check: Task::default(),
                task: Task::default(),
            },
            about_license: false,
            paths,
            cwd,
        }
    }

    /// 2초마다 상태를 묻는다. 데몬이 없으면 설정에 따라 띄운다. (FR-GUI-3)
    fn poll_status(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.status_task.poll() {
            self.status = Some(result);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            self.status_at = local_time(now);
        }
        if Instant::now() >= self.next_poll && !self.status_task.running {
            self.next_poll = Instant::now() + POLL;
            let paths = self.paths.clone();
            self.status_task.start(ctx, move || {
                match query(&paths, &Request::Status, QUERY_TIMEOUT)? {
                    Response::Status(s) => Ok(s),
                    Response::Error { message } => Err(message),
                    other => Err(format!("unexpected response: {other:?}")),
                }
            });
        }
        ctx.request_repaint_after(POLL);
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let st = match &self.status {
            None => {
                ui.weak(tr!(lang, "Waiting for the daemon…", "데몬을 기다리는 중…"));
                return;
            }
            Some(Err(e)) => {
                banner(ui, ui.visuals().error_fg_color, |ui| {
                    ui.label(tr!(lang, "Not connected: {e}", "연결 안 됨: {e}"));
                });
                return;
            }
            Some(Ok(st)) => st,
        };
        ui.horizontal(|ui| {
            stat(
                ui,
                &tr!(lang, "Daemon", "데몬"),
                RichText::new(tr!(lang, "running", "실행 중")).color(OK_COLOR),
            );
            stat(
                ui,
                &tr!(lang, "Spool pending", "스풀 대기"),
                RichText::new(st.spool_pending.to_string()),
            );
            stat(
                ui,
                &tr!(lang, "Reindex", "재색인"),
                RichText::new(&st.reindex),
            );
            stat(
                ui,
                &tr!(lang, "Embedding model", "임베딩 모델"),
                RichText::new(&st.embed_model),
            );
        });
        ui.weak(tr!(
            lang,
            "Last update: {}",
            "마지막 갱신: {}",
            self.status_at
        ));
        ui.add_space(4.0);
        card(ui, &tr!(lang, "Projects", "프로젝트"), |ui| {
            project_table(ui, st, lang);
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let s = &mut self.settings;
        if let Some(result) = s.load.poll() {
            match result {
                Ok(list) => {
                    s.loaded = list.clone();
                    s.edited = list;
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            }
        }
        if s.loaded.is_empty() && s.error.is_none() && !s.load.running {
            let path = self.paths.config();
            s.load.start(&ctx, move || {
                Config::load(&path)
                    .map(|c| fields(&c))
                    .map_err(|e| e.to_string())
            });
        }
        if let Some((snapshot, changes, result)) = s.save.poll() {
            match result {
                Ok(outcome) => {
                    s.loaded = snapshot;
                    s.restart |= needs_restart(&changes);
                    s.reindex |= needs_reindex(&changes);
                    if let Some(lang_field) = changes
                        .iter()
                        .find(|f| (f.section, f.key) == ("ui", "language"))
                    {
                        self.lang = if lang_field.value == FieldValue::Text("ko".into()) {
                            Lang::Ko
                        } else {
                            Lang::En
                        };
                    }
                    s.message = Some(match outcome {
                        SaveOutcome::Written => tr!(self.lang, "Saved.", "저장했습니다."),
                        SaveOutcome::Unchanged => tr!(self.lang, "No change.", "변경 없음."),
                    });
                }
                Err(e) => s.message = Some(tr!(lang, "Not saved: {e}", "저장하지 않았습니다: {e}")),
            }
        }
        if let Some(reply) = s.daemon.poll() {
            s.message = Some(match reply {
                Ok(Response::Done) if s.restart => {
                    s.restart = false;
                    tr!(
                        lang,
                        "Daemon stopped. The next status poll starts it with the new settings.",
                        "데몬을 멈췄습니다. 다음 상태 확인 때 새 설정으로 다시 시작합니다."
                    )
                }
                Ok(Response::Done) => {
                    s.reindex = false;
                    tr!(lang, "Reindex started.", "재색인을 시작했습니다.")
                }
                other => text_of(other),
            });
        }
        let lang = self.lang;
        if let Some(e) = &s.error {
            banner(ui, ui.visuals().error_fg_color, |ui| {
                ui.label(tr!(
                    lang,
                    "config.toml cannot be read, so saving is disabled: {e}",
                    "config.toml을 읽을 수 없어 저장을 막았습니다: {e}"
                ));
            });
            return;
        }
        let changes = changed(&s.loaded, &s.edited);
        ui.horizontal(|ui| {
            let can_save = !changes.is_empty() && !s.save.running;
            if primary(ui, can_save, tr!(lang, "Save", "저장")).clicked() {
                let path = self.paths.config();
                let snapshot = s.edited.clone();
                let changes = changes.clone();
                s.save.start(&ctx, move || {
                    let result = save(&path, &changes).map_err(|e| e.to_string());
                    (snapshot, changes, result)
                });
            }
            if ui
                .add_enabled(!changes.is_empty(), button(tr!(lang, "Revert", "되돌리기")))
                .clicked()
            {
                s.edited = s.loaded.clone();
            }
            if changes.is_empty() {
                ui.weak(tr!(lang, "No unsaved changes", "저장하지 않은 변경 없음"));
            } else {
                ui.label(
                    RichText::new(tr!(
                        lang,
                        "{} unsaved change(s)",
                        "저장하지 않은 변경 {}개",
                        changes.len()
                    ))
                    .color(CHANGED_COLOR),
                );
            }
            if let Some(m) = &s.message {
                ui.separator();
                ui.label(m);
            }
        });
        if s.restart || s.reindex {
            banner(ui, WARN_COLOR, |ui| {
                if s.restart {
                    ui.label(tr!(
                        lang,
                        "Restart the daemon to apply the changes.",
                        "변경을 적용하려면 데몬을 다시 시작하십시오."
                    ));
                    if s.reindex {
                        ui.label(tr!(lang, "Then reindex.", "그다음 재색인하십시오."));
                    }
                    if primary(
                        ui,
                        !s.daemon.running,
                        tr!(lang, "Restart daemon", "데몬 다시 시작"),
                    )
                    .clicked()
                    {
                        let socket = self.paths.socket();
                        s.daemon.start(&ctx, move || {
                            match request(&socket, &Request::Shutdown, QUERY_TIMEOUT) {
                                Err(_) => Ok(Response::Done),
                                Ok(r) => Ok(r),
                            }
                        });
                    }
                } else {
                    ui.label(tr!(
                        lang,
                        "The changed keys need a reindex.",
                        "바꾼 키는 재색인이 필요합니다."
                    ));
                    if primary(
                        ui,
                        !s.daemon.running,
                        tr!(lang, "Reindex all", "전체 재색인"),
                    )
                    .clicked()
                    {
                        let paths = self.paths.clone();
                        let cwd = self.cwd.clone();
                        s.daemon.start(&ctx, move || {
                            query(
                                &paths,
                                &Request::Reindex {
                                    scope: QueryScope::All,
                                    cwd,
                                },
                                QUERY_TIMEOUT,
                            )
                        });
                    }
                }
            });
        }
        ui.add_space(4.0);
        let defaults = s.defaults.clone();
        let loaded = s.loaded.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut sections: Vec<&'static str> = Vec::new();
                for f in &s.edited {
                    if !sections.contains(&f.section) {
                        sections.push(f.section);
                    }
                }
                for section in sections {
                    card(ui, &format!("[{section}]"), |ui| {
                        egui::Grid::new(("settings", section))
                            .num_columns(3)
                            .spacing([16.0, 12.0])
                            .show(ui, |ui| {
                                for field in s.edited.iter_mut().filter(|f| f.section == section) {
                                    let default = defaults
                                        .iter()
                                        .find(|d| d.section == field.section && d.key == field.key);
                                    let modified = !loaded.contains(field);
                                    setting_row(ui, field, default, modified, lang);
                                    ui.end_row();
                                }
                            });
                    });
                    ui.add_space(6.0);
                }
            });
    }

    fn import_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        if let Some((from, to, result)) = self.preview.poll() {
            match result {
                Ok((summary, sessions)) => {
                    self.import.apply_preview(from, to, summary, sessions);
                    self.import_error = None;
                }
                Err(e) => self.import_error = Some(e),
            }
        }
        let progress = self.progress.lock().map(|p| p.clone()).unwrap_or_default();
        let running = self.import_started && !progress.finished;
        card(
            ui,
            &tr!(lang, "Source and target", "원본과 대상"),
            |ui| {
                let hint = tr!(lang, "path or project id", "경로 또는 프로젝트 id");
                form(ui, "import-args", |ui, w| {
                    ui.label(tr!(lang, "From", "원본"));
                    ui.add_enabled_ui(!running, |ui| {
                        text_field(ui, w, &mut self.import.from, &hint)
                    });
                    ui.end_row();
                    ui.label(tr!(lang, "To", "대상"));
                    ui.add_enabled_ui(!running, |ui| text_field(ui, w, &mut self.import.to, ""));
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !running && !self.preview.running,
                            button(tr!(lang, "Preview", "미리 보기")),
                        )
                        .clicked()
                    {
                        let (raw_from, raw_to) = (self.import.from.clone(), self.import.to.clone());
                        let (from, to) = resolve_import_args(&self.cwd, &raw_from, &raw_to);
                        let paths = self.paths.clone();
                        self.preview.start(&ctx, move || {
                            let req = Request::ImportPrepare {
                                from,
                                to,
                                dry_run: true,
                            };
                            let result = match query(&paths, &req, QUERY_TIMEOUT) {
                                Ok(Response::ImportPlan {
                                    summary, sessions, ..
                                }) => Ok((summary, sessions)),
                                Ok(Response::Error { message }) | Err(message) => Err(message),
                                Ok(other) => Err(format!("unexpected response: {other:?}")),
                            };
                            (raw_from, raw_to, result)
                        });
                    }
                    let current = self.import.preview_current();
                    let selected = self.import.selected_ids();
                    if primary(
                        ui,
                        current && !running && !selected.is_empty(),
                        tr!(
                            lang,
                            "Import {} selected",
                            "선택한 {}개 가져오기",
                            selected.len()
                        ),
                    )
                    .clicked()
                    {
                        self.start_import(selected);
                    }
                    if running && ui.add(button(tr!(lang, "Stop", "멈춤"))).clicked() {
                        self.stop.store(true, Ordering::SeqCst);
                    }
                    if self.preview.running {
                        ui.spinner();
                    }
                    if !current && !self.import.candidates.is_empty() {
                        ui.colored_label(
                            WARN_COLOR,
                            tr!(
                                lang,
                                "Inputs changed. Preview again.",
                                "입력이 바뀌었습니다. 다시 미리 보기를 하십시오."
                            ),
                        );
                    }
                });
                if let Some(e) = &self.import_error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if !self.import.summary.is_empty() {
                    ui.label(self.import.summary.trim_end());
                }
            },
        );
        ui.add_space(4.0);
        if self.import_started {
            card(ui, &tr!(lang, "Progress", "진행"), |ui| {
                if progress.total > 0 {
                    ui.add(
                        egui::ProgressBar::new(progress.done as f32 / progress.total as f32)
                            .text(format!("{}/{}", progress.done, progress.total)),
                    );
                }
                if let Some(e) = &progress.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                egui::ScrollArea::vertical()
                    .id_salt("import-log")
                    .max_height(LOG_HEIGHT)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &progress.lines {
                            ui.monospace(line);
                        }
                        if let Some(summary) = &progress.summary {
                            ui.label(summary.trim_end());
                        }
                    });
                if progress.finished
                    && let (Some(id), Some(Ok(st))) = (&progress.project_id, &self.status)
                    && let Some(p) = st.projects.iter().find(|p| &p.id == id)
                {
                    ui.label(tr!(
                        lang,
                        "Pending embeddings: {}{}",
                        "남은 임베딩: {}{}",
                        p.embed_jobs,
                        embed_progress(p.embed_jobs, p.embed_per_min, lang)
                    ));
                }
            });
            ui.add_space(4.0);
        }
        if !self.import.candidates.is_empty() {
            let title = tr!(
                lang,
                "Candidate sessions ({} of {} selected)",
                "후보 세션 ({}개 중 {}개 선택)",
                self.import.selected_ids().len(),
                self.import.candidates.len()
            );
            card(ui, &title, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(tr!(lang, "Select all", "전체 선택")).clicked() {
                        self.import.set_all(true);
                    }
                    if ui.button(tr!(lang, "Select none", "전체 해제")).clicked() {
                        self.import.set_all(false);
                    }
                });
                candidate_table(ui, &mut self.import, lang);
            });
        }
    }

    fn start_import(&mut self, selected: Vec<String>) {
        let (from, to) = resolve_import_args(&self.cwd, &self.import.from, &self.import.to);
        if let Ok(mut p) = self.progress.lock() {
            *p = ImportProgress::default();
        }
        self.stop.store(false, Ordering::SeqCst);
        self.import_started = true;
        let paths = self.paths.clone();
        let progress = Arc::clone(&self.progress);
        let stop = Arc::clone(&self.stop);
        let lang = self.lang;
        thread::spawn(move || {
            let mut send = |req: &Request| query(&paths, req, QUERY_TIMEOUT);
            run_import(&mut send, &from, &to, &selected, &stop, &progress, lang);
        });
    }

    fn search_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let s = &mut self.search;
        if let Some(reply) = s.task.poll() {
            match reply {
                Ok(Response::Hits { hits, notes }) => {
                    s.hits = Some((hits, notes));
                    s.output.clear();
                }
                other => {
                    s.hits = None;
                    s.output = text_of(other);
                }
            }
        }
        let mut search_clicked = false;
        let mut show_clicked = false;
        ui.columns(2, |cols| {
            card(&mut cols[0], &tr!(lang, "Search", "검색"), |ui| {
                form(ui, "search-args", |ui, w| {
                    ui.label(tr!(lang, "Query", "질의"));
                    let r = text_field(ui, w, &mut s.query, "");
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        search_clicked = true;
                    }
                    ui.end_row();
                    ui.label(tr!(lang, "Scope", "범위"));
                    text_field(ui, w, &mut s.scope, "project | all | <path>");
                    ui.end_row();
                    ui.label(tr!(lang, "Kind / limit", "종류 / 개수"));
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("kind")
                            .selected_text(kind_name(s.kind))
                            .show_ui(ui, |ui| {
                                for kind in
                                    [SearchKind::Iteration, SearchKind::Result, SearchKind::Both]
                                {
                                    ui.selectable_value(&mut s.kind, kind, kind_name(kind));
                                }
                            });
                        ui.add(egui::DragValue::new(&mut s.limit).range(1..=100));
                    });
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if primary(
                        ui,
                        !s.task.running && !s.query.trim().is_empty(),
                        tr!(lang, "Search", "검색"),
                    )
                    .clicked()
                    {
                        search_clicked = true;
                    }
                });
            });
            card(&mut cols[1], &tr!(lang, "Show a turn", "턴 보기"), |ui| {
                let hint = tr!(lang, "empty | project | <path>", "비움 | project | <경로>");
                form(ui, "show-args", |ui, w| {
                    ui.label("prompt_id");
                    text_field(ui, w, &mut s.prompt_id, "");
                    ui.end_row();
                    ui.label(tr!(lang, "Scope", "범위"));
                    text_field(ui, w, &mut s.show_scope, &hint);
                    ui.end_row();
                    ui.label(tr!(lang, "Results", "결과"));
                    ui.checkbox(
                        &mut s.with_results,
                        tr!(lang, "include tool results", "도구 결과 포함"),
                    );
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if primary(
                        ui,
                        !s.task.running && !s.prompt_id.trim().is_empty(),
                        tr!(lang, "Show turn", "턴 보기"),
                    )
                    .clicked()
                    {
                        show_clicked = true;
                    }
                });
            });
        });
        if search_clicked && !s.task.running && !s.query.trim().is_empty() {
            let req = Request::Search {
                query: s.query.clone(),
                kind: s.kind,
                limit: s.limit,
                scope: scope_arg(&self.cwd, s.scope.trim()),
                cwd: self.cwd.clone(),
                structured: true,
            };
            let paths = self.paths.clone();
            s.task
                .start(&ctx, move || query(&paths, &req, QUERY_TIMEOUT));
        }
        if show_clicked {
            let scope = s.show_scope.trim();
            let req = Request::Show {
                prompt_id: s.prompt_id.trim().to_string(),
                with_results: s.with_results,
                scope: (!scope.is_empty()).then(|| scope_arg(&self.cwd, scope)),
                cwd: self.cwd.clone(),
            };
            let paths = self.paths.clone();
            s.task
                .start(&ctx, move || query(&paths, &req, QUERY_TIMEOUT));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!(lang, "Output", "결과")).strong());
            if s.task.running {
                ui.spinner();
            }
        });
        let mut open = None;
        match &s.hits {
            Some((hits, notes)) => {
                egui::ScrollArea::vertical()
                    .id_salt("search-hits")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        open = hit_rows(ui, hits, notes, true, lang);
                    });
            }
            None => output(
                ui,
                "search-output",
                &s.output,
                &tr!(lang, "Results appear here.", "결과가 여기에 표시됩니다."),
            ),
        }
        if let Some(hit) = open
            && let Some(prompt_id) = &hit.prompt_id
        {
            self.records.view.open_turn(&hit.project_id, prompt_id);
            self.tab = Tab::Records;
        }
    }

    /// 기록 탭. 프로젝트 → 세션 → 턴 → 턴 상세 순서로 본다. (FR-GUI-13~16)
    fn records_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let cwd = self.cwd.clone();
        let paths = self.paths.clone();
        let projects: Vec<(String, String)> = match &self.status {
            Some(Ok(st)) => st
                .projects
                .iter()
                .map(|p| (p.id.clone(), p.location.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let r = &mut self.records;
        if r.poll(lang) {
            self.next_poll = Instant::now();
        }
        if r.view
            .project
            .as_ref()
            .is_none_or(|p| !projects.iter().any(|(id, _)| id == p))
            && let Some((id, _)) = projects.first()
        {
            r.view.select_project(id);
        }
        let mut delete = None;
        ui.horizontal(|ui| {
            ui.label(tr!(lang, "Project", "프로젝트"));
            let selected = r
                .view
                .project
                .as_ref()
                .map(|id| project_label(&projects, id))
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("records-project")
                .width(TEXT_FIELD_WIDTH)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (id, _) in &projects {
                        let on = r.view.project.as_deref() == Some(id.as_str());
                        if ui
                            .selectable_label(on, project_label(&projects, id))
                            .clicked()
                        {
                            r.view.select_project(id);
                        }
                    }
                });
            if ui.add(button(tr!(lang, "Refresh", "새로 고침"))).clicked() {
                r.view.reload = Reload {
                    sessions: true,
                    turns: true,
                    detail: true,
                };
            }
            if let Some(id) = &r.view.project
                && ui
                    .add(button(tr!(lang, "Delete project", "프로젝트 삭제")))
                    .clicked()
            {
                delete = Some(DeleteTarget::Project(id.clone()));
            }
            if let Some(message) = &r.message {
                ui.separator();
                ui.label(message);
            }
        });
        if projects.is_empty() {
            ui.weak(tr!(lang, "No project yet.", "아직 프로젝트가 없습니다."));
            return;
        }
        ui.add_space(4.0);
        let mut open_prompt = None;
        card(
            ui,
            &tr!(lang, "Query this project", "이 프로젝트에 질의"),
            |ui| {
                ui.horizontal(|ui| {
                    let field = text_field(
                        ui,
                        TEXT_FIELD_WIDTH,
                        &mut r.query,
                        &tr!(lang, "query or prompt", "질의 또는 프롬프트"),
                    );
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.radio_value(&mut r.preview_mode, false, tr!(lang, "Search", "검색"));
                    ui.radio_value(
                        &mut r.preview_mode,
                        true,
                        tr!(lang, "Injection preview", "주입 미리 보기"),
                    );
                    let ready = !r.query_task.running && r.view.project.is_some();
                    if (primary(ui, ready, tr!(lang, "Run", "실행")).clicked() || enter)
                        && ready
                        && let Some(id) = r.view.project.clone()
                    {
                        let req = if r.preview_mode {
                            Request::InjectPreview {
                                scope: QueryScope::Id(id),
                                cwd: cwd.clone(),
                                prompt: r.query.clone(),
                            }
                        } else {
                            Request::Search {
                                query: r.query.clone(),
                                kind: SearchKind::Iteration,
                                limit: RECORDS_SEARCH_LIMIT,
                                scope: QueryScope::Id(id),
                                cwd: cwd.clone(),
                                structured: true,
                            }
                        };
                        let paths = paths.clone();
                        r.query_task
                            .start(&ctx, move || query(&paths, &req, QUERY_TIMEOUT));
                    }
                    if r.query_task.running {
                        ui.spinner();
                    }
                });
                if let Some(out) = &r.query_out {
                    egui::ScrollArea::vertical()
                        .id_salt("records-query")
                        .max_height(QUERY_HEIGHT)
                        .show(ui, |ui| match out {
                            Ok(QueryOut::Hits(hits, notes)) => {
                                open_prompt = hit_rows(ui, hits, notes, false, lang)
                                    .and_then(|h| h.prompt_id.clone());
                            }
                            Ok(QueryOut::Preview(plan, elapsed_ms)) => {
                                preview_view(ui, plan, *elapsed_ms, lang);
                            }
                            Err(e) => {
                                ui.colored_label(ui.visuals().error_fg_color, e);
                            }
                        });
                }
            },
        );
        if let Some(prompt_id) = open_prompt {
            r.view.select_turn(&prompt_id);
        }
        ui.add_space(4.0);
        let project = r.view.project.clone().unwrap_or_default();
        ui.columns(3, |cols| {
            let ui = &mut cols[0];
            ui.horizontal(|ui| {
                ui.strong(tr!(lang, "Sessions", "세션"));
                if let Some(session_id) = &r.view.session
                    && ui.small_button(tr!(lang, "Delete", "삭제")).clicked()
                {
                    delete = Some(DeleteTarget::Session {
                        project: project.clone(),
                        session_id: session_id.clone(),
                    });
                }
            });
            let filter_hint = tr!(
                lang,
                "filter (3+ chars: full text)",
                "필터 (3자 이상은 전문 검색)"
            );
            if text_field(
                ui,
                ui.available_width(),
                &mut r.session_filter,
                &filter_hint,
            )
            .changed()
            {
                r.view.set_session_filter(&r.session_filter);
            }
            match &r.sessions {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Some(Ok(page)) => {
                    if let Some(to) = pager(ui, r.view.session_page, page.total, lang) {
                        r.view.session_page_to(to, page.total);
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("records-sessions")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for item in &page.items {
                                let on =
                                    r.view.session.as_deref() == Some(item.session_id.as_str());
                                let label = tr!(
                                    lang,
                                    "{}  {}  turns {}  iterations {}\n{}",
                                    "{}  {}  턴 {}  반복 {}\n{}",
                                    ts_label(&item.started_at),
                                    short(&item.session_id),
                                    item.turns,
                                    item.iterations,
                                    item.first_prompt
                                );
                                if ui.selectable_label(on, label).clicked() {
                                    r.view.select_session(&item.session_id);
                                }
                            }
                        });
                }
            }

            let ui = &mut cols[1];
            ui.horizontal(|ui| {
                ui.strong(tr!(lang, "Turns", "턴"));
                if r.view.session.is_some()
                    && let Some(prompt_id) = &r.view.prompt
                    && ui.small_button(tr!(lang, "Delete", "삭제")).clicked()
                {
                    delete = Some(DeleteTarget::Turn {
                        project: project.clone(),
                        prompt_id: prompt_id.clone(),
                    });
                }
            });
            if r.view.session.is_none() {
                ui.weak(tr!(lang, "Select a session.", "세션을 고르십시오."));
            } else {
                if text_field(ui, ui.available_width(), &mut r.turn_filter, &filter_hint).changed()
                {
                    r.view.set_turn_filter(&r.turn_filter);
                }
                match &r.turns {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Some(Ok(page)) => {
                        if let Some(to) = pager(ui, r.view.turn_page, page.total, lang) {
                            r.view.turn_page_to(to, page.total);
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("records-turns")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for item in &page.items {
                                    let label = tr!(
                                        lang,
                                        "{}  {}  iterations {}  {}\n{}",
                                        "{}  {}  반복 {}  {}\n{}",
                                        ts_label(&item.ts),
                                        item.prompt_id.as_deref().map(short).unwrap_or_default(),
                                        item.iterations,
                                        embed_state(item.pending, lang),
                                        item.prompt
                                    );
                                    let Some(prompt_id) = &item.prompt_id else {
                                        ui.weak(label);
                                        continue;
                                    };
                                    let on = r.view.prompt.as_deref() == Some(prompt_id.as_str());
                                    if ui.selectable_label(on, label).clicked() {
                                        r.view.select_turn(prompt_id);
                                    }
                                }
                            });
                    }
                }
            }

            let ui = &mut cols[2];
            ui.horizontal(|ui| {
                ui.strong(tr!(lang, "Turn detail", "턴 상세"));
                if r.view.session.is_none()
                    && let Some(prompt_id) = &r.view.prompt
                    && ui.small_button(tr!(lang, "Delete", "삭제")).clicked()
                {
                    delete = Some(DeleteTarget::Turn {
                        project: project.clone(),
                        prompt_id: prompt_id.clone(),
                    });
                }
            });
            match (&r.view.prompt, &r.detail) {
                (None, _) => {
                    ui.weak(tr!(lang, "Select a turn.", "턴을 고르십시오."));
                }
                (Some(_), None) => {
                    ui.spinner();
                }
                (Some(_), Some(Err(e))) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                (Some(prompt_id), Some(Ok(iterations))) => {
                    mono_cell(ui, &format!("prompt {prompt_id}"));
                    egui::ScrollArea::vertical()
                        .id_salt("records-detail")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if iterations.is_empty() {
                                ui.weak(tr!(lang, "No records.", "기록 없음"));
                            }
                            for it in iterations {
                                iteration_view(ui, it, lang);
                            }
                        });
                }
            }
        });
        if let Some(target) = delete {
            r.ask_delete(&ctx, &paths, &cwd, target);
        }
        r.start_loads(&ctx, &paths, &cwd);
        if let Some((target, report)) = r.confirm.clone() {
            let mut close = false;
            let mut confirmed = false;
            let modal = egui::Modal::new(egui::Id::unique("records-delete")).show(&ctx, |ui| {
                ui.set_max_width(MODAL_WIDTH);
                ui.label(
                    RichText::new(tr!(lang, "Delete", "삭제"))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(4.0);
                match &report {
                    Ok(Some(report)) => {
                        ui.label(render_delete_report(report, lang));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if primary(ui, true, tr!(lang, "Delete", "삭제")).clicked() {
                                confirmed = true;
                            }
                            if ui.add(button(tr!(lang, "Cancel", "취소"))).clicked() {
                                close = true;
                            }
                        });
                    }
                    Ok(None) => {
                        ui.label(tr!(lang, "Nothing to delete.", "대상 없음"));
                        if ui.add(button("OK".into())).clicked() {
                            close = true;
                        }
                    }
                    Err(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                        if ui.add(button("OK".into())).clicked() {
                            close = true;
                        }
                    }
                }
            });
            if confirmed {
                let req = target.request(&cwd, false);
                let paths = paths.clone();
                r.delete_task
                    .start(&ctx, move || (target, query(&paths, &req, QUERY_TIMEOUT)));
            }
            if confirmed || close || modal.should_close() {
                r.confirm = None;
            }
        }
    }

    fn projects_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let p = &mut self.projects;
        if let Some(reply) = p.list.poll() {
            p.output = text_of(reply);
        }
        if let Some(reply) = p.reindex.poll() {
            p.message = match reply {
                Ok(Response::Done) => tr!(lang, "Reindex started.", "재색인을 시작했습니다."),
                other => text_of(other),
            };
        }
        if p.output.is_empty() && !p.list.running {
            let paths = self.paths.clone();
            p.list.start(&ctx, move || {
                query(&paths, &Request::ProjectList, QUERY_TIMEOUT)
            });
        }
        card(ui, &tr!(lang, "Reindex", "재색인"), |ui| {
            form(ui, "reindex-args", |ui, w| {
                ui.label(tr!(lang, "Scope", "범위"));
                text_field(ui, w, &mut p.reindex_scope, "all | project | <path>");
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if primary(ui, !p.reindex.running, tr!(lang, "Reindex", "재색인")).clicked() {
                    let req = Request::Reindex {
                        scope: scope_arg(&self.cwd, p.reindex_scope.trim()),
                        cwd: self.cwd.clone(),
                    };
                    let paths = self.paths.clone();
                    p.reindex
                        .start(&ctx, move || query(&paths, &req, QUERY_TIMEOUT));
                }
                if let Some(Ok(st)) = &self.status {
                    ui.label(tr!(lang, "State: {}", "상태: {}", st.reindex));
                }
                if !p.message.is_empty() {
                    ui.separator();
                    ui.label(p.message.trim_end());
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!(lang, "Project list", "프로젝트 목록")).strong());
            if ui
                .add_enabled(
                    !p.list.running,
                    egui::Button::new(tr!(lang, "Refresh", "새로 고침")),
                )
                .clicked()
            {
                let paths = self.paths.clone();
                p.list.start(&ctx, move || {
                    query(&paths, &Request::ProjectList, QUERY_TIMEOUT)
                });
            }
            if p.list.running {
                ui.spinner();
            }
        });
        output(ui, "project-list", &p.output, "");
    }

    fn install_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let i = &mut self.install;
        if let Some(installed) = i.check.poll() {
            i.installed = installed;
        }
        if let Some((key, result)) = i.task.poll() {
            match (key, result) {
                (Some(key), Ok(text)) => {
                    i.form.mark_previewed(key);
                    i.output = text;
                }
                (None, Ok(text)) => {
                    i.form.clear_preview();
                    i.output = text;
                    i.checked = None;
                }
                (_, Err(e)) => i.output = tr!(lang, "error: {e}\n", "오류: {e}\n"),
            }
        }
        card(ui, &tr!(lang, "Where to apply", "적용 범위"), |ui| {
            form(ui, "install-args", |ui, w| {
                ui.label(tr!(lang, "Scope", "범위"));
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut i.form.user_scope,
                        false,
                        tr!(lang, "Project", "프로젝트"),
                    );
                    ui.radio_value(&mut i.form.user_scope, true, tr!(lang, "User", "사용자"));
                });
                ui.end_row();
                ui.label(tr!(lang, "Project path", "프로젝트 경로"));
                ui.add_enabled_ui(!i.form.user_scope, |ui| {
                    text_field(ui, w, &mut i.form.project, "")
                });
                ui.end_row();
                ui.label(tr!(lang, "Settings file", "설정 파일"));
                ui.weak(if i.form.user_scope {
                    "~/.claude/settings.json, ~/.claude/CLAUDE.md"
                } else {
                    "<project>/.claude/settings.local.json, <project>/CLAUDE.local.md"
                });
                ui.end_row();
                ui.label(tr!(lang, "Current", "현재"));
                match i.installed {
                    Some(true) => ui.colored_label(OK_COLOR, tr!(lang, "● applied", "● 적용됨")),
                    Some(false) => ui.weak(tr!(lang, "○ not applied", "○ 적용 안 됨")),
                    None => ui.weak(tr!(lang, "unknown", "알 수 없음")),
                };
                ui.end_row();
            });
        });
        let inputs = (i.form.user_scope, i.form.project.clone());
        if i.checked.as_ref() != Some(&inputs) && !i.check.running {
            i.checked = Some(inputs);
            let scope = install_scope(&i.form, &self.cwd);
            i.check.start(&ctx, move || {
                let path = target_file(&scope.ok()?).ok()?;
                Some(is_installed(&path))
            });
        }
        ui.add_space(4.0);
        ui.columns(2, |cols| {
            let actions = [
                (
                    InstallAction::Install,
                    tr!(lang, "Apply", "적용"),
                    tr!(
                        lang,
                        "Add the hippo hooks and the guide block",
                        "hippo hook과 안내 블록을 넣습니다"
                    ),
                ),
                (
                    InstallAction::Uninstall,
                    tr!(lang, "Remove", "해제"),
                    tr!(
                        lang,
                        "Remove only the hippo hooks and the guide block",
                        "hippo hook과 안내 블록만 지웁니다"
                    ),
                ),
            ];
            for ((action, label, about), col) in actions.into_iter().zip(cols.iter_mut()) {
                card(col, &label, |ui| {
                    ui.weak(about);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!i.task.running, button(tr!(lang, "Preview", "미리 보기")))
                            .clicked()
                        {
                            let key = i.form.key(action);
                            let scope = install_scope(&i.form, &self.cwd);
                            let paths = self.paths.clone();
                            i.task.start(&ctx, move || {
                                (Some(key), install_text(&paths, scope, action, true, lang))
                            });
                        }
                        if primary(ui, !i.task.running && i.form.can_run(action), label.clone())
                            .clicked()
                        {
                            i.confirm = Some(action);
                        }
                        if !i.form.can_run(action) {
                            ui.weak(tr!(lang, "Preview first", "먼저 미리 보기"));
                        }
                    });
                });
            }
        });
        if let Some(action) = i.confirm {
            let modal = egui::Modal::new(egui::Id::unique("install-confirm")).show(&ctx, |ui| {
                ui.set_max_width(MODAL_WIDTH);
                ui.label(
                    RichText::new(tr!(lang, "Confirm", "확인"))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(4.0);
                ui.label(match action {
                    InstallAction::Install => tr!(
                        lang,
                        "Write the hippo hooks and guide block shown in the preview?",
                        "미리 보기의 hippo hook과 안내 블록을 쓸까요?"
                    ),
                    InstallAction::Uninstall => tr!(
                        lang,
                        "Remove the hippo hooks and guide block shown in the preview?",
                        "미리 보기의 hippo hook과 안내 블록을 지울까요?"
                    ),
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if primary(ui, true, tr!(lang, "Confirm", "확인")).clicked() {
                        let scope = install_scope(&i.form, &self.cwd);
                        let paths = self.paths.clone();
                        i.task.start(&ctx, move || {
                            (None, install_text(&paths, scope, action, false, lang))
                        });
                        i.confirm = None;
                    }
                    if ui.add(button(tr!(lang, "Cancel", "취소"))).clicked() {
                        i.confirm = None;
                    }
                });
            });
            if modal.should_close() {
                i.confirm = None;
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr!(lang, "Output", "결과")).strong());
            if i.task.running {
                ui.spinner();
            }
        });
        output(
            ui,
            "install-output",
            &i.output,
            &tr!(
                lang,
                "The preview diff appears here. Nothing is written until you confirm.",
                "미리 보기의 diff가 여기에 표시됩니다. 확인하기 전에는 아무것도 쓰지 않습니다."
            ),
        );
    }

    fn about_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("hippo {VERSION}"))
                    .strong()
                    .size(16.0),
            );
            ui.weak("· MIT");
            ui.separator();
            ui.selectable_value(&mut self.about_license, false, "README");
            ui.selectable_value(&mut self.about_license, true, "LICENSE");
        });
        let text = match (self.about_license, lang) {
            (true, _) => LICENSE,
            (false, Lang::En) => README_EN,
            (false, Lang::Ko) => README_KO,
        };
        output(ui, "about-text", text, "");
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_status(&ctx);
        let lang = self.lang;
        egui::Panel::top("tabs")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("hippo").strong().size(17.0));
                    ui.add_space(8.0);
                    for tab in Tab::ALL {
                        ui.selectable_value(
                            &mut self.tab,
                            tab,
                            RichText::new(tab.label(lang)).size(15.0),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &self.status {
                            Some(Ok(_)) => {
                                ui.colored_label(OK_COLOR, tr!(lang, "● daemon", "● 데몬"))
                            }
                            Some(Err(_)) => ui.colored_label(
                                ui.visuals().error_fg_color,
                                tr!(lang, "● daemon", "● 데몬"),
                            ),
                            None => ui.weak(tr!(lang, "○ daemon", "○ 데몬")),
                        };
                        ui.add(egui::Label::new(RichText::new(&self.cwd).weak()).truncate());
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(14)))
            .show(ui, |ui| match self.tab {
                Tab::Status => self.status_tab(ui),
                Tab::Records => self.records_tab(ui),
                Tab::Settings => self.settings_tab(ui),
                Tab::Import => self.import_tab(ui),
                Tab::Search => self.search_tab(ui),
                Tab::Projects => self.projects_tab(ui),
                Tab::Install => self.install_tab(ui),
                Tab::About => self.about_tab(ui),
            });
    }
}

fn error_of(reply: Reply) -> String {
    text_of(reply).trim_end().to_string()
}

fn short(id: &str) -> String {
    id.chars().take(SESSION_ID_SHORT).collect()
}

/// ISO 8601 시각을 `YYYY-MM-DD HH:MM:SS`로 보여 준다.
fn ts_label(ts: &str) -> String {
    ts.replace('T', " ").chars().take(TS_CHARS).collect()
}

fn project_label(projects: &[(String, String)], id: &str) -> String {
    let location = projects
        .iter()
        .find(|(p, _)| p == id)
        .map_or("", |(_, l)| l.as_str());
    format!("{}  {location}", short(id))
}

/// 이전·다음 페이지 버튼. 누르면 옮길 페이지를 돌려준다. (FR-GUI-13)
fn pager(ui: &mut egui::Ui, page: usize, total: u64, lang: Lang) -> Option<usize> {
    let pages = page_count(total);
    let mut to = None;
    ui.horizontal(|ui| {
        if ui.add_enabled(page > 0, egui::Button::new("◀")).clicked() {
            to = Some(page - 1);
        }
        ui.label(format!("{}/{pages}", page + 1));
        if ui
            .add_enabled(page + 1 < pages, egui::Button::new("▶"))
            .clicked()
        {
            to = Some(page + 1);
        }
        ui.weak(tr!(lang, "{total} total", "전체 {total}개"));
    });
    to
}

/// 검색 결과 목록. 누른 반복 결과를 돌려준다. (FR-GUI-16, FR-GUI-17)
fn hit_rows<'a>(
    ui: &mut egui::Ui,
    hits: &'a [HitView],
    notes: &[String],
    in_search_tab: bool,
    lang: Lang,
) -> Option<&'a HitView> {
    for note in notes {
        ui.colored_label(WARN_COLOR, note);
    }
    if hits.is_empty() {
        ui.weak(tr!(lang, "No results", "결과 없음"));
        return None;
    }
    let mut clicked = None;
    for (n, hit) in hits.iter().enumerate() {
        let sim = hit
            .similarity
            .map_or_else(|| "-".to_string(), |s| format!("{s:.3}"));
        let project = if in_search_tab {
            format!("project={}  ", short(&hit.project_id))
        } else {
            String::new()
        };
        let head = format!(
            "{}. [{}] {}  score={:.4} sim={sim}  {project}prompt={}",
            n + 1,
            hit.kind,
            hit.id,
            hit.score,
            hit.prompt_id.as_deref().unwrap_or("-")
        );
        ui.horizontal(|ui| {
            if hit.kind == RecordKind::Iteration.as_str() && hit.prompt_id.is_some() {
                let text = if in_search_tab {
                    tr!(lang, "Open in records", "기록에서 열기")
                } else {
                    tr!(lang, "Open", "열기")
                };
                if ui.small_button(text).clicked() {
                    clicked = Some(hit);
                }
            }
            ui.add(egui::Label::new(RichText::new(head).monospace()).truncate());
        });
        ui.add(egui::Label::new(RichText::new(&hit.preview).weak()).truncate());
    }
    clicked
}

/// 주입 미리 보기: 무주입 사유, 또는 후보별 판정과 최종 주입 텍스트. (FR-GUI-16)
fn preview_view(ui: &mut egui::Ui, plan: &InjectionPlan, elapsed_ms: u64, lang: Lang) {
    ui.weak(tr!(
        lang,
        "Took {elapsed_ms} ms",
        "걸린 시간 {elapsed_ms}ms"
    ));
    let (candidates, text) = match plan {
        InjectionPlan::Skipped { reason } => {
            ui.label(tr!(
                lang,
                "Not injected: {}",
                "주입하지 않음: {}",
                skip_label(*reason, lang)
            ));
            return;
        }
        InjectionPlan::Planned { candidates, text } => (candidates, text),
    };
    egui::Grid::new("inject-candidates")
        .striped(true)
        .show(ui, |ui| {
            for head in [
                tr!(lang, "rank", "순위"),
                tr!(lang, "similarity", "유사도"),
                tr!(lang, "verdict", "판정"),
                "prompt".to_string(),
                tr!(lang, "iteration", "반복"),
            ] {
                ui.strong(head);
            }
            ui.end_row();
            for c in candidates {
                ui.label(c.rank.to_string());
                ui.label(
                    c.similarity
                        .map_or_else(|| "-".to_string(), |s| format!("{s:.3}")),
                );
                ui.label(verdict_label(c.verdict, lang));
                ui.monospace(c.prompt_id.as_deref().map(short).unwrap_or_default());
                ui.monospace(&c.iteration_id);
                ui.end_row();
            }
        });
    ui.add_space(4.0);
    match text {
        Some(text) => {
            ui.strong(tr!(lang, "Injected text", "주입 텍스트"));
            ui.label(RichText::new(text).monospace());
        }
        None => {
            ui.weak(tr!(lang, "Nothing is injected.", "주입할 내용이 없습니다."));
        }
    }
}

/// 반복 하나. 결과 원문은 처음에 접어 둔다. (FR-GUI-13)
fn iteration_view(ui: &mut egui::Ui, it: &IterationView, lang: Lang) {
    let (input_label, response_label, result_label) = match lang {
        Lang::En => ("[input]", "[response]", "result"),
        Lang::Ko => ("[입력]", "[응답]", "결과"),
    };
    ui.label(
        RichText::new(format!(
            "[{}] {}  {}",
            it.iteration_index,
            ts_label(&it.ts),
            short(&it.session_id)
        ))
        .strong(),
    );
    for input in &it.inputs {
        ui.label(format!("{input_label} {input}"));
    }
    for (n, tool) in it.tools.iter().enumerate() {
        let error = if tool.is_error { " (error)" } else { "" };
        let input: String = tool.input.chars().take(TOOL_INPUT_CHARS).collect();
        let cut = if input.len() < tool.input.len() {
            "…"
        } else {
            ""
        };
        ui.label(RichText::new(format!("{}({input}{cut}){error}", tool.name)).monospace());
        if !cut.is_empty() {
            egui::CollapsingHeader::new(tr!(lang, "full input", "입력 전체"))
                .id_salt((&it.id, n, "input"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(RichText::new(&tool.input).monospace());
                });
        }
        if let Some(result) = &tool.result {
            egui::CollapsingHeader::new(result_label)
                .id_salt((&it.id, n, "result"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(RichText::new(result).monospace());
                });
        }
        for done in &tool.completions {
            ui.label(format!(
                "bg #{} {}: {}",
                done.seq, done.status, done.summary
            ));
            if let Some(result) = &done.result {
                egui::CollapsingHeader::new(result_label)
                    .id_salt((&it.id, n, done.seq))
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new(result).monospace());
                    });
            }
        }
    }
    if !it.response.is_empty() {
        ui.label(format!("{response_label} {}", it.response));
    }
    ui.separator();
}

fn kind_name(kind: SearchKind) -> &'static str {
    match kind {
        SearchKind::Iteration => "iteration",
        SearchKind::Result => "result",
        SearchKind::Both => "both",
    }
}

fn project_table(ui: &mut egui::Ui, st: &StatusInfo, lang: Lang) {
    if st.projects.is_empty() {
        ui.weak(tr!(lang, "No project yet.", "아직 프로젝트가 없습니다."));
        return;
    }
    let heads = [
        "id".to_string(),
        tr!(lang, "location", "위치"),
        tr!(lang, "iterations", "반복"),
        tr!(lang, "results", "결과"),
        tr!(lang, "pending embeddings", "임베딩 대기"),
        tr!(lang, "vectors", "벡터"),
        tr!(lang, "compatibility", "호환성"),
    ];
    TableBuilder::new(ui)
        .id_salt("project-table")
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(80.0))
        .column(Column::remainder().at_least(200.0).clip(true))
        .columns(Column::auto().at_least(70.0), 2)
        .column(Column::auto().at_least(150.0))
        .columns(Column::auto().at_least(70.0), 2)
        .header(ROW_HEIGHT, |mut header| {
            for head in &heads {
                header.col(|ui| {
                    ui.strong(head);
                });
            }
        })
        .body(|mut body| {
            for p in &st.projects {
                body.row(ROW_HEIGHT, |mut row| {
                    row.col(|ui| {
                        mono_cell(ui, &p.id.chars().take(SESSION_ID_SHORT).collect::<String>())
                    });
                    row.col(|ui| {
                        if p.exists {
                            cell(ui, &p.location);
                        } else {
                            cell(ui, &format!("{}{}", p.location, missing_location(lang)));
                        }
                    });
                    row.col(|ui| cell(ui, &p.iterations.to_string()));
                    row.col(|ui| cell(ui, &p.results.to_string()));
                    row.col(|ui| {
                        cell(
                            ui,
                            &format!(
                                "{}{}",
                                p.embed_jobs,
                                embed_progress(p.embed_jobs, p.embed_per_min, lang)
                            ),
                        )
                    });
                    row.col(|ui| cell(ui, &p.vectors.to_string()));
                    row.col(|ui| cell(ui, &p.compatibility));
                });
            }
        });
}

fn candidate_table(ui: &mut egui::Ui, form: &mut ImportForm, lang: Lang) {
    let heads = [
        String::new(),
        tr!(lang, "started", "시작 시각"),
        "session".to_string(),
        tr!(lang, "status", "상태"),
        tr!(lang, "size", "크기"),
        tr!(lang, "start path", "시작 경로"),
        tr!(lang, "first prompt", "첫 프롬프트"),
    ];
    TableBuilder::new(ui)
        .id_salt("candidate-table")
        .striped(true)
        .resizable(true)
        .auto_shrink([false, false])
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::auto().at_least(150.0))
        .column(Column::auto().at_least(80.0))
        .column(Column::auto().at_least(90.0))
        .column(Column::auto().at_least(70.0))
        .column(Column::initial(240.0).at_least(80.0).clip(true))
        .column(Column::remainder().at_least(160.0).clip(true))
        .header(ROW_HEIGHT, |mut header| {
            for head in &heads {
                header.col(|ui| {
                    ui.strong(head);
                });
            }
        })
        .body(|mut body| {
            for (c, on) in form.candidates.iter().zip(form.selected.iter_mut()) {
                body.row(ROW_HEIGHT, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(on, "");
                    });
                    row.col(|ui| cell(ui, &started_label(&c.started_at)));
                    row.col(|ui| {
                        mono_cell(
                            ui,
                            &c.session_id
                                .chars()
                                .take(SESSION_ID_SHORT)
                                .collect::<String>(),
                        )
                    });
                    row.col(|ui| {
                        let color = match c.status {
                            ImportStatus::New => OK_COLOR,
                            ImportStatus::LiveOnly => WARN_COLOR,
                            ImportStatus::Imported | ImportStatus::OtherProject => {
                                ui.visuals().weak_text_color()
                            }
                        };
                        ui.colored_label(color, c.status.label(lang));
                    });
                    row.col(|ui| cell(ui, &size_label(c.size)));
                    row.col(|ui| cell(ui, &c.start_cwd));
                    row.col(|ui| cell(ui, &c.first_prompt));
                });
            }
        });
}

/// 설정 한 줄: 키와 설명, 편집기, 기본값 표시나 되돌리기 버튼.
fn setting_row(
    ui: &mut egui::Ui,
    field: &mut Field,
    default: Option<&Field>,
    modified: bool,
    lang: Lang,
) {
    ui.vertical(|ui| {
        ui.set_width(SETTING_KEY_WIDTH);
        let key = RichText::new(field.key).strong();
        ui.label(if modified {
            key.color(CHANGED_COLOR)
        } else {
            key
        });
        ui.add(
            egui::Label::new(
                RichText::new(describe(field.section, field.key, lang))
                    .small()
                    .weak(),
            )
            .wrap(),
        );
    });
    field_editor(ui, field, lang);
    match default {
        Some(d) if d.value == field.value => {
            ui.weak(tr!(lang, "default", "기본값"));
        }
        Some(d) => {
            if ui
                .small_button(tr!(lang, "↺ default", "↺ 기본값으로"))
                .on_hover_text(tr!(
                    lang,
                    "Set back to the default value",
                    "기본값으로 되돌립니다"
                ))
                .clicked()
            {
                field.value = d.value.clone();
            }
        }
        None => {
            ui.label("");
        }
    }
}

fn field_editor(ui: &mut egui::Ui, field: &mut Field, lang: Lang) {
    let is_language = (field.section, field.key) == ("ui", "language");
    let is_multiline = (field.section, field.key) == ("embedder", "query_prefix");
    match &mut field.value {
        FieldValue::Bool(b) => {
            ui.checkbox(b, "");
        }
        FieldValue::Int(n) => {
            ui.add_sized(
                [NUMBER_WIDTH, FIELD_HEIGHT],
                egui::DragValue::new(n).range(0..=i64::MAX),
            );
        }
        FieldValue::Float(x) => {
            ui.add_sized(
                [NUMBER_WIDTH, FIELD_HEIGHT],
                egui::DragValue::new(x).speed(0.01).range(0.0..=f64::MAX),
            );
        }
        FieldValue::Text(s) if is_language => {
            egui::ComboBox::from_id_salt("language")
                .selected_text(s.clone())
                .width(NUMBER_WIDTH)
                .show_ui(ui, |ui| {
                    for code in ["en", "ko"] {
                        ui.selectable_value(s, code.to_string(), code);
                    }
                });
        }
        FieldValue::Text(s) if is_multiline => {
            ui.add_sized(
                [TEXT_FIELD_WIDTH, FIELD_HEIGHT * 2.0],
                egui::TextEdit::multiline(s).desired_rows(2),
            );
        }
        FieldValue::Text(s) => {
            ui.add_sized(
                [TEXT_FIELD_WIDTH, FIELD_HEIGHT],
                egui::TextEdit::singleline(s),
            );
        }
        FieldValue::List(items) => {
            ui.vertical(|ui| {
                let mut remove = None;
                for (n, item) in items.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [TEXT_FIELD_WIDTH - LIST_BUTTON_WIDTH, FIELD_HEIGHT],
                            egui::TextEdit::singleline(item),
                        );
                        if ui.small_button("✕").clicked() {
                            remove = Some(n);
                        }
                    });
                }
                if let Some(n) = remove {
                    items.remove(n);
                }
                if ui.small_button(tr!(lang, "+ add", "+ 추가")).clicked() {
                    items.push(String::new());
                }
            });
        }
        FieldValue::Patterns(patterns) => {
            ui.vertical(|ui| {
                let mut remove = None;
                for (n, p) in patterns.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [PATTERN_KIND_WIDTH, FIELD_HEIGHT],
                            egui::TextEdit::singleline(&mut p.kind).hint_text("kind"),
                        );
                        ui.add_sized(
                            [
                                TEXT_FIELD_WIDTH - PATTERN_KIND_WIDTH - LIST_BUTTON_WIDTH,
                                FIELD_HEIGHT,
                            ],
                            egui::TextEdit::singleline(&mut p.pattern).hint_text("pattern (regex)"),
                        );
                        if ui.small_button("✕").clicked() {
                            remove = Some(n);
                        }
                    });
                }
                if let Some(n) = remove {
                    patterns.remove(n);
                }
                if ui.small_button(tr!(lang, "+ add", "+ 추가")).clicked() {
                    patterns.push(PatternEntry {
                        kind: String::new(),
                        pattern: String::new(),
                    });
                }
            });
        }
    }
}

fn install_scope(form: &InstallForm, cwd: &str) -> Result<Scope, String> {
    if form.user_scope {
        return Ok(Scope::User);
    }
    let project = PathBuf::from(cwd).join(form.project.trim());
    project_scope(Some(project), &PathBuf::from(cwd))
}

/// `hippo install`/`uninstall`과 같은 동작. 복사할 실행 파일은 실행 중인 hippo이다. (FR-GUI-10)
fn install_text(
    paths: &Paths,
    scope: Result<Scope, String>,
    action: InstallAction,
    dry_run: bool,
    lang: Lang,
) -> Result<String, String> {
    let scope = scope?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut out = String::new();
    match run_install(
        paths,
        &scope,
        action == InstallAction::Install,
        dry_run,
        &exe,
        lang,
        &mut out,
    ) {
        Ok(()) => Ok(out),
        Err(e) => Err(format!("{out}{e}")),
    }
}
