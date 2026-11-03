//! CLI와 GUI가 함께 쓰는 명령 동작. 출력 텍스트를 돌려주고 출력 방법은 호출한 쪽이 정한다.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::client::{
    Launcher, STARTUP_WAIT, ensure_daemon, launch_disabled, request, spawn_daemon_process,
};
use crate::config::{Config, Paths};
use crate::i18n::Lang;
use crate::ingest::ImportOutcome;
use crate::install::{
    Action, Outcome, Scope, apply, apply_memory, forget_install, guide_block, install_binary,
    installed_exe, memory_file, record_install, target_file, warn_untracked,
};
use crate::protocol::{DeleteReport, ImportSessionReport, Request, Response, Scope as QueryScope};
use crate::search::escape_control;
use crate::setup::hooks_config;
use crate::tr;

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
    let config = Config::load(&paths.config()).map_err(|e| e.to_string())?;
    ensure_daemon(paths, launcher(&config), STARTUP_WAIT);
    request(&paths.socket(), req, timeout).map_err(|e| e.to_string())
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

/// `--project`(없으면 `cwd`)를 실제 경로로 바꿔 설치 범위를 만든다. (FR-INS-1)
pub fn project_scope(project: Option<PathBuf>, cwd: &Path) -> Result<Scope, String> {
    let dir = project.unwrap_or_else(|| cwd.to_path_buf());
    std::fs::canonicalize(&dir)
        .or_else(|_| std::path::absolute(&dir))
        .map(Scope::Project)
        .map_err(|e| e.to_string())
}

/// hook 설정과 메모리 파일의 안내 블록을 함께 바꾼다. 출력은 `out`에 쌓는다. (IF-CLI-6, FR-INS-7, FR-GUI-10)
pub fn run_install(
    paths: &Paths,
    scope: &Scope,
    install: bool,
    dry_run: bool,
    exe: &Path,
    lang: Lang,
    out: &mut String,
) -> Result<(), String> {
    let root = std::path::absolute(&paths.root).map_err(|e| e.to_string())?;
    let settings_path = target_file(scope).map_err(|e| e.to_string())?;
    let (action, block) = if install {
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
        let hooks = hook_settings(paths, &binary.path)?;
        (Action::Install(hooks), Some(guide_block(&binary.path)))
    } else {
        (Action::Uninstall, None)
    };
    let settings = apply(&settings_path, &action, dry_run).map_err(|e| e.to_string())?;
    let memory = memory_file(scope)
        .and_then(|path| apply_memory(&path, block.as_deref(), dry_run))
        .map_err(|e| e.to_string())?;
    out.push_str(&outcome_text(&settings, dry_run, lang));
    out.push_str(&outcome_text(&memory, dry_run, lang));
    let removed_binary = if install {
        if !dry_run {
            record_install(&root, &settings_path).map_err(|e| e.to_string())?;
        }
        false
    } else {
        forget_install(&root, &settings_path, dry_run).map_err(|e| e.to_string())?
    };
    if removed_binary {
        let path = installed_exe(&root);
        let path = path.display();
        let line = if dry_run {
            tr!(
                lang,
                "(dry-run) Would remove binary: {path}",
                "(dry-run) 지울 실행 파일: {path}"
            )
        } else {
            tr!(
                lang,
                "Removed binary: {path}",
                "실행 파일을 지웠습니다: {path}"
            )
        };
        let _ = writeln!(out, "{line}");
    }
    if let Scope::Project(dir) = scope
        && install
        && warn_untracked(dir)
    {
        let path = memory.path.display();
        let _ = writeln!(
            out,
            "{}",
            tr!(
                lang,
                "Warning: {path} is not ignored by git and may be committed. Add it to .gitignore if it should stay private.",
                "경고: {path}이(가) git에서 무시되지 않아 커밋될 수 있습니다. 개인 파일로 두려면 .gitignore에 추가하십시오."
            )
        );
    }
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
