use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_hippo");

fn hippo(root: &Path, args: &[&str], stdin: &str) -> Output {
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
    while !root.join("hippo.sock").exists() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    Daemon(child)
}

#[test]
fn hooks_config_prints_settings_json() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let out = hippo(dir.path(), &["hooks-config"], "");
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
    let out = hippo(dir.path(), &["launchd-plist"], "");
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
    fs::write(
        dir.path().join("config.toml"),
        "[daemon]\nautostart = false\n",
    )
    .unwrap();
    let input = r#"{"session_id":"s","transcript_path":"/t/s.jsonl","cwd":"/w"}"#;
    let out = hippo(dir.path(), &["hook", "stop"], input);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(dir.path().join("spool/pending.jsonl").exists());
    let bad = hippo(dir.path(), &["hook", "user-prompt-submit"], "garbage");
    assert!(bad.status.success());
    assert!(bad.stdout.is_empty());
}

#[test]
fn query_commands_fail_without_daemon() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    fs::write(
        dir.path().join("config.toml"),
        "[daemon]\nautostart = false\n",
    )
    .unwrap();
    for args in [&["search", "q"][..], &["show", "p"], &["status"]] {
        let out = hippo(dir.path(), args, "");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(!out.stderr.is_empty());
    }
}

#[test]
fn daemon_serves_status_search_show() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    let _daemon = spawn_daemon(&root);
    let status = hippo(&root, &["status"], "");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let text = String::from_utf8(status.stdout).unwrap();
    assert!(text.contains("iterations: 0"), "{text}");
    assert!(text.contains("test-hash-64"));

    let search = hippo(
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
    let show = hippo(&root, &["show", "nope", "--with-results"], "");
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

// ---- IF-CLI-6 ----

#[test]
fn install_and_uninstall_project_scope() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let project = dir.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    let p = project.to_string_lossy().to_string();
    let target = project.join(".claude/settings.local.json");

    let dry = hippo(
        dir.path(),
        &[
            "install",
            "--scope",
            "project",
            "--project",
            &p,
            "--dry-run",
        ],
        "",
    );
    assert!(dry.status.success());
    assert!(!target.exists());
    assert!(String::from_utf8_lossy(&dry.stdout).contains("hook stop"));

    assert!(
        hippo(
            dir.path(),
            &["install", "--scope", "project", "--project", &p],
            ""
        )
        .status
        .success()
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert!(
        v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("hippo hook stop")
    );

    let again = hippo(
        dir.path(),
        &["install", "--scope", "project", "--project", &p],
        "",
    );
    assert!(String::from_utf8_lossy(&again.stdout).contains("변경 없음"));

    assert!(
        hippo(
            dir.path(),
            &["uninstall", "--scope", "project", "--project", &p],
            ""
        )
        .status
        .success()
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({}));
}

#[test]
fn install_user_scope_uses_home() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let out = Command::new(BIN)
        .env("HOME", dir.path())
        .args([
            "--root",
            &dir.path().join("root").to_string_lossy(),
            "install",
            "--scope",
            "user",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join(".claude/settings.json").exists());
}

#[test]
fn install_refuses_invalid_settings() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let project = dir.path().join("proj/.claude");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("settings.local.json"), "{ broken").unwrap();
    let out = hippo(
        dir.path(),
        &[
            "install",
            "--scope",
            "project",
            "--project",
            &dir.path().join("proj").to_string_lossy(),
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(project.join("settings.local.json")).unwrap(),
        "{ broken"
    );
}

// ---- FR-DMN-7, FR-DMN-8 (실제 프로세스) ----

fn wait_for(cond: impl Fn() -> bool, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn cli_autostarts_daemon_which_exits_when_idle() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("config.toml"), "[embedder]\nbackend = \"hash\"\ndimension = 64\n[vector]\nbackend = \"memory\"\n[daemon]\nidle_timeout_secs = 2\n").unwrap();

    let transcript = dir.path().join("s.jsonl");
    fs::write(
        &transcript,
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/s2_tools.jsonl"),
        )
        .unwrap(),
    )
    .unwrap();
    let input = serde_json::json!({"session_id": "afd070e6-4625-4b31-8c43-f1f553949222", "transcript_path": transcript, "cwd": "/w"}).to_string();
    let hook = hippo(&root, &["hook", "stop"], &input);
    assert!(hook.status.success());
    assert!(
        !root.join("spool/pending.jsonl").exists(),
        "데몬을 띄워 바로 처리"
    );

    let status = hippo(&root, &["status"], "");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("iterations: 2"));
    assert!(
        wait_for(
            || !root.join("hippo.sock").exists(),
            Duration::from_secs(15)
        ),
        "유휴 종료"
    );
}
