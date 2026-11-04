//! 데이터 루트 판정, PATH 링크, Claude 폴더 기록, 설치 상태, 완전 삭제. (SRS 3.2.15, FR-INS-11~13, FR-GUI-22~23)

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fsutil::{ensure_private_dir, set_private_file};
use crate::install::{installed_exe, same_content};

const EXE_NAME: &str = "hippo";
const BIN_DIR: &str = "bin";
const CLAUDE_DIR_FILE: &str = "claude-dir";
const DEFAULT_DATA_DIR: &str = ".hippo";
const DEFAULT_CLAUDE_DIR: &str = ".claude";
const LINK_DIR: &[&str] = &[".local", "bin"];
const APP_EXTENSION: &str = "app";
const APP_CONTENTS: &str = "Contents";
const APP_MACOS: &str = "MacOS";
const OLLAMA_NAME: &str = "ollama";
/// hippo가 띄운 Ollama 서버의 pid를 담는 파일. 데이터 루트 아래에 둔다. (FR-EMB-4)
pub const OLLAMA_PID: &str = "ollama.pid";
#[cfg(unix)]
const LINK_DIR_MODE: u32 = 0o755;

/// 링크를 푼 실제 경로가 `<루트>/bin/hippo`이면 그 루트. (FR-ROOT-1)
pub fn root_of_exe(exe: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(exe).ok()?;
    let bin = real.parent()?;
    (real.file_name()? == EXE_NAME && bin.file_name()? == BIN_DIR)
        .then(|| bin.parent().map(Path::to_path_buf))
        .flatten()
}

/// 링크를 푼 실제 경로가 `<이름>.app/Contents/MacOS/` 안이면 true. (IF-CLI-14)
pub fn is_app_exe(exe: &Path) -> bool {
    let Ok(real) = fs::canonicalize(exe) else {
        return false;
    };
    let macos = real.parent();
    let contents = macos.and_then(Path::parent);
    let bundle = contents.and_then(Path::parent);
    macos.and_then(Path::file_name) == Some(OsStr::new(APP_MACOS))
        && contents.and_then(Path::file_name) == Some(OsStr::new(APP_CONTENTS))
        && bundle.and_then(Path::extension) == Some(OsStr::new(APP_EXTENSION))
}

/// `$HOME/.hippo`
pub fn default_data_root(home: &Path) -> PathBuf {
    home.join(DEFAULT_DATA_DIR)
}

/// PATH 링크 `~/.local/bin/hippo`. (FR-INS-11)
pub fn path_link(home: &Path) -> PathBuf {
    LINK_DIR
        .iter()
        .fold(home.to_path_buf(), |p, part| p.join(part))
        .join(EXE_NAME)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Missing,
    /// `target`을 가리키는 심볼릭 링크.
    Ours,
    /// 다른 곳을 가리키는 링크 또는 링크가 아닌 파일.
    Foreign,
}

pub fn link_state(link: &Path, target: &Path) -> LinkState {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return LinkState::Missing;
    };
    if !meta.file_type().is_symlink() {
        return LinkState::Foreign;
    }
    let points_here = fs::read_link(link).is_ok_and(|t| t == target)
        || matches!(
            (fs::canonicalize(link), fs::canonicalize(target)),
            (Ok(a), Ok(b)) if a == b
        );
    if points_here {
        LinkState::Ours
    } else {
        LinkState::Foreign
    }
}

/// 링크를 만들었으면 true, 이미 있었으면 false. 다른 것이 있으면 오류. (FR-INS-11)
pub fn create_link(link: &Path, target: &Path) -> io::Result<bool> {
    match link_state(link, target) {
        LinkState::Ours => return Ok(false),
        LinkState::Foreign => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} already exists and does not point to {}; remove it or run uninstall from the other install first",
                    link.display(),
                    target.display()
                ),
            ));
        }
        LinkState::Missing => {}
    }
    if let Some(dir) = link.parent()
        && !dir.exists()
    {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(LINK_DIR_MODE))?;
        }
    }
    std::os::unix::fs::symlink(target, link)?;
    Ok(true)
}

/// `target`을 가리키는 링크일 때만 지운다. 지웠으면 true. (FR-INS-11)
pub fn remove_link(link: &Path, target: &Path) -> io::Result<bool> {
    if link_state(link, target) != LinkState::Ours {
        return Ok(false);
    }
    fs::remove_file(link)?;
    Ok(true)
}

/// PATH 링크가 가리키는 관리 실행 파일의 루트. (FR-GUI-22)
pub fn root_from_link(link: &Path) -> Option<PathBuf> {
    root_of_exe(link)
}

/// 환경 변수 `PATH`에 `dir`이 있는지. 끝의 `/`는 무시한다. (FR-INS-11)
pub fn path_has_dir(path_env: &OsStr, dir: &Path) -> bool {
    std::env::split_paths(path_env)
        .any(|p| !p.as_os_str().is_empty() && p.components().eq(dir.components()))
}

fn claude_dir_file(root: &Path) -> PathBuf {
    root.join(BIN_DIR).join(CLAUDE_DIR_FILE)
}

/// 설치가 기록한 Claude 폴더. 없거나 비어 있으면 `None`. (FR-INS-12)
pub fn read_claude_dir(root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(claude_dir_file(root)).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

pub fn record_claude_dir(root: &Path, claude_dir: &Path) -> io::Result<()> {
    let file = claude_dir_file(root);
    ensure_private_dir(&root.join(BIN_DIR))?;
    fs::write(&file, format!("{}\n", claude_dir.display()))?;
    set_private_file(&file)
}

/// 기록 값이 `claude_dir`과 같으면 기록 파일을 지운다. 지웠으면 true. (FR-INS-9)
pub fn forget_claude_dir(root: &Path, claude_dir: &Path) -> io::Result<bool> {
    if read_claude_dir(root).as_deref() != Some(claude_dir) {
        return Ok(false);
    }
    fs::remove_file(claude_dir_file(root))?;
    Ok(true)
}

/// 기록 값, 없으면 `~/.claude`. (FR-INS-12)
pub fn default_claude_dir(root: &Path, home: &Path) -> PathBuf {
    read_claude_dir(root).unwrap_or_else(|| home.join(DEFAULT_CLAUDE_DIR))
}

/// GUI 데이터 루트의 기본값: 자기 루트, PATH 링크의 루트, `~/.hippo` 순. (FR-GUI-22)
pub fn default_gui_root(exe: &Path, home: &Path) -> PathBuf {
    root_of_exe(exe)
        .or_else(|| root_from_link(&path_link(home)))
        .unwrap_or_else(|| default_data_root(home))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    NeedInstall,
    NeedUpdate,
    Installed,
}

/// 설치 경로의 실행 파일을 GUI 실행 파일과 비교한다. (FR-GUI-23)
pub fn install_state(exe: &Path, root: &Path) -> InstallState {
    let installed = installed_exe(root);
    if !installed.is_file() {
        return InstallState::NeedInstall;
    }
    let same_file = matches!(
        (fs::canonicalize(exe), fs::canonicalize(&installed)),
        (Ok(a), Ok(b)) if a == b
    );
    if same_file || same_content(exe, &installed).unwrap_or(false) {
        InstallState::Installed
    } else {
        InstallState::NeedUpdate
    }
}

/// 데이터 루트를 통째로 지운다. 링크, `$HOME`, `/`는 거부한다. 없으면 아무것도 하지 않는다. (FR-INS-13)
pub fn remove_root(root: &Path, home: &Path) -> Result<(), String> {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing to delete a symbolic link: {}",
            root.display()
        ));
    }
    let real = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if real.parent().is_none() || real == home {
        return Err(format!("refusing to delete {}", real.display()));
    }
    fs::remove_dir_all(&real).map_err(|e| e.to_string())
}

/// hippo가 띄운 Ollama 서버가 실행 중이면 종료 신호를 보낸다. 보냈으면 true. (FR-EMB-4)
pub fn stop_ollama(root: &Path) -> bool {
    let Some(pid) = fs::read_to_string(root.join(OLLAMA_PID))
        .ok()
        .and_then(|t| t.trim().parse::<i32>().ok())
        .filter(|pid| *pid > 0)
    else {
        return false;
    };
    let Ok(out) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    else {
        return false;
    };
    let comm = String::from_utf8_lossy(&out.stdout);
    let name = Path::new(comm.trim()).file_name();
    if name != Some(OsStr::new(OLLAMA_NAME)) {
        return false;
    }
    // SAFETY: kill(2)은 메모리를 건드리지 않는다. pid는 위에서 확인한 ollama 프로세스이다.
    unsafe { libc::kill(pid, libc::SIGTERM) == 0 }
}

/// 데몬은 끝날 때까지 `hippo.lock`을 쥔다. 다른 프로세스가 쥐고 있으면 true. (FR-DMN-12)
pub fn daemon_lock_held(root: &Path) -> bool {
    let Ok(file) = fs::File::open(crate::config::Paths::new(root.to_path_buf()).lock()) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(fs::TryLockError::WouldBlock) => true,
        Err(fs::TryLockError::Error(_)) => false,
    }
}

/// FR-EMB-1로 띄운 Ollama 서버의 pid를 기록한다. (FR-EMB-4)
pub fn record_ollama_pid(root: &Path, pid: u32) -> io::Result<()> {
    let file = root.join(OLLAMA_PID);
    fs::write(&file, pid.to_string())?;
    set_private_file(&file)
}
