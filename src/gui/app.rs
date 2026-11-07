//! GUI 화면. 데몬 요청과 파일 작업은 작업 스레드에서 하고, 화면은 2초마다 갱신한다. (FR-GUI-1~12)

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::client::request;
use crate::commands::render_delete_report;
use crate::commands::{
    InstallTarget, SESSION_ID_SHORT, purge_data, query, query_until_reply, run_install,
    run_uninstall, scope_arg, stop_services,
};
use crate::config::PatternEntry;
use crate::config::{Config, Paths, StartTab, Theme};
use crate::config_edit::{
    CAPTURE_ENABLED, Field, FieldValue, INJECT_ENABLED, SaveOutcome, changed, fields,
    needs_reindex, needs_restart, save,
};
use crate::gui::images;
use crate::gui::picker::{pick_folder, start_folder};
use crate::gui::state::{
    DeleteTarget, ImportForm, ImportProgress, InstallAction, InstallForm, PollGate, PreviewKey,
    RecordsView, Reload, Tab, Toast, Toasts, default_project, embed_state, initial_tab, page_count,
    preview_scope_ok, purge_may_start, queue_summary, read_last_tab, resolve_import_args,
    run_import, should_poll, skip_label, verdict_label, write_last_tab,
};
use crate::gui::text::describe;
use crate::gui::theme;
use crate::gui::widgets::{
    FIELD_HEIGHT, apply_style, banner, button, card, cell, form, mono_cell, output, primary, stat,
    text_field,
};
use crate::i18n::{Lang, local_time};
use crate::import::{Candidate, ImportStatus, size_label, started_label};
use crate::install::{installed_exe, is_installed, settings_file};
use crate::layout::{
    InstallState, LinkState, default_claude_dir, install_state, link_state, path_link,
};
use crate::protocol::{
    DeleteReport, Request, Response, Scope as QueryScope, StatusInfo, embed_progress, inject_ratio,
    missing_location,
};
use crate::registry::project_folder;
use crate::search::{HitView, InjectionPlan, IterationView, SearchKind};
use crate::setup::home_dir;
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
const DEFAULT_LIMIT: usize = 10;
const BROWSE_WIDTH: f32 = 96.0;
const PROJECT_MENU_WIDTH: f32 = 110.0;
const RECORDS_SEARCH_LIMIT: usize = 20;
const QUERY_HEIGHT: f32 = 220.0;
const TS_CHARS: usize = 19;
const TOOL_INPUT_CHARS: usize = 200;
const README_EN: &str = include_str!("../../README.md");
const README_KO: &str = include_str!("../../README.ko.md");
const LICENSE: &str = include_str!("../../LICENSE");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const HEADER_ICON_SIZE: egui::Vec2 = egui::vec2(24.0, 24.0);
const LOGO_SIZE: egui::Vec2 = egui::vec2(144.0, 48.0);
const CHARACTER_SIZE: egui::Vec2 = egui::vec2(94.0, 120.0);

/// 창을 열고 닫힐 때까지 돌아온다. (IF-CLI-10)
pub fn run(paths: Paths, cwd: String) -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("hippo")
        .with_inner_size(WINDOW_SIZE)
        .with_min_inner_size(MIN_WINDOW_SIZE);
    if let Some(icon) = images::app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "hippo",
        options,
        Box::new(move |cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(App::new(paths, cwd, &cc.egui_ctx)))
        }),
    )
    .map_err(|e| e.to_string())
}

/// 화면에 쓰는 이미지. 해석하지 못한 이미지는 `None`이고 그 자리를 비운다. (FR-GUI-20)
struct Art {
    icon: Option<egui::TextureHandle>,
    logo: Option<egui::TextureHandle>,
    logo_dark: Option<egui::TextureHandle>,
    character: Option<egui::TextureHandle>,
}

impl Art {
    fn load(ctx: &egui::Context) -> Self {
        let load = |name: &str, bytes: &[u8]| {
            images::decode_png(bytes)
                .map(|image| ctx.load_texture(name, image, egui::TextureOptions::LINEAR))
        };
        Self {
            icon: load("icon", images::ICON_SMALL),
            logo: load("logo", images::LOGO),
            logo_dark: load("logo-dark", images::LOGO_DARK),
            character: load("character", images::CHARACTER),
        }
    }
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

impl Tab {
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
    /// 주입 미리 보기 모드. (FR-GUI-9)
    preview_mode: bool,
    /// 주입 미리 보기 결과와 걸린 시간. 있으면 `output` 대신 보여 준다. (FR-GUI-9)
    preview: Option<(InjectionPlan, u64)>,
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
    toasts: Toasts,
    /// 실행 경로의 프로젝트. 아직 묻지 않았으면 `None`. (FR-GUI-18)
    current: Option<Option<String>>,
    delete_started: Option<Instant>,
    sessions_task: Task<Reply>,
    turns_task: Task<Reply>,
    detail_task: Task<Reply>,
    /// 질의한 프로젝트와 응답.
    query_task: Task<(String, Reply)>,
    preview_task: Task<(DeleteTarget, Reply)>,
    delete_task: Task<(DeleteTarget, Reply)>,
    current_task: Task<Reply>,
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

/// Claude 적용 탭이 보여 주는 현재 상태: hook 적용 여부와 PATH 링크. (FR-GUI-10)
type InstallCheck = (bool, LinkState);

struct Install {
    form: InstallForm,
    output: String,
    current: Option<InstallCheck>,
    checked: Option<(String, String)>,
    confirm: Option<InstallAction>,
    confirm_purge: bool,
    /// 확인한 완전 삭제. 진행 중인 상태 요청이 끝나면 시작한다. (FR-GUI-10)
    purge_pending: bool,
    /// 진행 중인 작업이 완전 삭제인지. 끝나면 GUI를 멈춤 상태로 둔다.
    purging: bool,
    check: Task<InstallCheck>,
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
        if let Some((project, reply)) = self.query_task.poll()
            && self.view.project.as_deref() == Some(project.as_str())
        {
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
        if let Some(reply) = self.current_task.poll() {
            self.current = Some(match reply {
                Ok(Response::CurrentProject { id }) => id,
                _ => None,
            });
        }
        if let Some((target, reply)) = self.delete_task.poll() {
            self.delete_started = None;
            match reply {
                Ok(Response::Delete { .. }) => {
                    self.view.after_delete(&target);
                    self.toasts.push(Toast::success(
                        tr!(lang, "Deleted.", "지웠습니다."),
                        Instant::now(),
                    ));
                    return matches!(target, DeleteTarget::Project(_));
                }
                other => self
                    .toasts
                    .push(Toast::error(error_of(other), Instant::now())),
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
        let req = target.request(cwd, true);
        let paths = paths.clone();
        self.preview_task
            .start(ctx, move || (target, query(&paths, &req, QUERY_TIMEOUT)));
    }
}

/// 찾아보기로 채울 경로 입력란. (FR-GUI-24)
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathField {
    ImportFrom,
    ImportTo,
    SearchScope,
    ShowScope,
    ReindexScope,
    DataRoot,
    ClaudeDir,
    TranscriptsDir,
}

type PickResult = (PathField, Option<PathBuf>);

/// 찾아보기 버튼. 선택 창은 작업 스레드에서 띄운다. 한 번에 하나만 연다. (FR-GUI-24)
struct Browse<'a> {
    picker: &'a mut Task<PickResult>,
    ctx: &'a egui::Context,
    cwd: &'a str,
    home: &'a Path,
    lang: Lang,
}

impl Browse<'_> {
    fn button(&mut self, ui: &mut egui::Ui, field: PathField, current: &str) {
        let label = tr!(self.lang, "Browse…", "찾아보기…");
        if ui
            .add_enabled(
                !self.picker.running,
                button(label).min_size(egui::vec2(BROWSE_WIDTH, FIELD_HEIGHT)),
            )
            .clicked()
        {
            let start = start_folder(current, Path::new(self.cwd), self.home);
            let prompt = tr!(self.lang, "Choose a folder", "폴더를 고르십시오");
            self.picker
                .start(self.ctx, move || (field, pick_folder(&prompt, &start)));
        }
    }
}

/// 경로 입력란과 찾아보기 버튼. (FR-GUI-24)
fn path_input(
    ui: &mut egui::Ui,
    width: f32,
    text: &mut String,
    hint: &str,
    field: PathField,
    browse: &mut Browse,
) {
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        text_field(
            ui,
            (width - BROWSE_WIDTH - spacing).max(BROWSE_WIDTH),
            text,
            hint,
        );
        browse.button(ui, field, text);
    });
}

/// 저장된 프로젝트를 골라 그 폴더 경로를 입력란에 넣는 목록. (FR-GUI-7)
fn project_menu(
    ui: &mut egui::Ui,
    salt: &str,
    projects: &[(String, String)],
    text: &mut String,
    lang: Lang,
) {
    ui.add_enabled_ui(!projects.is_empty(), |ui| {
        egui::ComboBox::from_id_salt(salt)
            .width(PROJECT_MENU_WIDTH)
            .selected_text(tr!(lang, "Project", "프로젝트"))
            .show_ui(ui, |ui| {
                ui.set_min_width(TEXT_FIELD_WIDTH);
                for (id, location) in projects {
                    if ui
                        .selectable_label(false, project_label(projects, id))
                        .clicked()
                    {
                        *text = project_folder(location).to_string_lossy().into_owned();
                    }
                }
            });
    });
}

pub struct App {
    paths: Paths,
    cwd: String,
    exe: PathBuf,
    home: PathBuf,
    /// 데이터 루트의 설치 상태. 계산 중이면 `None`. (FR-GUI-23)
    state: Option<InstallState>,
    state_task: Task<(PathBuf, InstallState)>,
    /// 수동 중지 뒤 폴링을 멈춘 상태. 폴링이 데몬을 다시 띄우지 않게 한다. (FR-GUI-3)
    paused: bool,
    stop_task: Task<Result<String, String>>,
    stop_output: String,
    /// (주입 동작, 저장 동작). (FR-GUI-3, FR-CAP-8, FR-RET-12)
    switches: (bool, bool),
    switch_task: Task<Result<(bool, bool), String>>,
    switch_error: Option<String>,
    /// 찾아보기 선택 창. (FR-GUI-24)
    picker: Task<PickResult>,
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
    art: Art,
    readme_en: String,
    readme_ko: String,
    markdown: egui_commonmark::CommonMarkCache,
    /// 마지막으로 기록한 탭. 보이는 탭과 다르면 다시 기록한다. (FR-GUI-25)
    recorded_tab: Tab,
}

impl App {
    fn new(paths: Paths, cwd: String, ctx: &egui::Context) -> Self {
        let ui_config = Config::load(&paths.config())
            .map(|c| c.ui)
            .unwrap_or_default();
        let lang = ui_config.language;
        let exe = std::env::current_exe().unwrap_or_default();
        let home = home_dir().unwrap_or_default();
        let claude_dir = default_claude_dir(&paths.root, &home);
        ctx.set_visuals_of(egui::Theme::Light, theme::visuals(false));
        ctx.set_visuals_of(egui::Theme::Dark, theme::visuals(true));
        ctx.set_theme(theme::preference(ui_config.theme));
        let tab = initial_tab(ui_config.start_tab, read_last_tab(&paths.gui_last_tab()));
        Self {
            lang,
            tab,
            recorded_tab: tab,
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
                preview_mode: false,
                preview: None,
                task: Task::default(),
            },
            records: Records::default(),
            projects: Projects {
                reindex_scope: "all".into(),
                ..Projects::default()
            },
            install: Install {
                form: InstallForm::new(&paths.root, &claude_dir),
                output: String::new(),
                current: None,
                checked: None,
                confirm: None,
                confirm_purge: false,
                purge_pending: false,
                purging: false,
                check: Task::default(),
                task: Task::default(),
            },
            exe,
            home,
            state: None,
            state_task: Task::default(),
            paused: false,
            stop_task: Task::default(),
            stop_output: String::new(),
            switches: load_switches(&paths),
            switch_task: Task::default(),
            switch_error: None,
            picker: Task::default(),
            about_license: false,
            art: Art::load(ctx),
            readme_en: images::readme_markdown(README_EN),
            readme_ko: images::readme_markdown(README_KO),
            markdown: egui_commonmark::CommonMarkCache::default(),
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
        let polling = should_poll(PollGate {
            installed: self.state == Some(InstallState::Installed),
            paused: self.paused,
            stopping: self.stop_task.running,
            install_busy: self.install.task.running || self.install.purge_pending,
        });
        if polling && Instant::now() >= self.next_poll && !self.status_task.running {
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

    /// 멈추기와 다시 시작. 멈춘 동안에는 폴링하지 않는다. (FR-GUI-3, FR-DMN-12)
    fn stop_controls(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        if let Some(result) = self.stop_task.poll() {
            self.paused = true;
            self.status = None;
            self.stop_output = result.unwrap_or_else(|e| tr!(lang, "error: {e}\n", "오류: {e}\n"));
        }
        ui.horizontal(|ui| {
            if self.paused {
                ui.colored_label(warn_color(ui), tr!(lang, "● stopped", "● 멈춤"));
                if primary(ui, true, tr!(lang, "Start again", "다시 시작")).clicked() {
                    self.paused = false;
                    self.stop_output.clear();
                    self.next_poll = Instant::now();
                }
            } else {
                let label = tr!(lang, "Stop daemon and Ollama", "데몬과 Ollama 멈추기");
                if ui
                    .add_enabled(!self.stop_task.running, button(label))
                    .clicked()
                {
                    let root = self.paths.root.clone();
                    self.stop_task.start(&ctx, move || {
                        let mut out = String::new();
                        stop_services(&root, lang, &mut out).map(|()| out)
                    });
                }
                if self.stop_task.running {
                    ui.spinner();
                }
            }
        });
        if !self.stop_output.is_empty() {
            ui.weak(self.stop_output.trim_end());
        }
        ui.add_space(4.0);
        self.pause_controls(ui);
    }

    /// 주입과 저장의 일시정지/재생. 그 키만 `config.toml`에 저장한다. (FR-GUI-3)
    fn pause_controls(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        if let Some(result) = self.switch_task.poll() {
            match result {
                Ok(switches) => {
                    self.switches = switches;
                    self.switch_error = None;
                }
                Err(e) => self.switch_error = Some(e),
            }
        }
        let rows = [
            (
                INJECT_ENABLED,
                tr!(lang, "Injection", "주입"),
                self.switches.0,
            ),
            (
                CAPTURE_ENABLED,
                tr!(lang, "Saving", "저장"),
                self.switches.1,
            ),
        ];
        for ((section, key), name, on) in rows {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).strong());
                if on {
                    ui.colored_label(ok_color(ui), tr!(lang, "● running", "● 동작 중"));
                } else {
                    ui.colored_label(warn_color(ui), tr!(lang, "● paused", "● 일시정지"));
                }
                let label = if on {
                    tr!(lang, "Pause", "일시정지")
                } else {
                    tr!(lang, "Resume", "재생")
                };
                if ui
                    .add_enabled(!self.switch_task.running, button(label))
                    .clicked()
                {
                    let paths = self.paths.clone();
                    let field = Field {
                        section,
                        key,
                        value: FieldValue::Bool(!on),
                    };
                    self.switch_task.start(&ctx, move || {
                        save(&paths.config(), &[field]).map_err(|e| e.to_string())?;
                        Ok(load_switches(&paths))
                    });
                }
            });
        }
        if let Some(e) = &self.switch_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.add_space(4.0);
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        self.stop_controls(ui);
        if self.paused {
            ui.weak(tr!(
                lang,
                "Status polling is paused. Press Start again to resume; it starts the daemon.",
                "상태 폴링을 멈췄습니다. 다시 시작을 누르면 폴링을 다시 하며, 데몬도 다시 뜹니다."
            ));
            return;
        }
        let st = match &self.status {
            None => {
                ui.weak(tr!(lang, "Waiting for the daemon…", "데몬을 기다리는 중…"));
                return;
            }
            Some(Err(e)) => {
                empty_state(ui, self.art.character.as_ref(), |ui| {
                    banner(ui, ui.visuals().error_fg_color, |ui| {
                        ui.label(tr!(lang, "Not connected: {e}", "연결 안 됨: {e}"));
                    });
                });
                return;
            }
            Some(Ok(st)) => st,
        };
        ui.horizontal(|ui| {
            stat(
                ui,
                &tr!(lang, "Daemon", "데몬"),
                RichText::new(tr!(lang, "running", "실행 중")).color(ok_color(ui)),
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
        queue_card(ui, st, lang);
        ui.add_space(4.0);
        card(ui, &tr!(lang, "Projects", "프로젝트"), |ui| {
            project_table(ui, st, lang, self.art.character.as_ref());
        });
        if !st.projects.is_empty() {
            ui.add_space(4.0);
            card(
                ui,
                &tr!(lang, "Injection (last 7 days)", "주입 (최근 7일)"),
                |ui| inject_table(ui, st, lang),
            );
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let s = &mut self.settings;
        let mut browse = Browse {
            picker: &mut self.picker,
            ctx: &ctx,
            cwd: &self.cwd,
            home: &self.home,
            lang,
        };
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
                    if let Some(FieldValue::Text(value)) = changes
                        .iter()
                        .find(|f| (f.section, f.key) == ("ui", "theme"))
                        .map(|f| &f.value)
                        && let Some(theme) = Theme::ALL.into_iter().find(|t| t.as_str() == value)
                    {
                        ctx.set_theme(theme::preference(theme));
                    }
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
                    .color(changed_color(ui)),
                );
            }
            if let Some(m) = &s.message {
                ui.separator();
                ui.label(m);
            }
        });
        if s.restart || s.reindex {
            banner(ui, warn_color(ui), |ui| {
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
                                    setting_row(ui, field, default, modified, lang, &mut browse);
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
                let mut browse = Browse {
                    picker: &mut self.picker,
                    ctx: &ctx,
                    cwd: &self.cwd,
                    home: &self.home,
                    lang,
                };
                let projects: Vec<(String, String)> = match &self.status {
                    Some(Ok(st)) => st
                        .projects
                        .iter()
                        .map(|p| (p.id.clone(), p.location.clone()))
                        .collect(),
                    _ => Vec::new(),
                };
                let import = &mut self.import;
                form(ui, "import-args", |ui, w| {
                    let w = w - PROJECT_MENU_WIDTH - ui.spacing().item_spacing.x;
                    ui.label(tr!(lang, "From", "원본"));
                    ui.add_enabled_ui(!running, |ui| {
                        ui.horizontal(|ui| {
                            path_input(
                                ui,
                                w,
                                &mut import.from,
                                &hint,
                                PathField::ImportFrom,
                                &mut browse,
                            );
                            project_menu(
                                ui,
                                "import-from-project",
                                &projects,
                                &mut import.from,
                                lang,
                            );
                        })
                    });
                    ui.end_row();
                    ui.label(tr!(lang, "To", "대상"));
                    ui.add_enabled_ui(!running, |ui| {
                        ui.horizontal(|ui| {
                            path_input(ui, w, &mut import.to, "", PathField::ImportTo, &mut browse);
                            project_menu(ui, "import-to-project", &projects, &mut import.to, lang);
                        })
                    });
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
                            warn_color(ui),
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
            // 실제 가져오기 요청은 끝날 때까지 기다린다. (FR-GUI-7)
            let mut send = |req: &Request| query_until_reply(&paths, req);
            run_import(&mut send, &from, &to, &selected, &stop, &progress, lang);
        });
    }

    fn search_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let s = &mut self.search;
        let mut browse = Browse {
            picker: &mut self.picker,
            ctx: &ctx,
            cwd: &self.cwd,
            home: &self.home,
            lang,
        };
        if let Some(reply) = s.task.poll() {
            s.hits = None;
            s.preview = None;
            s.output.clear();
            match reply {
                Ok(Response::Hits { hits, notes }) => s.hits = Some((hits, notes)),
                Ok(Response::InjectPreview { plan, elapsed_ms }) => {
                    s.preview = Some((plan, elapsed_ms));
                }
                other => s.output = text_of(other),
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
                    path_input(
                        ui,
                        w,
                        &mut s.scope,
                        "project | all | <path>",
                        PathField::SearchScope,
                        &mut browse,
                    );
                    ui.end_row();
                    ui.label(tr!(lang, "Kind / limit", "종류 / 개수"));
                    ui.add_enabled_ui(!s.preview_mode, |ui| {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("kind")
                                .selected_text(kind_name(s.kind))
                                .show_ui(ui, |ui| {
                                    for kind in [
                                        SearchKind::Iteration,
                                        SearchKind::Result,
                                        SearchKind::Both,
                                    ] {
                                        ui.selectable_value(&mut s.kind, kind, kind_name(kind));
                                    }
                                });
                            ui.add(egui::DragValue::new(&mut s.limit).range(1..=100));
                        });
                    });
                    ui.end_row();
                });
                ui.add_space(4.0);
                let scope_ok = !s.preview_mode || preview_scope_ok(&s.scope);
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.preview_mode, false, tr!(lang, "Search", "검색"));
                    ui.radio_value(
                        &mut s.preview_mode,
                        true,
                        tr!(lang, "Injection preview", "주입 미리 보기"),
                    );
                    if primary(
                        ui,
                        !s.task.running && !s.query.trim().is_empty() && scope_ok,
                        tr!(lang, "Run", "실행"),
                    )
                    .clicked()
                    {
                        search_clicked = true;
                    }
                });
                if !scope_ok {
                    ui.colored_label(
                        warn_color(ui),
                        tr!(
                            lang,
                            "Injection works inside one project. Set the scope to project or a path.",
                            "주입은 프로젝트 하나 안에서만 합니다. 범위를 project나 경로로 두십시오."
                        ),
                    );
                }
            });
            card(&mut cols[1], &tr!(lang, "Show a turn", "턴 보기"), |ui| {
                let hint = tr!(lang, "empty | project | <path>", "비움 | project | <경로>");
                form(ui, "show-args", |ui, w| {
                    ui.label("prompt_id");
                    text_field(ui, w, &mut s.prompt_id, "");
                    ui.end_row();
                    ui.label(tr!(lang, "Scope", "범위"));
                    path_input(
                        ui,
                        w,
                        &mut s.show_scope,
                        &hint,
                        PathField::ShowScope,
                        &mut browse,
                    );
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
        if search_clicked && s.preview_mode {
            if !s.task.running && !s.query.trim().is_empty() && preview_scope_ok(&s.scope) {
                let req = Request::InjectPreview {
                    scope: scope_arg(&self.cwd, s.scope.trim()),
                    cwd: self.cwd.clone(),
                    prompt: s.query.clone(),
                };
                let paths = self.paths.clone();
                s.task
                    .start(&ctx, move || query(&paths, &req, QUERY_TIMEOUT));
            }
        } else if search_clicked && !s.task.running && !s.query.trim().is_empty() {
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
        match (&s.hits, &s.preview) {
            (Some((hits, notes)), _) => {
                egui::ScrollArea::vertical()
                    .id_salt("search-hits")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        open = hit_rows(ui, hits, notes, true, lang);
                    });
            }
            (None, Some((plan, elapsed_ms))) => {
                egui::ScrollArea::vertical()
                    .id_salt("search-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| preview_view(ui, plan, *elapsed_ms, lang));
            }
            (None, None) => output(
                ui,
                "search-output",
                &s.output,
                &tr!(lang, "Results appear here.", "결과가 여기에 표시됩니다."),
            ),
        }
        if let Some(hit) = open
            && let Some(prompt_id) = &hit.prompt_id
        {
            let before = self.records.view.project.clone();
            self.records.view.open_turn(&hit.project_id, prompt_id);
            if before != self.records.view.project {
                self.records.query_out = None;
            }
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
        if r.current.is_none() {
            let (paths, req) = (paths.clone(), Request::CurrentProject { cwd: cwd.clone() });
            r.current_task
                .start(&ctx, move || query(&paths, &req, QUERY_TIMEOUT));
        }
        if let Some(current) = &r.current
            && r.view
                .project
                .as_ref()
                .is_none_or(|p| !projects.iter().any(|(id, _)| id == p))
        {
            let ids: Vec<String> = projects.iter().map(|(id, _)| id.clone()).collect();
            if let Some(id) = default_project(&ids, current.as_deref())
                && r.view.select_project(&id)
            {
                r.query_out = None;
            }
        }
        let deleting = r.delete_task.running;
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
                            && r.view.select_project(id)
                        {
                            r.query_out = None;
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
                    .add_enabled(
                        !deleting,
                        button(tr!(lang, "Delete project", "프로젝트 삭제")),
                    )
                    .clicked()
            {
                delete = Some(DeleteTarget::Project(id.clone()));
            }
            if let Some(started) = r.delete_started {
                ui.separator();
                ui.spinner();
                let secs = started.elapsed().as_secs();
                ui.label(tr!(
                    lang,
                    "Deleting… {secs}s (waiting for the embedding job in progress)",
                    "지우는 중… {secs}초 (처리 중인 임베딩 작업이 끝나기를 기다립니다)"
                ));
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            let now = Instant::now();
            r.toasts.expire(now);
            let mut close = None;
            for (i, toast) in r.toasts.items().iter().enumerate() {
                ui.separator();
                if toast.is_error() {
                    ui.colored_label(ui.visuals().error_fg_color, &toast.text);
                } else {
                    ui.label(&toast.text);
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
                if ui.small_button("×").clicked() {
                    close = Some(i);
                }
            }
            if let Some(i) = close {
                r.toasts.close(i);
            }
        });
        if projects.is_empty() {
            empty_state(ui, self.art.character.as_ref(), |ui| {
                ui.weak(tr!(lang, "No project yet.", "아직 프로젝트가 없습니다."));
            });
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
                                scope: QueryScope::Id(id.clone()),
                                cwd: cwd.clone(),
                                prompt: r.query.clone(),
                            }
                        } else {
                            Request::Search {
                                query: r.query.clone(),
                                kind: SearchKind::Iteration,
                                limit: RECORDS_SEARCH_LIMIT,
                                scope: QueryScope::Id(id.clone()),
                                cwd: cwd.clone(),
                                structured: true,
                            }
                        };
                        let paths = paths.clone();
                        r.query_task
                            .start(&ctx, move || (id, query(&paths, &req, QUERY_TIMEOUT)));
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
                    && ui
                        .add_enabled(
                            !deleting,
                            egui::Button::new(tr!(lang, "Delete", "삭제")).small(),
                        )
                        .clicked()
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
                    && ui
                        .add_enabled(
                            !deleting,
                            egui::Button::new(tr!(lang, "Delete", "삭제")).small(),
                        )
                        .clicked()
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
                    && ui
                        .add_enabled(
                            !deleting,
                            egui::Button::new(tr!(lang, "Delete", "삭제")).small(),
                        )
                        .clicked()
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
                            // 열 레이아웃의 가로 정렬을 물려받으면 Label이 양쪽 정렬되어 글자 간격이 벌어진다.
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                if iterations.is_empty() {
                                    ui.weak(tr!(lang, "No records.", "기록 없음"));
                                }
                                for it in iterations {
                                    iteration_view(ui, it, lang);
                                }
                            });
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
                r.delete_started = Some(Instant::now());
                r.delete_task
                    .start(&ctx, move || (target, query_until_reply(&paths, &req)));
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
        let mut browse = Browse {
            picker: &mut self.picker,
            ctx: &ctx,
            cwd: &self.cwd,
            home: &self.home,
            lang,
        };
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
                path_input(
                    ui,
                    w,
                    &mut p.reindex_scope,
                    "all | project | <path>",
                    PathField::ReindexScope,
                    &mut browse,
                );
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

    /// 찾아보기로 고른 폴더를 그 입력란에 넣는다. 취소했으면 그대로 둔다. (FR-GUI-24)
    fn apply_pick(&mut self) {
        let Some((field, Some(path))) = self.picker.poll() else {
            return;
        };
        let text = path.display().to_string();
        match field {
            PathField::ImportFrom => self.import.from = text,
            PathField::ImportTo => self.import.to = text,
            PathField::SearchScope => self.search.scope = text,
            PathField::ShowScope => self.search.show_scope = text,
            PathField::ReindexScope => self.projects.reindex_scope = text,
            PathField::DataRoot => self.install.form.root = text,
            PathField::ClaudeDir => self.install.form.claude_dir = text,
            PathField::TranscriptsDir => {
                if let Some(f) = self
                    .settings
                    .edited
                    .iter_mut()
                    .find(|f| (f.section, f.key) == ("import", "transcripts_dir"))
                {
                    f.value = FieldValue::Text(text);
                }
            }
        }
    }

    /// 입력란의 데이터 루트가 바뀌면 요청 대상과 설치 상태를 그 루트로 바꾼다. (FR-GUI-22, FR-GUI-23)
    fn sync_root(&mut self, ctx: &egui::Context) {
        let typed = self.install.form.root.trim();
        if !typed.is_empty() && Path::new(typed) != self.paths.root {
            let old_default = default_claude_dir(&self.paths.root, &self.home);
            let root = PathBuf::from(typed);
            if Path::new(self.install.form.claude_dir.trim()) == old_default {
                self.install.form.claude_dir =
                    default_claude_dir(&root, &self.home).display().to_string();
            }
            self.paths = Paths::new(root);
            self.switches = load_switches(&self.paths);
            self.state = None;
            self.status = None;
            self.records = Records::default();
        }
        if let Some((root, state)) = self.state_task.poll()
            && root == self.paths.root
        {
            self.state = Some(state);
        }
        if self.state.is_none() && !self.state_task.running {
            let (exe, root) = (self.exe.clone(), self.paths.root.clone());
            self.state_task.start(ctx, move || {
                let state = install_state(&exe, &root);
                (root, state)
            });
        }
    }

    fn install_target(&self) -> InstallTarget {
        let form = &self.install.form;
        InstallTarget {
            root: PathBuf::from(form.root.trim()),
            claude_dir: PathBuf::from(form.claude_dir.trim()),
            home: self.home.clone(),
        }
    }

    fn install_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        let target = self.install_target();
        let state = self.state;
        let exe = self.exe.clone();
        let i = &mut self.install;
        let mut browse = Browse {
            picker: &mut self.picker,
            ctx: &ctx,
            cwd: &self.cwd,
            home: &self.home,
            lang,
        };
        if let Some(current) = i.check.poll() {
            i.current = Some(current);
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
                    self.state = None;
                }
                (_, Err(e)) => {
                    i.output = tr!(lang, "error: {e}\n", "오류: {e}\n");
                    i.checked = None;
                    self.state = None;
                }
            }
            if i.purging {
                i.purging = false;
                self.paused = true;
                self.status = None;
            }
        }
        if purge_may_start(i.purge_pending, self.status_task.running) {
            i.purge_pending = false;
            i.purging = true;
            self.paused = true;
            let target = target.clone();
            i.task
                .start(&ctx, move || (None, purge_text(&target, lang)));
        }
        card(ui, &tr!(lang, "Where to apply", "적용 위치"), |ui| {
            form(ui, "install-args", |ui, w| {
                ui.label(tr!(lang, "Data root", "데이터 루트"));
                path_input(
                    ui,
                    w,
                    &mut i.form.root,
                    "~/.hippo",
                    PathField::DataRoot,
                    &mut browse,
                );
                ui.end_row();
                ui.label(tr!(lang, "Claude folder", "Claude 폴더"));
                path_input(
                    ui,
                    w,
                    &mut i.form.claude_dir,
                    "~/.claude",
                    PathField::ClaudeDir,
                    &mut browse,
                );
                ui.end_row();
                ui.label(tr!(lang, "Binary", "실행 파일"));
                match state {
                    Some(InstallState::Installed) => {
                        ui.colored_label(ok_color(ui), tr!(lang, "● installed", "● 설치됨"))
                    }
                    Some(InstallState::NeedUpdate) => ui.colored_label(
                        warn_color(ui),
                        tr!(lang, "● update needed", "● 업데이트 필요"),
                    ),
                    Some(InstallState::NeedInstall) => {
                        ui.weak(tr!(lang, "○ not installed", "○ 설치 필요"))
                    }
                    None => ui.weak(tr!(lang, "checking…", "확인 중…")),
                };
                ui.end_row();
                ui.label(tr!(lang, "Claude hooks", "Claude hook"));
                match i.current {
                    Some((true, _)) => {
                        ui.colored_label(ok_color(ui), tr!(lang, "● applied", "● 적용됨"))
                    }
                    Some((false, _)) => ui.weak(tr!(lang, "○ not applied", "○ 적용 안 됨")),
                    None => ui.weak(tr!(lang, "unknown", "알 수 없음")),
                };
                ui.end_row();
                ui.label(tr!(lang, "PATH link", "PATH 링크"));
                let link = path_link(&target.home).display().to_string();
                match i.current {
                    Some((_, LinkState::Ours)) => {
                        ui.colored_label(ok_color(ui), format!("● {link}"))
                    }
                    Some((_, LinkState::Foreign)) => ui.colored_label(
                        warn_color(ui),
                        tr!(
                            lang,
                            "● {link} points elsewhere",
                            "● {link}이(가) 다른 곳을 가리킴"
                        ),
                    ),
                    Some((_, LinkState::Missing)) => ui.weak(tr!(lang, "○ none", "○ 없음")),
                    None => ui.weak(tr!(lang, "unknown", "알 수 없음")),
                };
                ui.end_row();
            });
        });
        let inputs = (i.form.root.clone(), i.form.claude_dir.clone());
        if i.checked.as_ref() != Some(&inputs) && !i.check.running {
            i.checked = Some(inputs);
            let target = target.clone();
            i.check.start(&ctx, move || {
                let hooks = is_installed(&settings_file(&target.claude_dir));
                let link = link_state(&path_link(&target.home), &installed_exe(&target.root));
                (hooks, link)
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
                        "Copy the binary, add the hippo hooks and the guide block, and link it on PATH",
                        "실행 파일을 복사하고 hippo hook과 안내 블록, PATH 링크를 넣습니다"
                    ),
                ),
                (
                    InstallAction::Uninstall,
                    tr!(lang, "Remove", "해제"),
                    tr!(
                        lang,
                        "Remove only the hippo hooks, the guide block, and the PATH link",
                        "hippo hook과 안내 블록, PATH 링크만 지웁니다"
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
                            let (target, exe) = (target.clone(), exe.clone());
                            i.task.start(&ctx, move || {
                                (Some(key), install_text(&target, &exe, action, true, lang))
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
        card(ui, &tr!(lang, "Delete everything", "완전 삭제"), |ui| {
            ui.weak(tr!(
                lang,
                "Remove, then delete the data root with all records",
                "해제한 뒤 데이터 루트와 모든 기록을 지웁니다"
            ));
            if ui
                .add_enabled(
                    !i.task.running && !i.purge_pending,
                    button(tr!(lang, "Delete everything", "완전 삭제")),
                )
                .clicked()
            {
                i.confirm_purge = true;
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
                        "Apply the changes shown in the preview?",
                        "미리 보기의 변경을 적용할까요?"
                    ),
                    InstallAction::Uninstall => tr!(
                        lang,
                        "Remove the hippo hooks, guide block, and PATH link shown in the preview?",
                        "미리 보기의 hippo hook과 안내 블록, PATH 링크를 지울까요?"
                    ),
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if primary(ui, true, tr!(lang, "Confirm", "확인")).clicked() {
                        let (target, exe) = (target.clone(), exe.clone());
                        i.task.start(&ctx, move || {
                            (None, install_text(&target, &exe, action, false, lang))
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
        if i.confirm_purge {
            let modal = egui::Modal::new(egui::Id::unique("purge-confirm")).show(&ctx, |ui| {
                ui.set_max_width(MODAL_WIDTH);
                ui.label(
                    RichText::new(tr!(lang, "Delete everything", "완전 삭제"))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(4.0);
                let root = target.root.display().to_string();
                ui.label(tr!(
                    lang,
                    "Remove the hooks, then delete {root} and all its records. This cannot be undone.",
                    "hook을 해제한 뒤 {root}와(과) 모든 기록을 지웁니다. 되돌릴 수 없습니다."
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if primary(ui, true, tr!(lang, "Delete", "지우기")).clicked() {
                        i.purge_pending = true;
                        i.confirm_purge = false;
                    }
                    if ui.add(button(tr!(lang, "Cancel", "취소"))).clicked() {
                        i.confirm_purge = false;
                    }
                });
            });
            if modal.should_close() {
                i.confirm_purge = false;
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
            let logo = if ui.visuals().dark_mode {
                &self.art.logo_dark
            } else {
                &self.art.logo
            };
            if let Some(logo) = logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(LOGO_SIZE));
            }
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
        if self.about_license {
            output(ui, "about-text", LICENSE, "");
            return;
        }
        let readme = match lang {
            Lang::En => self.readme_en.as_str(),
            Lang::Ko => self.readme_ko.as_str(),
        };
        egui::ScrollArea::vertical()
            .id_salt("about-readme")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.markdown, readme);
            });
    }

    /// 보이는 탭이 바뀌었으면 화면 스레드 밖에서 기록한다. (FR-GUI-25)
    fn record_tab(&mut self) {
        if self.tab == self.recorded_tab {
            return;
        }
        self.recorded_tab = self.tab;
        let file = self.paths.gui_last_tab();
        let tab = self.tab;
        thread::spawn(move || write_last_tab(&file, tab));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.sync_root(&ctx);
        self.apply_pick();
        self.poll_status(&ctx);
        let lang = self.lang;
        let ready = self.state == Some(InstallState::Installed);
        if self.state.is_some() && !ready && !matches!(self.tab, Tab::Install | Tab::About) {
            self.tab = Tab::Install;
        }
        self.record_tab();
        egui::Panel::top("tabs")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(icon) = &self.art.icon {
                        ui.add(egui::Image::new(icon).fit_to_exact_size(HEADER_ICON_SIZE));
                    }
                    ui.label(RichText::new("hippo").strong().size(17.0));
                    ui.add_space(8.0);
                    for tab in Tab::ALL {
                        let enabled = ready || matches!(tab, Tab::Install | Tab::About);
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.selectable_value(
                                &mut self.tab,
                                tab,
                                RichText::new(tab.label(lang)).size(15.0),
                            );
                        });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &self.status {
                            Some(Ok(_)) => {
                                ui.colored_label(ok_color(ui), tr!(lang, "● daemon", "● 데몬"))
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
            .show(ui, |ui| {
                let notice = match self.state {
                    Some(InstallState::NeedInstall) => Some(tr!(
                        lang,
                        "hippo is not installed in this data root. Apply it in the Claude setup tab.",
                        "이 데이터 루트에 hippo가 설치되어 있지 않습니다. Claude 적용 탭에서 적용하십시오."
                    )),
                    Some(InstallState::NeedUpdate) => Some(tr!(
                        lang,
                        "The installed binary differs from this app. Apply again in the Claude setup tab to update it.",
                        "설치된 실행 파일이 이 앱과 다릅니다. 업데이트하려면 Claude 적용 탭에서 다시 적용하십시오."
                    )),
                    _ => None,
                };
                if let Some(notice) = notice {
                    banner(ui, warn_color(ui), |ui| {
                        ui.label(notice);
                    });
                    ui.add_space(6.0);
                }
                self.show_tab(ui);
            });
    }
}

impl App {
    fn show_tab(&mut self, ui: &mut egui::Ui) {
        match self.tab {
            Tab::Status => self.status_tab(ui),
            Tab::Records => self.records_tab(ui),
            Tab::Settings => self.settings_tab(ui),
            Tab::Import => self.import_tab(ui),
            Tab::Search => self.search_tab(ui),
            Tab::Projects => self.projects_tab(ui),
            Tab::Install => self.install_tab(ui),
            Tab::About => self.about_tab(ui),
        }
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
        ui.colored_label(warn_color(ui), note);
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
    let stored = it
        .stored_label()
        .map(|l| format!("  {l}"))
        .unwrap_or_default();
    ui.label(
        RichText::new(format!(
            "[{}] {}  {}{stored}",
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

fn project_table(
    ui: &mut egui::Ui,
    st: &StatusInfo,
    lang: Lang,
    character: Option<&egui::TextureHandle>,
) {
    if st.projects.is_empty() {
        empty_state(ui, character, |ui| {
            ui.weak(tr!(lang, "No project yet.", "아직 프로젝트가 없습니다."));
        });
        return;
    }
    let heads = [
        "id".to_string(),
        tr!(lang, "location", "위치"),
        tr!(lang, "iterations", "반복"),
        tr!(lang, "results", "결과"),
        tr!(lang, "live queue", "실시간 대기"),
        tr!(lang, "import queue", "가져오기 대기"),
        tr!(lang, "rate · time left", "속도 · 남은 시간"),
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
        .columns(Column::auto().at_least(70.0), 4)
        .column(Column::auto().at_least(170.0))
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
                    let import = p.embed_jobs_import.min(p.embed_jobs);
                    row.col(|ui| cell(ui, &(p.embed_jobs - import).to_string()));
                    row.col(|ui| cell(ui, &import.to_string()));
                    row.col(|ui| cell(ui, &rate_text(p.embed_jobs, p.embed_per_min, lang)));
                    row.col(|ui| cell(ui, &p.vectors.to_string()));
                    row.col(|ui| cell(ui, &p.compatibility));
                });
            }
        });
}

/// 프로젝트별 최근 7일 주입 통계와 에이전트의 기억 활용. (FR-OBS-7)
fn inject_table(ui: &mut egui::Ui, st: &StatusInfo, lang: Lang) {
    let heads = [
        "id".to_string(),
        tr!(lang, "requests", "요청"),
        tr!(lang, "injected", "주입"),
        tr!(lang, "no match", "후보 없음"),
        tr!(lang, "skipped (reason)", "무주입 (사유)"),
        tr!(lang, "timeout", "시간 초과"),
        tr!(lang, "error", "오류"),
        tr!(lang, "opened", "열람"),
        tr!(lang, "search", "검색"),
        tr!(lang, "show", "show"),
    ];
    TableBuilder::new(ui)
        .id_salt("inject-table")
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(80.0))
        .columns(Column::auto().at_least(60.0), 3)
        .column(Column::remainder().at_least(200.0).clip(true))
        .columns(Column::auto().at_least(60.0), 5)
        .header(ROW_HEIGHT, |mut header| {
            for head in &heads {
                header.col(|ui| {
                    ui.strong(head);
                });
            }
        })
        .body(|mut body| {
            for p in &st.projects {
                let s = &p.inject;
                let injected = match inject_ratio(s) {
                    Some(p) => format!("{} ({p}%)", s.injected),
                    None => s.injected.to_string(),
                };
                let reasons: Vec<String> = s
                    .skip_reasons
                    .iter()
                    .map(|(r, n)| format!("{r} {n}"))
                    .collect();
                let skipped = if reasons.is_empty() {
                    s.skipped.to_string()
                } else {
                    format!("{} ({})", s.skipped, reasons.join(", "))
                };
                body.row(ROW_HEIGHT, |mut row| {
                    row.col(|ui| {
                        mono_cell(ui, &p.id.chars().take(SESSION_ID_SHORT).collect::<String>())
                    });
                    row.col(|ui| cell(ui, &s.requests.to_string()));
                    row.col(|ui| cell(ui, &injected));
                    row.col(|ui| cell(ui, &s.no_match.to_string()));
                    row.col(|ui| cell(ui, &skipped));
                    row.col(|ui| cell(ui, &s.timeout.to_string()));
                    row.col(|ui| cell(ui, &s.error.to_string()));
                    row.col(|ui| cell(ui, &s.opened.to_string()));
                    row.col(|ui| cell(ui, &s.search.to_string()));
                    row.col(|ui| cell(ui, &s.show.to_string()));
                });
            }
        });
}

/// 속도와 남은 시간. 대기가 없거나 속도를 모르면 `-`. (FR-GUI-3)
fn rate_text(pending: u64, per_min: Option<u64>, lang: Lang) -> String {
    match per_min.filter(|r| *r > 0 && pending > 0) {
        Some(rate) => {
            let minutes = pending.div_ceil(rate);
            tr!(
                lang,
                "{rate}/min · about {minutes} min",
                "분당 {rate}건 · 약 {minutes}분"
            )
        }
        None => "-".to_string(),
    }
}

/// 모든 프로젝트의 임베딩 큐 요약. (FR-GUI-3)
fn queue_card(ui: &mut egui::Ui, st: &StatusInfo, lang: Lang) {
    let q = queue_summary(st);
    let pending = q.live + q.import;
    ui.horizontal(|ui| {
        stat(
            ui,
            &tr!(lang, "Embedding queue", "임베딩 큐"),
            if pending == 0 {
                RichText::new(tr!(lang, "empty", "대기 없음")).color(ok_color(ui))
            } else {
                RichText::new(tr!(lang, "embedding", "임베딩 중")).color(warn_color(ui))
            },
        );
        stat(
            ui,
            &tr!(lang, "Live", "실시간"),
            RichText::new(q.live.to_string()),
        );
        stat(
            ui,
            &tr!(lang, "Import", "가져오기"),
            RichText::new(q.import.to_string()),
        );
        stat(
            ui,
            &tr!(lang, "Rate · time left", "속도 · 남은 시간"),
            RichText::new(rate_text(pending, q.per_min, lang)),
        );
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
                            ImportStatus::New => ok_color(ui),
                            ImportStatus::LiveOnly => warn_color(ui),
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
    browse: &mut Browse,
) {
    ui.vertical(|ui| {
        ui.set_width(SETTING_KEY_WIDTH);
        let key = RichText::new(field.key).strong();
        ui.label(if modified {
            key.color(changed_color(ui))
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
    field_editor(ui, field, lang, browse);
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

fn field_editor(ui: &mut egui::Ui, field: &mut Field, lang: Lang, browse: &mut Browse) {
    let is_path = (field.section, field.key) == ("import", "transcripts_dir");
    let is_language = (field.section, field.key) == ("ui", "language");
    let is_theme = (field.section, field.key) == ("ui", "theme");
    let is_start_tab = (field.section, field.key) == ("ui", "start_tab");
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
        FieldValue::Text(s) if is_theme => {
            egui::ComboBox::from_id_salt("theme")
                .selected_text(s.clone())
                .width(NUMBER_WIDTH)
                .show_ui(ui, |ui| {
                    for theme in Theme::ALL {
                        ui.selectable_value(s, theme.as_str().to_string(), theme.as_str());
                    }
                });
        }
        FieldValue::Text(s) if is_start_tab => {
            egui::ComboBox::from_id_salt("start_tab")
                .selected_text(s.clone())
                .width(NUMBER_WIDTH)
                .show_ui(ui, |ui| {
                    for tab in StartTab::ALL {
                        ui.selectable_value(s, tab.as_str().to_string(), tab.as_str());
                    }
                });
        }
        FieldValue::Text(s) if is_path => {
            path_input(
                ui,
                TEXT_FIELD_WIDTH,
                s,
                "",
                PathField::TranscriptsDir,
                browse,
            );
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
                        if ui.small_button("×").clicked() {
                            remove = Some(n);
                        }
                    });
                }
                if let Some(n) = remove {
                    items.remove(n);
                }
                if ui
                    .add(
                        egui::Button::new(tr!(lang, "+ add", "+ 추가"))
                            .small()
                            .wrap_mode(egui::TextWrapMode::Extend),
                    )
                    .clicked()
                {
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
                        if ui.small_button("×").clicked() {
                            remove = Some(n);
                        }
                    });
                }
                if let Some(n) = remove {
                    patterns.remove(n);
                }
                if ui
                    .add(
                        egui::Button::new(tr!(lang, "+ add", "+ 추가"))
                            .small()
                            .wrap_mode(egui::TextWrapMode::Extend),
                    )
                    .clicked()
                {
                    patterns.push(PatternEntry {
                        kind: String::new(),
                        pattern: String::new(),
                    });
                }
            });
        }
    }
}

/// 설정 파일의 (주입 동작, 저장 동작). 읽지 못하면 기본값(둘 다 동작)이다. (FR-GUI-3)
fn load_switches(paths: &Paths) -> (bool, bool) {
    let config = Config::load(&paths.config()).unwrap_or_default();
    (config.inject.enabled, config.capture.enabled)
}

/// `hippo install`/`uninstall`과 같은 동작. 복사할 실행 파일은 실행 중인 hippo이다. (FR-GUI-10)
fn install_text(
    target: &InstallTarget,
    exe: &Path,
    action: InstallAction,
    dry_run: bool,
    lang: Lang,
) -> Result<String, String> {
    let mut out = String::new();
    let result = match action {
        InstallAction::Install => run_install(target, dry_run, exe, lang, &mut out),
        InstallAction::Uninstall => run_uninstall(target, dry_run, lang, &mut out),
    };
    match result {
        Ok(()) => Ok(out),
        Err(e) => Err(format!("{out}{e}")),
    }
}

/// `hippo purge`와 같은 동작. 확인은 대화창에서 받았다. (FR-GUI-10, FR-INS-13)
fn purge_text(target: &InstallTarget, lang: Lang) -> Result<String, String> {
    let mut out = String::new();
    let result = run_uninstall(target, false, lang, &mut out)
        .and_then(|()| purge_data(&target.root, &target.home, lang, &mut out));
    match result {
        Ok(()) => Ok(out),
        Err(e) => Err(format!("{out}{e}")),
    }
}

/// 빈 상태. 캐릭터 이미지 아래에 안내를 둔다. (FR-GUI-20)
fn empty_state(
    ui: &mut egui::Ui,
    character: Option<&egui::TextureHandle>,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        if let Some(character) = character {
            ui.add(egui::Image::new(character).fit_to_exact_size(CHARACTER_SIZE));
            ui.add_space(8.0);
        }
        add(ui);
    });
}

/// 현재 테마의 상태 색. (FR-GUI-21)
fn ok_color(ui: &egui::Ui) -> egui::Color32 {
    theme::palette(ui.visuals().dark_mode).ok
}

fn warn_color(ui: &egui::Ui) -> egui::Color32 {
    theme::palette(ui.visuals().dark_mode).warn
}

fn changed_color(ui: &egui::Ui) -> egui::Color32 {
    theme::palette(ui.visuals().dark_mode).changed
}
