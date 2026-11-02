//! 가져오기 후보 세션 선택. (SRS 3.2.11, FR-IMP-2)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ingest::session_start_cwd;
use crate::project::{Location, normalize};

const TRANSCRIPT_EXT: &str = "jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub session_id: String,
    pub transcript_path: PathBuf,
}

/// `dir` 바로 아래 디렉터리들의 `*.jsonl` 중 세션 시작 경로가 `from` 아래이거나,
/// 존재하는 경로이고 프로젝트 위치가 `from_location`인 세션. 첫 `timestamp` 순(같으면 경로 순)이다.
pub fn candidates(
    dir: &Path,
    from: &Path,
    from_location: &str,
    locate: &dyn Fn(&Path) -> Location,
) -> Vec<Candidate> {
    let from = normalize(from);
    let mut locations: HashMap<PathBuf, String> = HashMap::new();
    let mut found: Vec<(String, Candidate)> = transcript_files(dir)
        .into_iter()
        .filter_map(|path| {
            let bytes = fs::read(&path).ok()?;
            let start = normalize(Path::new(&session_start_cwd(&bytes)?));
            let selected = start.starts_with(&from)
                || (start.exists()
                    && locations
                        .entry(start.clone())
                        .or_insert_with(|| locate(&start).path)
                        == from_location);
            if !selected {
                return None;
            }
            let session_id = path.file_stem()?.to_string_lossy().into_owned();
            Some((
                first_timestamp(&bytes).unwrap_or_else(|| modified(&path)),
                Candidate {
                    session_id,
                    transcript_path: path,
                },
            ))
        })
        .collect();
    found.sort_by(|a, b| (&a.0, &a.1.transcript_path).cmp(&(&b.0, &b.1.transcript_path)));
    found.into_iter().map(|(_, c)| c).collect()
}

fn transcript_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| fs::read_dir(e.path()).ok())
        .flat_map(|inner| inner.filter_map(Result::ok))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == TRANSCRIPT_EXT))
        .collect()
}

fn first_timestamp(bytes: &[u8]) -> Option<String> {
    bytes.split(|b| *b == b'\n').find_map(|raw| {
        serde_json::from_slice::<serde_json::Value>(raw)
            .ok()?
            .get("timestamp")?
            .as_str()
            .map(str::to_string)
    })
}

/// timestamp가 없는 transcript의 정렬 키. `~`는 숫자보다 뒤이므로 timestamp가 있는 것 뒤에 수정 시각 순으로 온다.
fn modified(path: &Path) -> String {
    let secs = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("~{secs:020}")
}
