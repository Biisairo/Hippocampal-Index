//! Claude Code 설정 파일에 hippo hook을 병합하거나 제거한다. (SRS 3.2.8)

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use similar::TextDiff;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("settings file is not valid JSON ({path}): {message}")]
    Parse { path: PathBuf, message: String },
    #[error("unexpected settings structure: {0}")]
    Shape(String),
    #[error("HOME is not set")]
    NoHome,
    #[error(
        "refusing to modify through a symbolic link: {0} (apply to the real file path instead)"
    )]
    Symlink(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    User,
    Project(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `hooks_config()` 결과를 병합한다.
    Install(Value),
    Uninstall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    pub changed: bool,
    pub diff: String,
    pub backup: Option<PathBuf>,
}

const HOOK_EVENTS: &[&str] = &[
    "post-tool-batch",
    "stop",
    "stop-failure",
    "session-end",
    "user-prompt-submit",
];
const EXE_NAME: &str = "hippo";
const CLAUDE_DIR: &str = ".claude";
const USER_SETTINGS: &str = "settings.json";
const PROJECT_SETTINGS: &str = "settings.local.json";
const BACKUP_SUFFIX: &str = "hippo-backup";
const MAX_BACKUP_ATTEMPTS: u32 = 100;
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;

pub fn target_file(scope: &Scope) -> Result<PathBuf, InstallError> {
    Ok(match scope {
        Scope::User => {
            let home = std::env::var_os("HOME").ok_or(InstallError::NoHome)?;
            PathBuf::from(home).join(CLAUDE_DIR).join(USER_SETTINGS)
        }
        Scope::Project(dir) => dir.join(CLAUDE_DIR).join(PROJECT_SETTINGS),
    })
}

/// `<경로>/hippo hook <이벤트>` 형태인지 확인한다. (FR-INS-2)
pub fn is_hippo_command(command: &str) -> bool {
    let command = command.trim();
    HOOK_EVENTS.iter().any(|event| {
        command
            .strip_suffix(&format!(" hook {event}"))
            .is_some_and(|exe| {
                !exe.chars().any(char::is_whitespace)
                    && Path::new(exe).file_name().is_some_and(|n| n == EXE_NAME)
            })
    })
}

pub fn merge_install(settings: &mut Value, hooks: &Value) -> Result<(), InstallError> {
    let mut next = settings.clone();
    remove_hippo_hooks(&mut next)?;
    let ours = hooks
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or_else(|| InstallError::Shape("hook config has no hooks object".into()))?;
    let root = next
        .as_object_mut()
        .ok_or_else(|| InstallError::Shape("settings root is not an object".into()))?;
    let target = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| InstallError::Shape("hooks is not an object".into()))?;
    for (event, groups) in ours {
        let groups = groups
            .as_array()
            .ok_or_else(|| InstallError::Shape(format!("hook config {event} is not an array")))?;
        let list = target
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| InstallError::Shape(format!("hooks.{event} is not an array")))?;
        list.extend(groups.iter().cloned());
    }
    *settings = next;
    Ok(())
}

pub fn merge_uninstall(settings: &mut Value) -> Result<(), InstallError> {
    let mut next = settings.clone();
    remove_hippo_hooks(&mut next)?;
    *settings = next;
    Ok(())
}

/// hippo hook만 지우고, 그 때문에 비게 된 그룹, 이벤트, `hooks` 키를 지운다. (FR-INS-3, 4)
fn remove_hippo_hooks(settings: &mut Value) -> Result<(), InstallError> {
    let root = settings
        .as_object_mut()
        .ok_or_else(|| InstallError::Shape("settings root is not an object".into()))?;
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(());
    };
    let events = hooks
        .as_object_mut()
        .ok_or_else(|| InstallError::Shape("hooks is not an object".into()))?;
    let mut removed_any = false;
    let mut emptied_events = Vec::new();
    for (event, groups) in events.iter_mut() {
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| InstallError::Shape(format!("hooks.{event} is not an array")))?;
        let before = groups.len();
        let mut removed_here = false;
        groups.retain_mut(|group| {
            let Some(inner) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let count = inner.len();
            inner.retain(|h| {
                !h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_hippo_command)
            });
            if inner.len() == count {
                return true;
            }
            removed_here = true;
            !inner.is_empty()
        });
        if removed_here {
            removed_any = true;
            if groups.is_empty() && before > 0 {
                emptied_events.push(event.clone());
            }
        }
    }
    for event in emptied_events {
        events.shift_remove(&event);
    }
    if removed_any && events.is_empty() {
        root.shift_remove("hooks");
    }
    Ok(())
}

pub fn apply(path: &Path, action: &Action, dry_run: bool) -> Result<Outcome, InstallError> {
    reject_symlinks(path)?;
    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let current: Value = match &existing {
        Some(text) => serde_json::from_str(text).map_err(|e| InstallError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?,
        None => Value::Object(Map::new()),
    };
    let mut next = current.clone();
    match action {
        Action::Install(hooks) => merge_install(&mut next, hooks)?,
        Action::Uninstall => merge_uninstall(&mut next)?,
    }
    let unchanged = Outcome {
        path: path.to_path_buf(),
        changed: false,
        diff: String::new(),
        backup: None,
    };
    if next == current {
        return Ok(unchanged);
    }
    let before = existing
        .as_ref()
        .map_or_else(String::new, |_| render(&current));
    let after = render(&next);
    let diff = TextDiff::from_lines(&before, &after)
        .unified_diff()
        .header(&path.display().to_string(), &path.display().to_string())
        .to_string();
    if dry_run {
        return Ok(Outcome {
            diff,
            changed: true,
            ..unchanged
        });
    }
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    reject_symlinks(path)?;
    let backup = match &existing {
        Some(original) => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let backup = write_backup(path, secs, original.as_bytes())?;
            Some(backup)
        }
        None => None,
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(after.as_bytes())?;
    if existing.is_some() {
        tmp.as_file()
            .set_permissions(fs::symlink_metadata(path)?.permissions())?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(Outcome {
        path: path.to_path_buf(),
        changed: true,
        diff,
        backup,
    })
}

/// 설정 파일이나 바로 위 디렉터리가 심볼릭 링크이면 거부한다. 링크를 따라 다른 파일을 덮어쓰지 않기 위해서다.
fn reject_symlinks(path: &Path) -> Result<(), InstallError> {
    for candidate in [Some(path), path.parent()].into_iter().flatten() {
        if fs::symlink_metadata(candidate).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(InstallError::Symlink(candidate.to_path_buf()));
        }
    }
    Ok(())
}

/// `<파일>.hippo-backup-<초>[-n]`에 배타적으로 쓴다. 이름이 이미 있으면 번호를 붙여 다시 시도한다.
fn write_backup(path: &Path, secs: u64, content: &[u8]) -> Result<PathBuf, InstallError> {
    for n in 0..MAX_BACKUP_ATTEMPTS {
        let suffix = if n == 0 {
            format!("{BACKUP_SUFFIX}-{secs}")
        } else {
            format!("{BACKUP_SUFFIX}-{secs}-{n}")
        };
        let backup = sibling(path, &suffix);
        match create_new_private(&backup) {
            Ok(mut file) => {
                file.write_all(content)?;
                return Ok(backup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free backup file name",
    )
    .into())
}

/// 링크를 따르지 않고 새 파일을 0600으로 만든다. 같은 이름이 있으면 실패한다.
fn create_new_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn render(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}
