use std::fs;
use std::path::{Path, PathBuf};

use hippo::config::{Config, PatternEntry};
use hippo::config_edit::{
    ConfigEditError, Field, FieldValue, SaveOutcome, changed, fields, needs_reindex, needs_restart,
    save,
};

/// 모든 키를 기본값이 아닌 값으로 둔 설정. (FR-GUI-4)
const ALL_KEYS: &str = r#"
[embedder]
backend = "hash"
model = "m"
dimension = 64
max_input_tokens = 900
url = "http://localhost:1"
keep_alive = "5m"
autostart = false
ollama_command = "/x/ollama"
query_prefix = "q: "
[vector]
backend = "memory"
name = "n"
[chunking]
max_tokens = 500
chars_per_token = 2.5
overlap_chars = 10
[embed_text]
max_tool_input_chars = 7
answer_tools = ["A"]
[inject]
threshold = 0.45
top_k = 9
total_chars = 99
budget_ms = 1
cold_budget_ms = 2
min_prompt_chars = 3
[index]
vector_index_threshold = 5
job_batch = 6
[daemon]
autostart = false
idle_timeout_secs = 0
max_open_projects = 2
[project]
git_timeout_ms = 3
[import]
transcripts_dir = "/t"
[ui]
language = "ko"
[[redact.patterns]]
kind = "k"
pattern = 'P-\d+'
"#;

struct Dir {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn dir() -> Dir {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("root").join("config.toml");
    Dir { _dir: d, path }
}

fn field(section: &'static str, key: &'static str, value: FieldValue) -> Field {
    Field {
        section,
        key,
        value,
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn fields_read_every_key_and_save_round_trips() {
    let all = Config::parse(ALL_KEYS).unwrap();
    let defaults = fields(&Config::default());
    let set = fields(&all);
    assert_eq!(set.len(), 31);
    assert_eq!(
        changed(&defaults, &set).len(),
        31,
        "모든 키가 설정값을 읽음"
    );
    let d = dir();
    assert_eq!(save(&d.path, &set).unwrap(), SaveOutcome::Written);
    assert_eq!(Config::load(&d.path).unwrap(), all);
    assert!(
        set.iter()
            .any(|f| f.value == FieldValue::Float(0.45) && f.key == "threshold"),
        "f32 값은 표기 그대로"
    );
}

#[test]
fn only_changed_key_is_written_and_comments_are_kept() {
    let d = dir();
    let original = "# my note\n[inject]\n# keep this\ntop_k = 5\n\n[ui]\nlanguage = \"ko\"\n";
    write(&d.path, original);
    let change = [field("inject", "threshold", FieldValue::Float(0.45))];
    assert_eq!(save(&d.path, &change).unwrap(), SaveOutcome::Written);
    let text = fs::read_to_string(&d.path).unwrap();
    assert!(
        text.starts_with("# my note\n[inject]\n# keep this\ntop_k = 5\n"),
        "{text}"
    );
    assert!(
        text.contains("threshold = 0.45") && text.contains("language = \"ko\""),
        "{text}"
    );
    let c = Config::load(&d.path).unwrap();
    assert_eq!((c.inject.threshold, c.inject.top_k), (0.45, 5));
}

#[test]
fn default_value_removes_key_and_empty_table() {
    let d = dir();
    write(&d.path, "[inject]\ntop_k = 5\n\n[ui]\nlanguage = \"ko\"\n");
    let change = [field("inject", "top_k", FieldValue::Int(3))];
    assert_eq!(save(&d.path, &change).unwrap(), SaveOutcome::Written);
    let text = fs::read_to_string(&d.path).unwrap();
    assert!(
        !text.contains("inject") && text.contains("language"),
        "{text}"
    );
}

#[test]
fn list_values_are_saved_and_removed_at_default() {
    let d = dir();
    let tools = field(
        "embed_text",
        "answer_tools",
        FieldValue::List(vec!["A".into()]),
    );
    let patterns = field(
        "redact",
        "patterns",
        FieldValue::Patterns(vec![PatternEntry {
            kind: "k".into(),
            pattern: r"P-\d+".into(),
        }]),
    );
    save(&d.path, &[tools, patterns]).unwrap();
    let c = Config::load(&d.path).unwrap();
    assert_eq!(c.embed_text.answer_tools, vec!["A".to_string()]);
    assert_eq!(c.redact.patterns.len(), 1);
    let defaults = fields(&Config::default());
    let back: Vec<Field> = defaults
        .into_iter()
        .filter(|f| {
            matches!(
                (f.section, f.key),
                ("embed_text", "answer_tools") | ("redact", "patterns")
            )
        })
        .collect();
    save(&d.path, &back).unwrap();
    assert_eq!(fs::read_to_string(&d.path).unwrap().trim(), "");
}

#[test]
fn invalid_result_is_rejected_and_file_is_untouched() {
    let d = dir();
    write(&d.path, "[inject]\nbudget_ms = 800\n");
    let change = [field("inject", "cold_budget_ms", FieldValue::Int(100))];
    assert!(matches!(
        save(&d.path, &change),
        Err(ConfigEditError::Invalid(_))
    ));
    assert_eq!(
        fs::read_to_string(&d.path).unwrap(),
        "[inject]\nbudget_ms = 800\n"
    );
    let remote = [field(
        "embedder",
        "url",
        FieldValue::Text("http://example.com".into()),
    )];
    assert!(matches!(
        save(&d.path, &remote),
        Err(ConfigEditError::Invalid(_))
    ));
}

#[test]
fn unparsable_file_is_not_overwritten() {
    let d = dir();
    write(&d.path, "[inject\n");
    let change = [field("inject", "top_k", FieldValue::Int(4))];
    assert!(save(&d.path, &change).is_err());
    assert_eq!(fs::read_to_string(&d.path).unwrap(), "[inject\n");
}

#[test]
fn same_result_or_empty_result_without_file_writes_nothing() {
    let d = dir();
    let change = [field("inject", "top_k", FieldValue::Int(3))];
    assert_eq!(save(&d.path, &change).unwrap(), SaveOutcome::Unchanged);
    assert!(!d.path.exists(), "기본값만 있으면 파일을 만들지 않음");
    write(&d.path, "[inject]\ntop_k = 5\n");
    let same = [field("inject", "top_k", FieldValue::Int(5))];
    assert_eq!(save(&d.path, &same).unwrap(), SaveOutcome::Unchanged);
}

#[cfg(unix)]
#[test]
fn new_file_is_owner_only_and_symlink_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir();
    save(&d.path, &[field("inject", "top_k", FieldValue::Int(4))]).unwrap();
    let mode = fs::metadata(&d.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let other = d.path.with_file_name("other.toml");
    fs::write(&other, "").unwrap();
    fs::remove_file(&d.path).unwrap();
    std::os::unix::fs::symlink(&other, &d.path).unwrap();
    let r = save(&d.path, &[field("inject", "top_k", FieldValue::Int(4))]);
    assert!(matches!(r, Err(ConfigEditError::Symlink(_))), "{r:?}");
    assert_eq!(fs::read_to_string(&other).unwrap(), "");
}

#[test]
fn restart_and_reindex_needs_follow_changed_keys() {
    let one = |s, k, v| vec![field(s, k, v)];
    let lang = one("ui", "language", FieldValue::Text("ko".into()));
    assert_eq!((needs_restart(&lang), needs_reindex(&lang)), (false, false));
    let threshold = one("inject", "threshold", FieldValue::Float(0.5));
    assert_eq!(
        (needs_restart(&threshold), needs_reindex(&threshold)),
        (true, false)
    );
    let url = one(
        "embedder",
        "url",
        FieldValue::Text("http://localhost:2".into()),
    );
    assert_eq!((needs_restart(&url), needs_reindex(&url)), (true, false));
    let prefix = one("embedder", "query_prefix", FieldValue::Text("q: ".into()));
    assert_eq!(
        (needs_restart(&prefix), needs_reindex(&prefix)),
        (true, false)
    );
    let min = one("inject", "min_prompt_chars", FieldValue::Int(3));
    assert_eq!((needs_restart(&min), needs_reindex(&min)), (true, false));
    for (s, k) in [
        ("embedder", "model"),
        ("embedder", "max_input_tokens"),
        ("vector", "name"),
        ("chunking", "overlap_chars"),
        ("embed_text", "answer_tools"),
    ] {
        let c = one(s, k, FieldValue::Int(1));
        assert!(needs_restart(&c) && needs_reindex(&c), "{s}.{k}");
    }
}
