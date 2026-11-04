//! 경로 입력란의 폴더 찾아보기. macOS 폴더 선택 창을 별도 프로세스(`osascript`)로 띄운다. (FR-GUI-24)

use std::path::{Path, PathBuf};
use std::process::Command;

const HOME_PREFIX: &str = "~/";
const HOME: &str = "~";
const ROOT: &str = "/";

/// 선택 창의 처음 위치: 입력란의 값이 가리키는 폴더가 있으면 그 폴더, 없으면 홈.
pub fn start_folder(value: &str, cwd: &Path, home: &Path) -> PathBuf {
    let value = value.trim();
    let path = if value == HOME {
        home.to_path_buf()
    } else if let Some(rest) = value.strip_prefix(HOME_PREFIX) {
        home.join(rest)
    } else if value.is_empty() {
        return home.to_path_buf();
    } else {
        cwd.join(value)
    };
    if path.is_dir() {
        path
    } else {
        home.to_path_buf()
    }
}

/// AppleScript 문자열 안에 넣을 수 있게 역슬래시와 따옴표를 이스케이프한다.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn folder_script(prompt: &str, start: &Path) -> String {
    format!(
        "POSIX path of (choose folder with prompt {} default location (POSIX file {}) with invisibles)",
        quote(prompt),
        quote(&start.to_string_lossy())
    )
}

/// 고른 폴더의 절대 경로. 끝의 `/`는 빼고, 루트는 `/`. 취소나 실패면 `None`.
pub fn parse_folder(success: bool, stdout: &str) -> Option<PathBuf> {
    if !success {
        return None;
    }
    let line = stdout.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return None;
    }
    let trimmed = line.trim_end_matches('/');
    Some(PathBuf::from(if trimmed.is_empty() {
        ROOT
    } else {
        trimmed
    }))
}

/// 선택 창을 띄우고 고를 때까지 기다린다. 화면을 그리는 스레드 밖에서 부른다.
pub fn pick_folder(prompt: &str, start: &Path) -> Option<PathBuf> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(folder_script(prompt, start))
        .output()
        .ok()?;
    parse_folder(out.status.success(), &String::from_utf8_lossy(&out.stdout))
}
