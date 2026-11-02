use std::path::PathBuf;

use hippo::config::{Config, ConfigError, Paths};

#[test]
fn defaults_match_srs() {
    let c = Config::default();
    assert_eq!(c.embedder.backend, "ollama");
    assert_eq!(c.embedder.model, "qwen3-embedding:0.6b");
    assert_eq!(c.embedder.dimension, 1024);
    assert_eq!(c.embedder.url, "http://127.0.0.1:11434");
    assert_eq!(c.vector.backend, "lancedb");
    assert_eq!(c.chunking.max_tokens, 8_000);
    assert_eq!(c.chunking.chars_per_token, 1.0);
    assert_eq!(c.embed_text.max_tool_input_chars, 500);
    assert_eq!(
        (
            c.inject.threshold,
            c.inject.top_k,
            c.inject.per_item_chars,
            c.inject.total_chars,
            c.inject.budget_ms
        ),
        (0.40, 3, 800, 4_000, 800)
    );
    assert_eq!(c.index.vector_index_threshold, 100_000);
    assert!(c.redact.patterns.is_empty());
    assert!(c.daemon.autostart);
    assert_eq!(c.daemon.idle_timeout_secs, 1_800);
}

#[test]
fn daemon_section_override() {
    let c = Config::parse("[daemon]\nautostart = false\nidle_timeout_secs = 0\n").unwrap();
    assert!(!c.daemon.autostart);
    assert_eq!(c.daemon.idle_timeout_secs, 0);
}

#[test]
fn missing_file_uses_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        Config::load(&dir.path().join("none.toml")).unwrap(),
        Config::default()
    );
}

#[test]
fn partial_override_keeps_other_defaults() {
    let c = Config::parse("[inject]\nthreshold = 0.7\n\n[[redact.patterns]]\nkind = \"internal\"\npattern = 'HCI-\\d+'\n").unwrap();
    assert_eq!(c.inject.threshold, 0.7);
    assert_eq!(c.inject.top_k, 3);
    assert_eq!(c.embedder, Config::default().embedder);
    let patterns = c.redact_patterns();
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].kind, "internal");
}

#[test]
fn rejects_unknown_keys_and_bad_toml() {
    assert!(matches!(
        Config::parse("[inject]\nthreshhold = 0.7\n"),
        Err(ConfigError::Parse(_))
    ));
    assert!(matches!(
        Config::parse("[inject\n"),
        Err(ConfigError::Parse(_))
    ));
}

#[test]
fn rejects_non_local_embedder_url() {
    for url in [
        "https://api.example.com",
        "http://10.0.0.5:11434",
        "http://localhost.evil.com:11434",
        "http://127.0.0.1:11434@evil.com",
        "http://user@localhost:11434",
        "http://localhost:11434/?next=http://evil.com",
        "http://localhost:11434/#frag",
        "ftp://localhost:11434",
        "localhost:11434",
    ] {
        let text = format!("[embedder]\nurl = \"{url}\"\n");
        assert!(
            matches!(Config::parse(&text), Err(ConfigError::NonLocalUrl(_))),
            "{url}"
        );
    }
    for url in [
        "http://localhost:11434",
        "http://127.0.0.1:11434",
        "http://[::1]:11434",
    ] {
        assert!(
            Config::parse(&format!("[embedder]\nurl = \"{url}\"\n")).is_ok(),
            "{url}"
        );
    }
}

#[test]
fn load_reads_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[vector]\nname = \"v2\"\n").unwrap();
    assert_eq!(Config::load(&path).unwrap().vector.name, "v2");
}

#[test]
fn paths_layout() {
    let p = Paths::new(PathBuf::from("/r"));
    assert_eq!(p.registry(), PathBuf::from("/r/registry.sqlite"));
    assert_eq!(p.projects(), PathBuf::from("/r/projects"));
    assert_eq!(p.project_dir("abc"), PathBuf::from("/r/projects/abc"));
    assert_eq!(
        p.project_records("abc"),
        PathBuf::from("/r/projects/abc/records.sqlite")
    );
    assert_eq!(
        p.project_vectors("abc", "default"),
        PathBuf::from("/r/projects/abc/vectors/default")
    );
    assert_eq!(p.spool(), PathBuf::from("/r/spool"));
    assert_eq!(p.logs(), PathBuf::from("/r/logs"));
    assert_eq!(p.config(), PathBuf::from("/r/config.toml"));
    assert_eq!(p.socket(), PathBuf::from("/r/hippo.sock"));
    assert_eq!(p.lock(), PathBuf::from("/r/hippo.lock"));
    assert!(Paths::default_root().unwrap().ends_with(".hippo"));
}

#[test]
fn project_and_daemon_defaults() {
    let c = Config::default();
    assert_eq!(c.daemon.max_open_projects, 8);
    assert_eq!(c.project.git_timeout_ms, 1_000);
    let c =
        Config::parse("[daemon]\nmax_open_projects = 2\n[project]\ngit_timeout_ms = 50\n").unwrap();
    assert_eq!(
        (c.daemon.max_open_projects, c.project.git_timeout_ms),
        (2, 50)
    );
}
