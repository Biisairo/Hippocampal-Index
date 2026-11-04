//! CLI와 GUI가 함께 쓰는 명령 동작. 출력 텍스트를 돌려주고 출력 방법은 호출한 쪽이 정한다.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::client::{
    Launcher, STARTUP_WAIT, ensure_daemon, launch_disabled, request, request_until_reply,
    spawn_daemon_process,
};
use crate::config::{Config, Paths};
use crate::i18n::Lang;
use crate::ingest::ImportOutcome;
use crate::install::{
    Action, Outcome, apply, apply_memory, guide_block, install_binary, installed_exe, memory_file,
    settings_file,
};
use crate::layout::{
    LinkState, create_link, forget_claude_dir, link_state, path_has_dir, path_link,
    record_claude_dir, remove_link, remove_root, stop_ollama,
};
use crate::protocol::{DeleteReport, ImportSessionReport, Request, Response, Scope as QueryScope};
use crate::search::escape_control;
use crate::setup::hooks_config;
use crate::tr;

/// 실행 파일을 바꾼 뒤 이전 데몬에 보내는 종료 요청의 응답 대기 시간. (FR-INS-8)
const SHUTDOWN_REPLY: Duration = Duration::from_secs(5);
/// 수동 중지가 데몬 종료를 기다리는 시간. (FR-DMN-12)
const STOP_DAEMON_WAIT: Duration = Duration::from_secs(10);
const STOP_POLL: Duration = Duration::from_millis(50);
/// UserPromptSubmit hook timeout(초)은 시간 예산보다 이만큼 여유를 둔다.
const INJECT_TIMEOUT_MARGIN_SECS: u64 = 2;
pub const SESSION_ID_SHORT: usize = 8;
pub const SCOPE_PROJECT: &str = "project";
pub const SCOPE_ALL: &str = "all";

/// 설정에 따라 데몬을 띄우는 방법을 고른다. (FR-DMN-7)
pub fn launcher(config: &Config) -> Launcher<'static> {
    if config.daemon.autostart {
        &spawn_daemon_process
    } else {
        &launch_disabled
    }
}

/// 데몬이 없으면 설정에 따라 띄운 뒤 요청한다. (IF-CLI-1, FR-GUI-1)
pub fn query(paths: &Paths, req: &Request, timeout: Duration) -> Result<Response, String> {
    send_query(paths, req, Some(timeout))
}

/// `query`와 같지만 응답이 올 때까지 시간 제한 없이 기다린다. (FR-GUI-15)
pub fn query_until_reply(paths: &Paths, req: &Request) -> Result<Response, String> {
    send_query(paths, req, None)
}

fn send_query(paths: &Paths, req: &Request, timeout: Option<Duration>) -> Result<Response, String> {
    let config = Config::load(&paths.config()).map_err(|e| e.to_string())?;
    ensure_daemon(paths, launcher(&config), STARTUP_WAIT);
    let reply = match timeout {
        Some(timeout) => request(&paths.socket(), req, timeout),
        None => request_until_reply(&paths.socket(), req),
    };
    reply.map_err(|e| e.to_string())
}

/// 상대 경로는 클라이언트의 현재 디렉터리 기준으로 바꾼다. 데몬의 현재 디렉터리와 다르기 때문이다.
pub fn absolute(cwd: &str, path: &str) -> String {
    PathBuf::from(cwd).join(path).to_string_lossy().into_owned()
}

/// `project`, `all`, 그 밖은 경로로 해석한다. (IF-CLI-1)
pub fn scope_arg(cwd: &str, scope: &str) -> QueryScope {
    match scope {
        SCOPE_PROJECT => QueryScope::Project,
        SCOPE_ALL => QueryScope::All,
        path => QueryScope::Path(absolute(cwd, path)),
    }
}

/// 실행 중인 hippo 경로로 hook 설정을 만든다. UserPromptSubmit timeout은 콜드 시작 예산보다 여유를 둔다. (IF-HK-4)
pub fn hook_settings(paths: &Paths, exe: &Path) -> Result<serde_json::Value, String> {
    let budget_ms = Config::load(&paths.config())
        .map_err(|e| e.to_string())?
        .inject
        .cold_budget_ms;
    let timeout = budget_ms.div_ceil(1_000) + INJECT_TIMEOUT_MARGIN_SECS;
    Ok(hooks_config(exe, timeout))
}

/// 설치·제거 대상. 경로는 절대 경로로 바꿔 쓴다. (IF-CLI-6, FR-GUI-10)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallTarget {
    pub root: PathBuf,
    pub claude_dir: PathBuf,
    pub home: PathBuf,
}

impl InstallTarget {
    fn absolute(&self) -> Result<(PathBuf, PathBuf), String> {
        let abs = |p: &Path| std::path::absolute(p).map_err(|e| e.to_string());
        Ok((abs(&self.root)?, abs(&self.claude_dir)?))
    }
}

/// 실행 파일 복사, hook, 안내 블록, PATH 링크, Claude 폴더 기록. 출력은 `out`에 쌓는다. (FR-INS-*)
pub fn run_install(
    target: &InstallTarget,
    dry_run: bool,
    exe: &Path,
    lang: Lang,
    out: &mut String,
) -> Result<(), String> {
    let (root, claude_dir) = target.absolute()?;
    let installed = installed_exe(&root);
    let link = path_link(&target.home);
    if link_state(&link, &installed) == LinkState::Foreign {
        let link = link.display();
        return Err(tr!(
            lang,
            "{link} already exists and does not point to this install; nothing was changed",
            "{link}이(가) 이미 있고 이 설치를 가리키지 않습니다. 아무것도 바꾸지 않았습니다"
        ));
    }
    let binary = install_binary(exe, &root, dry_run).map_err(|e| e.to_string())?;
    let path = binary.path.display();
    let line = match (binary.copied, dry_run) {
        (true, false) => tr!(
            lang,
            "Installed binary: {path}",
            "실행 파일을 설치했습니다: {path}"
        ),
        (true, true) => tr!(
            lang,
            "(dry-run) Would install binary: {path}",
            "(dry-run) 실행 파일을 설치할 위치: {path}"
        ),
        (false, _) => tr!(
            lang,
            "Binary up to date: {path}",
            "실행 파일이 최신입니다: {path}"
        ),
    };
    let _ = writeln!(out, "{line}");
    if binary.copied
        && !dry_run
        && let Err(e) = shutdown_daemon(&root)
    {
        let _ = writeln!(
            out,
            "{}",
            tr!(
                lang,
                "Warning: the running daemon did not stop ({e}). Restart it later so it uses the new binary.",
                "경고: 실행 중인 데몬이 멈추지 않았습니다({e}). 새 실행 파일을 쓰도록 나중에 다시 시작하십시오."
            )
        );
    }
    let hooks = hook_settings(&Paths::new(root.clone()), &binary.path)?;
    let settings = apply(
        &settings_file(&claude_dir),
        &Action::Install(hooks),
        dry_run,
    )
    .map_err(|e| e.to_string())?;
    let memory = apply_memory(
        &memory_file(&claude_dir),
        Some(&guide_block(&binary.path)),
        dry_run,
    )
    .map_err(|e| e.to_string())?;
    out.push_str(&outcome_text(&settings, dry_run, lang));
    out.push_str(&outcome_text(&memory, dry_run, lang));
    let shown = link.display();
    let line = if dry_run {
        match link_state(&link, &installed) {
            LinkState::Ours => tr!(
                lang,
                "Link up to date: {shown}",
                "링크가 최신입니다: {shown}"
            ),
            _ => tr!(
                lang,
                "(dry-run) Would link: {shown} -> {path}",
                "(dry-run) 만들 링크: {shown} -> {path}"
            ),
        }
    } else if create_link(&link, &installed).map_err(|e| e.to_string())? {
        tr!(
            lang,
            "Linked: {shown} -> {path}",
            "링크를 만들었습니다: {shown} -> {path}"
        )
    } else {
        tr!(
            lang,
            "Link up to date: {shown}",
            "링크가 최신입니다: {shown}"
        )
    };
    let _ = writeln!(out, "{line}");
    if !dry_run {
        record_claude_dir(&root, &claude_dir).map_err(|e| e.to_string())?;
    }
    let link_dir = link.parent().unwrap_or(&link);
    let on_path = std::env::var_os("PATH").is_some_and(|p| path_has_dir(&p, link_dir));
    if !on_path {
        let dir = link_dir.display();
        let _ = writeln!(
            out,
            "{}",
            tr!(
                lang,
                "Warning: {dir} is not on PATH. Add it to PATH to run `hippo` directly in a terminal.",
                "경고: {dir}이(가) PATH에 없습니다. 터미널에서 `hippo`를 바로 쓰려면 PATH에 더하십시오."
            )
        );
    }
    Ok(())
}

/// hook, 안내 블록, PATH 링크만 지운다. 실행 파일과 데이터는 남긴다. (FR-INS-9)
pub fn run_uninstall(
    target: &InstallTarget,
    dry_run: bool,
    lang: Lang,
    out: &mut String,
) -> Result<(), String> {
    let (root, claude_dir) = target.absolute()?;
    let settings = apply(&settings_file(&claude_dir), &Action::Uninstall, dry_run)
        .map_err(|e| e.to_string())?;
    let memory =
        apply_memory(&memory_file(&claude_dir), None, dry_run).map_err(|e| e.to_string())?;
    out.push_str(&outcome_text(&settings, dry_run, lang));
    out.push_str(&outcome_text(&memory, dry_run, lang));
    let installed = installed_exe(&root);
    let link = path_link(&target.home);
    let shown = link.display();
    let line = match (link_state(&link, &installed), dry_run) {
        (LinkState::Missing, _) => tr!(lang, "No change: {shown}", "변경 없음: {shown}"),
        (LinkState::Foreign, _) => tr!(
            lang,
            "Left {shown}: it does not point to this install",
            "{shown}은(는) 이 설치를 가리키지 않아 그대로 둡니다"
        ),
        (LinkState::Ours, true) => tr!(
            lang,
            "(dry-run) Would remove link: {shown}",
            "(dry-run) 지울 링크: {shown}"
        ),
        (LinkState::Ours, false) => {
            remove_link(&link, &installed).map_err(|e| e.to_string())?;
            tr!(lang, "Removed link: {shown}", "링크를 지웠습니다: {shown}")
        }
    };
    let _ = writeln!(out, "{line}");
    if !dry_run {
        forget_claude_dir(&root, &claude_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 데이터 루트의 데몬에 종료를 요청한다. 종료를 받아들였으면 true, 데몬이 없으면 false. (FR-INS-8, 13)
fn shutdown_daemon(root: &Path) -> Result<bool, String> {
    let socket = Paths::new(root.to_path_buf()).socket();
    if !socket.exists() {
        return Ok(false);
    }
    match request(&socket, &Request::Shutdown, SHUTDOWN_REPLY) {
        Ok(Response::Done) => Ok(true),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected response: {other:?}")),
        Err(_) => Ok(false),
    }
}

/// 데몬과 hippo가 띄운 Ollama를 멈춘다. 데몬은 소켓 파일이 없어질 때까지 기다린다. (FR-DMN-12)
pub fn stop_services(root: &Path, lang: Lang, out: &mut String) -> Result<(), String> {
    let socket = Paths::new(root.to_path_buf()).socket();
    let daemon = shutdown_daemon(root)?;
    if daemon {
        let deadline = std::time::Instant::now() + STOP_DAEMON_WAIT;
        while socket.exists() {
            if std::time::Instant::now() >= deadline {
                return Err(tr!(
                    lang,
                    "the daemon did not stop in time",
                    "데몬이 제때 멈추지 않았습니다"
                ));
            }
            std::thread::sleep(STOP_POLL);
        }
        let _ = writeln!(
            out,
            "{}",
            tr!(lang, "Stopped the daemon.", "데몬을 멈췄습니다.")
        );
    }
    let ollama = stop_ollama(root);
    if ollama {
        let _ = writeln!(
            out,
            "{}",
            tr!(
                lang,
                "Stopped the Ollama server hippo started.",
                "hippo가 띄운 Ollama 서버를 멈췄습니다."
            )
        );
    }
    if !daemon && !ollama {
        let _ = writeln!(
            out,
            "{}",
            tr!(lang, "Nothing to stop.", "멈출 것이 없습니다.")
        );
    }
    Ok(())
}

/// 데몬과 hippo가 띄운 Ollama를 멈춘 뒤 데이터 루트를 통째로 지운다. 제거와 확인은 호출한 쪽이 먼저 한다. (FR-INS-13)
pub fn purge_data(root: &Path, home: &Path, lang: Lang, out: &mut String) -> Result<(), String> {
    let root = std::path::absolute(root).map_err(|e| e.to_string())?;
    stop_services(&root, lang, out)?;
    remove_root(&root, home)?;
    let shown = root.display();
    let _ = writeln!(
        out,
        "{}",
        tr!(
            lang,
            "Deleted data root: {shown}",
            "데이터 루트를 지웠습니다: {shown}"
        )
    );
    Ok(())
}

fn outcome_text(outcome: &Outcome, dry_run: bool, lang: Lang) -> String {
    let path = outcome.path.display();
    if !outcome.changed {
        return format!("{}\n", tr!(lang, "No change: {path}", "변경 없음: {path}"));
    }
    let mut text = outcome.diff.clone();
    if dry_run {
        let _ = writeln!(
            text,
            "{}",
            tr!(
                lang,
                "(dry-run) Nothing written: {path}",
                "(dry-run) 파일을 쓰지 않았습니다: {path}"
            )
        );
        return text;
    }
    let _ = writeln!(
        text,
        "{}",
        tr!(lang, "Applied: {path}", "적용했습니다: {path}")
    );
    if let Some(backup) = &outcome.backup {
        let backup = backup.display();
        let _ = writeln!(text, "{}", tr!(lang, "Backup: {backup}", "백업: {backup}"));
    }
    text
}

/// 가져오기 결과 합계. (FR-IMP-1)
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportTally {
    pub imported: usize,
    pub unchanged: usize,
    pub other: usize,
    pub errors: usize,
    pub iterations: usize,
    pub fork_skipped: usize,
    pub pending_embeds: u64,
}

impl ImportTally {
    pub fn add(&mut self, r: &ImportSessionReport) {
        match r.outcome {
            ImportOutcome::Imported => self.imported += 1,
            ImportOutcome::Unchanged => self.unchanged += 1,
            ImportOutcome::OtherProject => self.other += 1,
        }
        self.iterations += r.iterations;
        self.fork_skipped += r.fork_skipped;
        self.pending_embeds = r.pending_embeds;
    }

    /// 합계 줄과, 남은 임베딩 작업이 있으면 안내 줄.
    pub fn summary(&self, total: usize, lang: Lang) -> String {
        let mut text = tr!(
            lang,
            "Total: {total} sessions; imported {}, unchanged {}, other project {}, errors {}. Stored {} iterations, skipped {} fork copies.\n",
            "합계: 세션 {total}개 중 가져옴 {}, 변경 없음 {}, 다른 프로젝트 {}, 오류 {}. 반복 {}개 저장, fork 복사본 {}개 건너뜀.\n",
            self.imported,
            self.unchanged,
            self.other,
            self.errors,
            self.iterations,
            self.fork_skipped
        );
        if self.pending_embeds > 0 {
            text.push_str(&tr!(
                lang,
                "The daemon keeps working on the {} pending embedding jobs. Check progress with `hippo status`.\n",
                "남은 임베딩 작업 {}개는 데몬이 계속 처리합니다. 진행 상태는 `hippo status`로 확인할 수 있습니다.\n",
                self.pending_embeds
            ));
        }
        text
    }
}

pub fn outcome_label(outcome: ImportOutcome, lang: Lang) -> &'static str {
    match (outcome, lang) {
        (ImportOutcome::Imported, Lang::En) => "imported",
        (ImportOutcome::Unchanged, Lang::En) => "unchanged",
        (ImportOutcome::OtherProject, Lang::En) => "other project",
        (ImportOutcome::Imported, Lang::Ko) => "가져옴",
        (ImportOutcome::Unchanged, Lang::Ko) => "변경 없음",
        (ImportOutcome::OtherProject, Lang::Ko) => "다른 프로젝트",
    }
}

/// 세션 하나의 결과 줄 `[n/m] <id 앞 8자> <결과>`. 결과를 합계에 더한다. (FR-IMP-1)
pub fn session_line(
    n: usize,
    total: usize,
    session_id: &str,
    result: Result<&ImportSessionReport, &str>,
    tally: &mut ImportTally,
    lang: Lang,
) -> String {
    let short: String = session_id.chars().take(SESSION_ID_SHORT).collect();
    let line = match result {
        Ok(r) => {
            tally.add(r);
            tr!(
                lang,
                "{}  iterations {}  pending embeddings {}",
                "{} 반복 {}  남은 임베딩 {}",
                outcome_label(r.outcome, lang),
                r.iterations,
                r.pending_embeds
            )
        }
        Err(message) => {
            tally.errors += 1;
            tr!(lang, "error: {message}", "오류: {message}")
        }
    };
    escape_control(&format!("[{n}/{total}] {short} {line}"))
}

/// 삭제를 진행할지. (FR-DEL-4)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Proceed,
    Cancel,
    /// 표준 입력이 터미널이 아니고 `--yes`도 없다.
    NeedYes,
}

/// `--yes`면 묻지 않고 진행한다. 터미널이면 `answer`로 한 줄을 읽어 `y`일 때만 진행한다. (FR-DEL-4)
pub fn confirm_delete(
    yes: bool,
    terminal: bool,
    answer: impl FnOnce() -> Option<String>,
) -> Confirm {
    if yes {
        return Confirm::Proceed;
    }
    if !terminal {
        return Confirm::NeedYes;
    }
    match answer() {
        Some(line) if line.trim().eq_ignore_ascii_case("y") => Confirm::Proceed,
        _ => Confirm::Cancel,
    }
}

/// 삭제 미리 보기. 프로젝트 id와 위치, 세션·턴·반복·결과 수. (FR-DEL-4)
pub fn render_delete_report(report: &DeleteReport, lang: Lang) -> String {
    let c = &report.counts;
    let text = tr!(
        lang,
        "Project {} ({})\n  sessions: {}  turns: {}  iterations: {}  results: {}\nAll vectors of these iterations and results are deleted too.\n",
        "프로젝트 {} ({})\n  세션 {}개, 턴 {}개, 반복 {}개, 결과 {}개\n이 반복과 결과의 벡터도 모두 지웁니다.\n",
        report.project_id,
        report.location,
        c.sessions,
        c.turns,
        c.iterations,
        c.results
    );
    escape_control(&text)
}
