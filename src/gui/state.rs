//! 화면과 분리한 GUI 상태 로직. (FR-GUI-7, FR-GUI-8, FR-GUI-10)

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::commands::{ImportTally, absolute, session_line};
use crate::i18n::Lang;
use crate::import::{Candidate, ImportStatus};
use crate::protocol::{Request, Response};
use crate::registry::is_project_id;

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
    user_scope: bool,
    project: String,
    action: InstallAction,
}

/// Claude 적용 탭의 입력. (FR-GUI-10)
#[derive(Debug, Clone, Default)]
pub struct InstallForm {
    pub user_scope: bool,
    pub project: String,
    previewed: Option<PreviewKey>,
}

impl InstallForm {
    pub fn new(cwd: &str) -> Self {
        Self {
            project: cwd.to_string(),
            ..Self::default()
        }
    }

    pub fn key(&self, action: InstallAction) -> PreviewKey {
        PreviewKey {
            user_scope: self.user_scope,
            project: self.project.clone(),
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
