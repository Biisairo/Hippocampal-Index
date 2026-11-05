//! GUI의 설정 편집: 키 목록, 바꾼 키만 고치는 저장, 재시작·재색인 필요 판정. (FR-GUI-4~6)

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, TableLike, value};

use crate::config::{Config, PatternEntry};
use crate::fsutil::ensure_private_dir;

#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;
const LANGUAGE: (&str, &str) = ("ui", "language");
const THEME: (&str, &str) = ("ui", "theme");
const START_TAB: (&str, &str) = ("ui", "start_tab");
/// hook이 매번 읽는 키. 바꿔도 데몬을 다시 시작할 필요가 없다. (FR-GUI-6)
const NO_RESTART: &[(&str, &str)] = &[LANGUAGE, THEME, START_TAB, INJECT_ENABLED, CAPTURE_ENABLED];
pub const INJECT_ENABLED: (&str, &str) = ("inject", "enabled");
pub const CAPTURE_ENABLED: (&str, &str) = ("capture", "enabled");
const REINDEX_EMBEDDER_KEYS: &[&str] = &["backend", "model", "dimension", "max_input_tokens"];
const REINDEX_SECTIONS: &[&str] = &["vector", "chunking", "embed_text"];

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<String>),
    Patterns(Vec<PatternEntry>),
}

/// 설정 키 하나와 그 값.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub section: &'static str,
    pub key: &'static str,
    pub value: FieldValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    Written,
    Unchanged,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigEditError {
    #[error("config file access failed: {0}")]
    Io(#[from] io::Error),
    #[error("config file is not valid TOML: {0}")]
    Syntax(String),
    #[error("{0}")]
    Invalid(String),
    #[error("refusing to write through a symbolic link: {0}")]
    Symlink(PathBuf),
}

fn int(n: impl TryInto<i64>) -> FieldValue {
    FieldValue::Int(n.try_into().unwrap_or(i64::MAX))
}

/// f32를 표기 그대로의 f64로 바꾼다(0.45f32 → 0.45).
fn float32(n: f32) -> FieldValue {
    FieldValue::Float(n.to_string().parse().unwrap_or(f64::from(n)))
}

fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.to_string())
}

/// `Config`의 모든 키를 섹션 순서대로 돌려준다. (FR-GUI-4)
pub fn fields(c: &Config) -> Vec<Field> {
    let e = &c.embedder;
    let lang = match c.ui.language {
        crate::i18n::Lang::En => "en",
        crate::i18n::Lang::Ko => "ko",
    };
    let list: Vec<(&'static str, &'static str, FieldValue)> = vec![
        ("embedder", "backend", text(&e.backend)),
        ("embedder", "model", text(&e.model)),
        ("embedder", "dimension", int(e.dimension)),
        ("embedder", "max_input_tokens", int(e.max_input_tokens)),
        ("embedder", "url", text(&e.url)),
        ("embedder", "keep_alive", text(&e.keep_alive)),
        ("embedder", "autostart", FieldValue::Bool(e.autostart)),
        ("embedder", "ollama_command", text(&e.ollama_command)),
        ("embedder", "query_prefix", text(&e.query_prefix)),
        ("vector", "backend", text(&c.vector.backend)),
        ("vector", "name", text(&c.vector.name)),
        ("chunking", "max_tokens", int(c.chunking.max_tokens)),
        (
            "chunking",
            "chars_per_token",
            FieldValue::Float(c.chunking.chars_per_token),
        ),
        ("chunking", "overlap_chars", int(c.chunking.overlap_chars)),
        (
            "embed_text",
            "max_tool_input_chars",
            int(c.embed_text.max_tool_input_chars),
        ),
        (
            "embed_text",
            "answer_tools",
            FieldValue::List(c.embed_text.answer_tools.clone()),
        ),
        ("inject", "threshold", float32(c.inject.threshold)),
        ("inject", "top_k", int(c.inject.top_k)),
        ("inject", "total_chars", int(c.inject.total_chars)),
        ("inject", "budget_ms", int(c.inject.budget_ms)),
        ("inject", "cold_budget_ms", int(c.inject.cold_budget_ms)),
        ("inject", "min_prompt_chars", int(c.inject.min_prompt_chars)),
        ("inject", "enabled", FieldValue::Bool(c.inject.enabled)),
        ("capture", "enabled", FieldValue::Bool(c.capture.enabled)),
        (
            "index",
            "vector_index_threshold",
            int(c.index.vector_index_threshold),
        ),
        ("index", "job_batch", int(c.index.job_batch)),
        ("index", "optimize_every", int(c.index.optimize_every)),
        ("daemon", "autostart", FieldValue::Bool(c.daemon.autostart)),
        (
            "daemon",
            "idle_timeout_secs",
            int(c.daemon.idle_timeout_secs),
        ),
        (
            "daemon",
            "max_open_projects",
            int(c.daemon.max_open_projects),
        ),
        (
            "daemon",
            "prevent_sleep",
            FieldValue::Bool(c.daemon.prevent_sleep),
        ),
        ("project", "git_timeout_ms", int(c.project.git_timeout_ms)),
        (
            "import",
            "transcripts_dir",
            text(&c.import.transcripts_dir.to_string_lossy()),
        ),
        ("ui", "language", text(lang)),
        ("ui", "theme", text(c.ui.theme.as_str())),
        ("ui", "start_tab", text(c.ui.start_tab.as_str())),
        (
            "redact",
            "patterns",
            FieldValue::Patterns(c.redact.patterns.clone()),
        ),
    ];
    list.into_iter()
        .map(|(section, key, value)| Field {
            section,
            key,
            value,
        })
        .collect()
}

/// `edited` 중 `loaded`의 같은 키와 값이 다른 것.
pub fn changed(loaded: &[Field], edited: &[Field]) -> Vec<Field> {
    edited
        .iter()
        .filter(|e| {
            !loaded
                .iter()
                .any(|l| l.section == e.section && l.key == e.key && l.value == e.value)
        })
        .cloned()
        .collect()
}

/// 언어, 테마, 일시정지 키 말고 다른 키가 바뀌었으면 데몬 재시작이 필요하다. (FR-GUI-6)
pub fn needs_restart(changes: &[Field]) -> bool {
    changes
        .iter()
        .any(|f| !NO_RESTART.contains(&(f.section, f.key)))
}

/// 재색인 필요 키가 바뀌었는지. (SRS 1.3)
pub fn needs_reindex(changes: &[Field]) -> bool {
    changes.iter().any(|f| {
        REINDEX_SECTIONS.contains(&f.section)
            || (f.section == "embedder" && REINDEX_EMBEDDER_KEYS.contains(&f.key))
    })
}

/// 파일에서 `changes`의 키만 고쳐 저장한다. 기본값과 같은 키는 지운다. (FR-GUI-5)
pub fn save(path: &Path, changes: &[Field]) -> Result<SaveOutcome, ConfigEditError> {
    reject_symlinks(path)?;
    let original = if path.exists() {
        Some(fs::read_to_string(path)?)
    } else {
        None
    };
    let mut doc: DocumentMut = original
        .as_deref()
        .unwrap_or_default()
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Syntax(e.to_string()))?;
    let defaults = fields(&Config::default());
    for change in changes {
        let is_default = defaults
            .iter()
            .any(|d| d.section == change.section && d.key == change.key && d.value == change.value);
        if is_default {
            remove_key(&mut doc, change.section, change.key);
        } else {
            set_key(&mut doc, change);
        }
    }
    let result = doc.to_string();
    Config::parse(&result).map_err(|e| ConfigEditError::Invalid(e.to_string()))?;
    let unchanged = match &original {
        Some(text) => *text == result,
        None => result.trim().is_empty(),
    };
    if unchanged {
        return Ok(SaveOutcome::Unchanged);
    }
    write_private(path, &result)?;
    Ok(SaveOutcome::Written)
}

fn remove_key(doc: &mut DocumentMut, section: &str, key: &str) {
    let Some(table) = doc.get_mut(section).and_then(Item::as_table_like_mut) else {
        return;
    };
    table.remove(key);
    if table.is_empty() {
        doc.remove(section);
    }
}

fn set_key(doc: &mut DocumentMut, field: &Field) {
    let item = to_item(&field.value);
    let section = doc.entry(field.section).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(matches!(field.value, FieldValue::Patterns(_)));
        Item::Table(table)
    });
    if let Some(table) = section.as_table_like_mut() {
        TableLike::insert(table, field.key, item);
    }
}

fn to_item(v: &FieldValue) -> Item {
    match v {
        FieldValue::Bool(b) => value(*b),
        FieldValue::Int(n) => value(*n),
        FieldValue::Float(x) => value(*x),
        FieldValue::Text(s) => value(s.as_str()),
        FieldValue::List(items) => {
            let mut array = Array::new();
            for s in items {
                array.push(s.as_str());
            }
            value(array)
        }
        FieldValue::Patterns(patterns) => {
            let mut tables = ArrayOfTables::new();
            for p in patterns {
                let mut table = Table::new();
                table.insert("kind", value(p.kind.as_str()));
                table.insert("pattern", value(p.pattern.as_str()));
                tables.push(table);
            }
            Item::ArrayOfTables(tables)
        }
    }
}

fn reject_symlinks(path: &Path) -> Result<(), ConfigEditError> {
    for candidate in [Some(path), path.parent()].into_iter().flatten() {
        if fs::symlink_metadata(candidate).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ConfigEditError::Symlink(candidate.to_path_buf()));
        }
    }
    Ok(())
}

/// 같은 디렉터리의 임시 파일(0600)에 쓰고 이름을 바꿔 원자적으로 교체한다.
fn write_private(path: &Path, text: &str) -> Result<(), ConfigEditError> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "config path has no parent"))?;
    ensure_private_dir(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".config.toml.")
        .tempfile_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
    }
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
