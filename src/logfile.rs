//! 로그 파일 한 줄 쓰기와 순환. (FR-OBS-1)

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fsutil::open_private_append;
use crate::i18n::local_time;

/// 이 크기 이상이면 덧붙이기 전에 `<이름>.1`로 넘긴다.
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// `YYYY-MM-DD HH:MM:SS <message>` 한 줄을 덧붙인다. 실패는 무시한다.
pub fn append_line(path: &Path, message: &str) {
    if fs::metadata(path).is_ok_and(|m| m.len() >= ROTATE_BYTES) {
        let _ = fs::rename(path, rotated(path));
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    if let Ok(mut f) = open_private_append(path) {
        let _ = writeln!(f, "{} {message}", local_time(secs));
    }
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}
