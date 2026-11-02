//! 데몬이 꺼져 있을 때 수집 요청을 보관하는 스풀. (FR-CAP-5, FR-CAP-6)

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fsutil::{ensure_private_dir, open_private_append};
use crate::ingest::IngestRequest;

const PENDING: &str = "pending.jsonl";
const DRAINING_PREFIX: &str = "draining-";

/// 요청 한 건을 `pending.jsonl`에 한 줄로 추가한다. 파일 권한은 0600.
pub fn append(dir: &Path, req: &IngestRequest) -> io::Result<()> {
    ensure_private_dir(dir)?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    open_private_append(&dir.join(PENDING))?.write_all(line.as_bytes())
}

/// `pending.jsonl`의 이름을 바꿔 가져온다. 이전에 처리하지 못한 파일이 있으면 먼저 돌려준다.
/// 각 항목은 (처리 후 지울 파일, 기록 순서대로의 요청)이다. 해석할 수 없는 줄은 건너뛴다.
pub fn take_all(dir: &Path) -> io::Result<Vec<(PathBuf, Vec<IngestRequest>)>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = draining_files(dir)?;
    let pending = dir.join(PENDING);
    if pending.exists() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let target = dir.join(format!("{DRAINING_PREFIX}{nanos:024}.jsonl"));
        fs::rename(&pending, &target)?;
        files.push(target);
    }
    files
        .into_iter()
        .map(|file| {
            let requests = parse_lines(&fs::read_to_string(&file)?);
            Ok((file, requests))
        })
        .collect()
}

pub fn finish(file: &Path) -> io::Result<()> {
    fs::remove_file(file)
}

/// 아직 처리되지 않은 요청 수.
pub fn pending_count(dir: &Path) -> u64 {
    let mut files = draining_files(dir).unwrap_or_default();
    files.push(dir.join(PENDING));
    files
        .iter()
        .filter_map(|f| fs::read_to_string(f).ok())
        .map(|s| parse_lines(&s).len() as u64)
        .sum()
}

fn draining_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(DRAINING_PREFIX))
        })
        .collect();
    files.sort();
    Ok(files)
}

fn parse_lines(text: &str) -> Vec<IngestRequest> {
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}
