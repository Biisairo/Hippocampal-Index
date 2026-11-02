use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_smem");

fn smem(root: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(BIN)
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_daemon(root: &Path) -> Daemon {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("config.toml"),
        "[embedder]\nbackend = \"hash\"\ndimension = 64\n[vector]\nbackend = \"memory\"\n",
    )
    .unwrap();
    let child = Command::new(BIN)
        .arg("--root")
        .arg(root)
        .arg("daemon")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("smem.sock").exists() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    Daemon(child)
}

#[test]
fn hooks_config_prints_settings_json() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let out = smem(dir.path(), &["hooks-config"], "");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(cmd.starts_with('/') && cmd.ends_with(" hook stop"), "{cmd}");
    assert!(
        v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["timeout"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

#[test]
fn launchd_plist_prints_xml() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let out = smem(dir.path(), &["launchd-plist"], "");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("<string>daemon</string>"));
    assert!(
        text.contains(&dir.path().display().to_string()),
        "--root 경로를 반영"
    );
}

#[test]
fn hook_never_fails_and_spools_without_daemon() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let input = r#"{"session_id":"s","transcript_path":"/t/s.jsonl","cwd":"/w"}"#;
    let out = smem(dir.path(), &["hook", "stop"], input);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(dir.path().join("spool/pending.jsonl").exists());
    let bad = smem(dir.path(), &["hook", "user-prompt-submit"], "garbage");
    assert!(bad.status.success());
    assert!(bad.stdout.is_empty());
}

#[test]
fn query_commands_fail_without_daemon() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    for args in [&["search", "q"][..], &["show", "p"], &["status"]] {
        let out = smem(dir.path(), args, "");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(!out.stderr.is_empty());
    }
}

#[test]
fn daemon_serves_status_search_show() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    let _daemon = spawn_daemon(&root);
    let status = smem(&root, &["status"], "");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let text = String::from_utf8(status.stdout).unwrap();
    assert!(text.contains("iterations: 0"), "{text}");
    assert!(text.contains("test-hash-64"));

    let search = smem(
        &root,
        &[
            "search",
            "아무거나",
            "--scope",
            "all",
            "--kind",
            "both",
            "--limit",
            "3",
        ],
        "",
    );
    assert!(search.status.success());
    assert_eq!(
        String::from_utf8(search.stdout).unwrap().trim(),
        "결과 없음"
    );
    let show = smem(&root, &["show", "nope", "--with-results"], "");
    assert!(show.status.success());
    assert!(
        String::from_utf8(show.stdout)
            .unwrap()
            .contains("기록 없음")
    );
}

#[test]
fn daemon_refuses_invalid_config() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    fs::write(
        dir.path().join("config.toml"),
        "[embedder]\nurl = \"https://api.example.com\"\n",
    )
    .unwrap();
    let out = Command::new(BIN)
        .arg("--root")
        .arg(dir.path())
        .arg("daemon")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("localhost"));
}
