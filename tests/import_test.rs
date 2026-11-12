use std::fs;
use std::path::{Path, PathBuf};

use hippo::i18n::{Lang, local_time};
use hippo::import::{Candidate, ImportStatus, candidates, import_status, render_preview};
use hippo::project::{Location, LocationKind};
use hippo::transcript::ExtractorState;
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

fn keep(text: &str) -> String {
    text.to_string()
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
    let found = candidates(&f.transcripts, &old, &old.to_string_lossy(), &locate, &keep);
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
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &keep);
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
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &keep);
    assert_eq!(ids(&found), vec!["s-main"]);
}

#[test]
fn missing_transcripts_dir_has_no_candidates() {
    let f = fixture();
    let locate = fake_locate(f.base.clone());
    let repo = f.base.join("repo");
    assert!(candidates(&f.base.join("none"), &repo, "x", &locate, &keep).is_empty());
}

fn user_line(cwd: Option<&Path>, extra: serde_json::Value) -> serde_json::Value {
    let mut v = json!({"type": "user", "timestamp": "2026-10-01T00:00:02Z"});
    if let Some(cwd) = cwd {
        v["cwd"] = json!(cwd.to_string_lossy());
    }
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

fn lines(values: &[serde_json::Value]) -> String {
    values.iter().map(|l| format!("{l}\n")).collect()
}

#[test]
fn candidate_carries_preview_info() {
    let f = fixture();
    let repo = f.base.join("repo");
    let body = lines(&[
        json!({"type": "queue-operation", "timestamp": "2026-10-01T00:00:00Z"}),
        user_line(
            Some(&repo),
            json!({"isMeta": true, "message": {"role": "user", "content": "<agent-message>meta</agent-message>"}}),
        ),
        user_line(
            Some(&repo),
            json!({"message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": "out"}]}}),
        ),
        user_line(
            None,
            json!({"message": {"role": "user", "content": "<task-notification>done</task-notification>"}}),
        ),
        user_line(
            None,
            json!({"message": {"role": "user", "content": [{"type": "text", "text": "  hello \n\n world"}, {"type": "text", "text": "SECRET"}]}}),
        ),
        user_line(
            None,
            json!({"message": {"role": "user", "content": "second prompt"}}),
        ),
    ]);
    let path = put(&f.transcripts, "slug", "s1", &body);
    let locate = fake_locate(f.base.clone());
    let mask = |t: &str| t.replace("SECRET", "[masked]");
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &mask);
    assert_eq!(found.len(), 1);
    let c = &found[0];
    assert_eq!(c.started_at, "2026-10-01T00:00:00Z");
    assert_eq!(c.start_cwd, repo.to_string_lossy());
    assert_eq!(c.size, fs::metadata(&path).unwrap().len());
    assert_eq!(c.first_prompt, "hello world [masked]");
    assert_eq!(c.status, ImportStatus::New);
}

#[test]
fn candidate_entrypoint_is_the_first_top_level_entrypoint() {
    let f = fixture();
    let repo = f.base.join("repo");
    let body = lines(&[
        json!({"type": "queue-operation", "timestamp": "2026-10-01T00:00:00Z", "message": {"entrypoint": "nested"}}),
        user_line(
            Some(&repo),
            json!({"entrypoint": "sdk-py", "message": {"role": "user", "content": "hi"}}),
        ),
        user_line(
            None,
            json!({"entrypoint": "cli", "message": {"role": "user", "content": "again"}}),
        ),
    ]);
    put(&f.transcripts, "slug", "s1", &body);
    let body = lines(&[user_line(
        Some(&repo),
        json!({"entrypoint": 3, "message": {"role": "user", "content": "x"}}),
    )]);
    put(&f.transcripts, "slug", "s2", &body);
    let locate = fake_locate(f.base.clone());
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &keep);
    let entry: Vec<&str> = found.iter().map(|c| c.entrypoint.as_str()).collect();
    assert_eq!(entry, vec!["sdk-py", ""]);
}

#[test]
fn first_prompt_is_masked_before_cut_to_80_chars() {
    let f = fixture();
    let repo = f.base.join("repo");
    let prompt = format!("{}SECRET-TOKEN", "가".repeat(75));
    let body = lines(&[user_line(
        Some(&repo),
        json!({"message": {"role": "user", "content": prompt}}),
    )]);
    put(&f.transcripts, "slug", "s1", &body);
    let locate = fake_locate(f.base.clone());
    let mask = |t: &str| t.replace("SECRET-TOKEN", "[masked]");
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &mask);
    let first = &found[0].first_prompt;
    assert_eq!(first.chars().count(), 80);
    assert_eq!(first, &format!("{}[mask", "가".repeat(75)));
}

#[test]
fn missing_prompt_and_timestamp_leave_fields_empty() {
    let f = fixture();
    let repo = f.base.join("repo");
    let body = lines(&[
        json!({"type": "user", "cwd": repo.to_string_lossy(), "isMeta": true, "message": {"role": "user", "content": "x"}}),
    ]);
    put(&f.transcripts, "slug", "s1", &body);
    let locate = fake_locate(f.base.clone());
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &keep);
    assert_eq!(
        (found[0].started_at.as_str(), found[0].first_prompt.as_str()),
        ("", "")
    );
}

fn state(start_offset: u64) -> ExtractorState {
    ExtractorState {
        start_offset,
        ..ExtractorState::default()
    }
}

#[test]
fn import_status_follows_pin_and_start_offset() {
    let target = ["T"];
    assert_eq!(
        import_status(None, &target, None, Some(0)),
        ImportStatus::New
    );
    assert_eq!(
        import_status(Some("T"), &target, None, Some(0)),
        ImportStatus::New
    );
    assert_eq!(
        import_status(Some("T"), &target, Some(&state(10)), Some(0)),
        ImportStatus::LiveOnly
    );
    assert_eq!(
        import_status(Some("T"), &target, Some(&state(0)), Some(0)),
        ImportStatus::Imported
    );
    assert_eq!(
        import_status(Some("X"), &target, Some(&state(0)), Some(0)),
        ImportStatus::OtherProject
    );
    assert_eq!(
        import_status(Some("S"), &["T", "S"], Some(&state(0)), Some(0)),
        ImportStatus::Imported,
        "병합 예정 원본의 세션은 대상으로 봄"
    );
}

#[test]
fn import_status_ignores_metadata_lines_before_first_prompt() {
    let target = ["T"];
    assert_eq!(
        import_status(Some("T"), &target, Some(&state(8_444)), Some(8_444)),
        ImportStatus::Imported,
        "첫 프롬프트부터 저장한 세션"
    );
    assert_eq!(
        import_status(Some("T"), &target, Some(&state(9_000)), Some(8_444)),
        ImportStatus::LiveOnly,
        "앞에 프롬프트가 있음"
    );
    assert_eq!(
        import_status(Some("T"), &target, Some(&state(10)), None),
        ImportStatus::Imported,
        "프롬프트 라인이 없음"
    );
}

#[test]
fn candidate_carries_first_prompt_line_offset() {
    let f = fixture();
    let repo = f.base.join("repo");
    let head = lines(&[
        json!({"type": "permission-mode", "permissionMode": "auto"}),
        json!({"type": "attachment", "cwd": repo.to_string_lossy(), "timestamp": "2026-10-01T00:00:00Z"}),
        json!({"type": "file-history-snapshot", "snapshot": {}}),
        user_line(
            Some(&repo),
            json!({"message": {"role": "user", "content": "no prompt id"}}),
        ),
    ]);
    let body = format!(
        "{head}{}",
        lines(&[user_line(
            Some(&repo),
            json!({"promptId": "p1", "message": {"role": "user", "content": "hi"}}),
        )])
    );
    put(&f.transcripts, "slug", "s1", &body);
    put(&f.transcripts, "slug", "s2", &head);
    let locate = fake_locate(f.base.clone());
    let found = candidates(&f.transcripts, &repo, &locate(&repo).path, &locate, &keep);
    let offsets: Vec<Option<u64>> = found.iter().map(|c| c.prompt_offset).collect();
    assert_eq!(offsets, vec![Some(head.len() as u64), None]);
}

fn candidate(id: &str, status: ImportStatus, prompt: &str) -> Candidate {
    Candidate {
        session_id: id.into(),
        transcript_path: PathBuf::from(format!("/t/{id}.jsonl")),
        started_at: "2026-10-01T00:00:00.000Z".into(),
        start_cwd: "/w/app".into(),
        size: 2_560,
        first_prompt: prompt.into(),
        status,
        entrypoint: "cli".into(),
        prompt_offset: None,
    }
}

#[test]
fn preview_text_lists_candidates_and_counts() {
    let sessions = vec![
        candidate(
            "0123456789abcdef",
            ImportStatus::New,
            "fix the \u{1b}[31mbug",
        ),
        candidate("fedcba9876543210", ImportStatus::OtherProject, ""),
    ];
    let text = render_preview("Would merge project a into b.\n", &sessions, Lang::En);
    let when = local_time(1_790_812_800);
    assert!(
        text.starts_with("Would merge project a into b.\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "[1] {when}  01234567  new  2.5KB  /w/app  fix the \\u{{1b}}[31mbug"
        )),
        "{text}"
    );
    assert!(
        text.contains("[2] ") && text.contains("fedcba98  other project"),
        "{text}"
    );
    assert!(
        text.trim_end()
            .ends_with("new 1, live-only 0, imported 0, other project 1"),
        "{text}"
    );
    let ko = render_preview("", &sessions, Lang::Ko);
    assert!(
        ko.contains("새 세션") && ko.contains("다른 프로젝트 1"),
        "{ko}"
    );
}
