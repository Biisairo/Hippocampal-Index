//! 가져오기 후보 세션 선택과 미리 보기. (SRS 3.2.11, FR-IMP-2, FR-IMP-8, FR-IMP-9)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::i18n::{Lang, local_time};
use crate::ingest::session_start_cwd;
use crate::project::{Location, normalize};
use crate::search::escape_control;
use crate::tr;
use crate::transcript::ExtractorState;

const TRANSCRIPT_EXT: &str = "jsonl";
const PROMPT_PREVIEW_CHARS: usize = 80;
const TASK_NOTIFICATION: &str = "<task-notification>";
const SESSION_ID_SHORT: usize = 8;
const KIB: u64 = 1024;
const SDK_ENTRYPOINT_PREFIX: &str = "sdk-";

/// 후보 세션과 대상 프로젝트의 관계. (FR-IMP-8)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    #[default]
    New,
    LiveOnly,
    Imported,
    OtherProject,
}

impl ImportStatus {
    pub fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::New, Lang::En) => "new",
            (Self::LiveOnly, Lang::En) => "live-only",
            (Self::Imported, Lang::En) => "imported",
            (Self::OtherProject, Lang::En) => "other project",
            (Self::New, Lang::Ko) => "새 세션",
            (Self::LiveOnly, Lang::Ko) => "실시간만",
            (Self::Imported, Lang::Ko) => "가져옴",
            (Self::OtherProject, Lang::Ko) => "다른 프로젝트",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub session_id: String,
    pub transcript_path: PathBuf,
    /// 첫 `timestamp`. 없으면 비어 있다.
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub start_cwd: String,
    #[serde(default)]
    pub size: u64,
    /// 마스킹한 뒤 공백을 줄이고 앞 80자만 남긴 첫 프롬프트.
    #[serde(default)]
    pub first_prompt: String,
    #[serde(default)]
    pub status: ImportStatus,
    /// 첫 최상위 `entrypoint`. 없으면 비어 있다. (FR-IMP-8)
    #[serde(default)]
    pub entrypoint: String,
}

impl Candidate {
    /// 실행 방식이 `sdk-`로 시작하는 세션. (FR-GUI-7)
    pub fn is_sdk(&self) -> bool {
        self.entrypoint.starts_with(SDK_ENTRYPOINT_PREFIX)
    }
}

/// `pin`은 세션 고정값, `owners`는 대상 프로젝트로 볼 프로젝트들, `state`는 그 저장소의 세션 상태이다. (FR-IMP-8, FR-IMP-9)
pub fn import_status(
    pin: Option<&str>,
    owners: &[&str],
    state: Option<&ExtractorState>,
) -> ImportStatus {
    if pin.is_some_and(|p| !owners.contains(&p)) {
        return ImportStatus::OtherProject;
    }
    match state {
        None => ImportStatus::New,
        Some(s) if s.start_offset > 0 => ImportStatus::LiveOnly,
        Some(_) => ImportStatus::Imported,
    }
}

/// `dir` 바로 아래 디렉터리들의 `*.jsonl` 중 세션 시작 경로가 `from` 아래이거나,
/// 존재하는 경로이고 프로젝트 위치가 `from_location`인 세션. 첫 `timestamp` 순(같으면 경로 순)이다.
pub fn candidates(
    dir: &Path,
    from: &Path,
    from_location: &str,
    locate: &dyn Fn(&Path) -> Location,
    redact: &dyn Fn(&str) -> String,
) -> Vec<Candidate> {
    let from = normalize(from);
    let mut locations: HashMap<PathBuf, String> = HashMap::new();
    let mut found: Vec<(String, Candidate)> = transcript_files(dir)
        .into_iter()
        .filter_map(|path| {
            let bytes = fs::read(&path).ok()?;
            let start_cwd = session_start_cwd(&bytes)?;
            let start = normalize(Path::new(&start_cwd));
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
            let started_at = first_timestamp(&bytes);
            let first_prompt = first_prompt(&bytes)
                .map(|text| prompt_preview(&redact(&text)))
                .unwrap_or_default();
            Some((
                started_at.clone().unwrap_or_else(|| modified(&path)),
                Candidate {
                    session_id,
                    transcript_path: path,
                    started_at: started_at.unwrap_or_default(),
                    start_cwd,
                    size: bytes.len() as u64,
                    first_prompt,
                    status: ImportStatus::New,
                    entrypoint: entrypoint(&bytes).unwrap_or_default(),
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
fn entrypoint(bytes: &[u8]) -> Option<String> {
    bytes.split(|b| *b == b'\n').find_map(|raw| {
        serde_json::from_slice::<Value>(raw)
            .ok()?
            .get("entrypoint")?
            .as_str()
            .map(str::to_string)
    })
}

fn modified(path: &Path) -> String {
    let secs = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("~{secs:020}")
}

/// 메타 라인, `tool_result`, 완료 알림이 아닌 첫 사용자 프롬프트. `text` 블록은 줄바꿈으로 잇는다.
fn first_prompt(bytes: &[u8]) -> Option<String> {
    bytes.split(|b| *b == b'\n').find_map(|raw| {
        let v: Value = serde_json::from_slice(raw).ok()?;
        if v.get("type")?.as_str()? != "user"
            || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            return None;
        }
        let text = match v.get("message")?.get("content")? {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => {
                if blocks
                    .iter()
                    .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                {
                    return None;
                }
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => return None,
        };
        (!text.trim_start().starts_with(TASK_NOTIFICATION)).then_some(text)
    })
}

fn prompt_preview(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(PROMPT_PREVIEW_CHARS)
        .collect()
}

/// `hippo import --dry-run` 출력. (FR-IMP-9)
pub fn render_preview(summary: &str, sessions: &[Candidate], lang: Lang) -> String {
    let mut out = summary.to_string();
    let mut counts = [0usize; 4];
    for (n, c) in sessions.iter().enumerate() {
        counts[c.status as usize] += 1;
        let short: String = c.session_id.chars().take(SESSION_ID_SHORT).collect();
        out.push_str(&format!(
            "[{}] {}  {short}  {}  {}  {}  {}\n",
            n + 1,
            started_label(&c.started_at),
            c.status.label(lang),
            size_label(c.size),
            c.start_cwd,
            c.first_prompt
        ));
    }
    out.push_str(&tr!(
        lang,
        "new {}, live-only {}, imported {}, other project {}\n",
        "새 세션 {}, 실시간만 {}, 가져옴 {}, 다른 프로젝트 {}\n",
        counts[ImportStatus::New as usize],
        counts[ImportStatus::LiveOnly as usize],
        counts[ImportStatus::Imported as usize],
        counts[ImportStatus::OtherProject as usize]
    ));
    escape_control(&out)
}

/// UTC `YYYY-MM-DDTHH:MM:SS…`를 로컬 시간대로 바꾼다. 해석할 수 없으면 그대로, 비어 있으면 `-`이다.
pub fn started_label(ts: &str) -> String {
    if ts.is_empty() {
        return "-".into();
    }
    utc_secs(ts).map_or_else(|| ts.to_string(), local_time)
}

fn utc_secs(ts: &str) -> Option<i64> {
    let num = |range: std::ops::Range<usize>| ts.get(range)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // 그레고리력 날짜를 1970-01-01부터의 일수로 바꾼다 (Howard Hinnant의 days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + s)
}

pub fn size_label(bytes: u64) -> String {
    if bytes < KIB {
        format!("{bytes}B")
    } else if bytes < KIB * KIB {
        format!("{:.1}KB", bytes as f64 / KIB as f64)
    } else {
        format!("{:.1}MB", bytes as f64 / (KIB * KIB) as f64)
    }
}
