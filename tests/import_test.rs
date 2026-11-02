use std::fs;
use std::path::{Path, PathBuf};

use hippo::import::{Candidate, candidates};
use hippo::project::{Location, LocationKind};
use serde_json::json;

fn transcript(cwd: &Path, ts: &str) -> String {
    let lines = [
        json!({"type": "queue-operation", "timestamp": ts}),
        json!({"type": "user", "cwd": cwd.to_string_lossy(), "timestamp": ts, "promptId": "p"}),
    ];
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn put(root: &Path, slug: &str, session: &str, body: &str) -> PathBuf {
    let dir = root.join(slug);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{session}.jsonl"));
    fs::write(&path, body).unwrap();
    path
}

/// `repo` 아래와 `wt` 아래는 같은 git 저장소(`repo/.git`), 그 밖은 경로 자체가 위치이다.
fn fake_locate(base: PathBuf) -> impl Fn(&Path) -> Location {
    move |p: &Path| {
        let git = [base.join("repo"), base.join("wt")]
            .iter()
            .any(|root| p.starts_with(root));
        Location {
            path: if git {
                base.join("repo/.git").to_string_lossy().into_owned()
            } else {
                p.to_string_lossy().into_owned()
            },
            kind: if git {
                LocationKind::Git
            } else {
                LocationKind::Dir
            },
            toplevel: None,
            root_commits: Vec::new(),
        }
    }
}

fn ids(found: &[Candidate]) -> Vec<&str> {
    found.iter().map(|c| c.session_id.as_str()).collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    transcripts: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    for d in ["repo/src", "wt", "other"] {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    let transcripts = base.join("transcripts");
    Fixture {
        _dir: dir,
        base,
        transcripts,
    }
}

#[test]
fn selects_sessions_under_from_path_including_missing_paths() {
    let f = fixture();
    let old = f.base.join("old/app");
    put(
        &f.transcripts,
        "a",
        "s-root",
        &transcript(&old, "2026-10-02T00:00:00Z"),
    );
    put(
        &f.transcripts,
        "b",
        "s-sub",
        &transcript(&old.join("sub"), "2026-10-01T00:00:00Z"),
    );
    put(
        &f.transcripts,
        "c",
        "s-sibling",
        &transcript(&f.base.join("old/application"), "2026-10-01T00:00:00Z"),
    );
    put(
        &f.transcripts,
        "d",
        "s-other",
        &transcript(&f.base.join("other"), "2026-10-01T00:00:00Z"),
    );
    let locate = fake_locate(f.base.clone());
    let found = candidates(&f.transcripts, &old, &old.to_string_lossy(), &locate);
    assert_eq!(ids(&found), vec!["s-sub", "s-root"], "첫 timestamp 순");
    assert_eq!(
        found[1].transcript_path,
        f.transcripts.join("a/s-root.jsonl")
    );
}

#[test]
fn selects_worktree_sessions_of_the_same_repository() {
    let f = fixture();
    put(
        &f.transcripts,
        "a",
        "s-repo",
        &transcript(&f.base.join("repo/src"), "2026-10-01T00:00:00Z"),
    );
    put(
        &f.transcripts,
        "b",
        "s-wt",
        &transcript(&f.base.join("wt"), "2026-10-02T00:00:00Z"),
    );
    put(
        &f.transcripts,
        "c",
        "s-other",
        &transcript(&f.base.join("other"), "2026-10-03T00:00:00Z"),
    );
    let locate = fake_locate(f.base.clone());
    let repo = f.base.join("repo");
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate);
    assert_eq!(ids(&found), vec!["s-repo", "s-wt"]);
}

#[test]
fn skips_subagents_files_without_cwd_and_non_jsonl() {
    let f = fixture();
    let repo = f.base.join("repo");
    let body = transcript(&repo, "2026-10-01T00:00:00Z");
    put(&f.transcripts, "a", "s-main", &body);
    put(&f.transcripts, "a/s-main/subagents", "agent-x", &body);
    put(
        &f.transcripts,
        "b",
        "s-nocwd",
        "{\"type\":\"user\",\"promptId\":\"p\"}\n",
    );
    fs::write(f.transcripts.join("a/notes.txt"), &body).unwrap();
    fs::write(f.transcripts.join("top.jsonl"), &body).unwrap();
    let locate = fake_locate(f.base.clone());
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate);
    assert_eq!(ids(&found), vec!["s-main"]);
}

#[test]
fn missing_transcripts_dir_has_no_candidates() {
    let f = fixture();
    let locate = fake_locate(f.base.clone());
    let repo = f.base.join("repo");
    assert!(candidates(&f.base.join("none"), &repo, "x", &locate).is_empty());
}
