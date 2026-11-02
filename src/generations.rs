//! 벡터 저장소 세대 디렉터리와 ACTIVE 포인터. (FR-SWP-5, FR-SWP-9)

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fsutil::ensure_private_dir;

const ACTIVE: &str = "ACTIVE";
const ACTIVE_TMP: &str = "ACTIVE.tmp";
const PREFIX: char = 'g';

/// `g` 뒤에 숫자만 오는 이름만 세대로 인정한다. 포인터가 base 밖을 가리키지 못하게 한다.
fn is_generation_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

fn read_active(base: &Path) -> Option<String> {
    let name = fs::read_to_string(base.join(ACTIVE))
        .ok()?
        .trim()
        .to_string();
    is_generation_name(&name).then_some(name)
}

/// ACTIVE가 가리키는 세대 디렉터리. 없으면 첫 세대를 만들어 가리키게 한다.
pub fn active_dir(base: &Path) -> io::Result<PathBuf> {
    ensure_private_dir(base)?;
    if let Some(name) = read_active(base) {
        let dir = base.join(name);
        if dir.is_dir() {
            return Ok(dir);
        }
    }
    let generation = new_generation(base)?;
    activate(base, &generation)?;
    Ok(generation)
}

/// 아직 활성화하지 않은 새 세대 디렉터리를 만든다.
pub fn new_generation(base: &Path) -> io::Result<PathBuf> {
    ensure_private_dir(base)?;
    let mut stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    loop {
        let dir = base.join(format!("{PREFIX}{stamp:024}"));
        if !dir.exists() {
            ensure_private_dir(&dir)?;
            return Ok(dir);
        }
        stamp += 1;
    }
}

/// ACTIVE를 원자적으로 바꾼다.
pub fn activate(base: &Path, generation: &Path) -> io::Result<()> {
    let name = generation
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| is_generation_name(n))
        .filter(|_| generation.parent() == Some(base) && generation.is_dir())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a generation of {}", base.display()),
            )
        })?;
    let tmp = base.join(ACTIVE_TMP);
    fs::write(&tmp, name)?;
    fs::rename(&tmp, base.join(ACTIVE))
}

/// ACTIVE가 가리키지 않는 세대 디렉터리를 지우고, 지운 경로를 돌려준다.
pub fn cleanup_inactive(base: &Path) -> io::Result<Vec<PathBuf>> {
    let Some(active) = read_active(base) else {
        return Ok(Vec::new());
    };
    let mut removed: Vec<PathBuf> = fs::read_dir(base)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| is_generation_name(n) && n != active)
        })
        .collect();
    removed.sort();
    for dir in &removed {
        fs::remove_dir_all(dir)?;
    }
    Ok(removed)
}
