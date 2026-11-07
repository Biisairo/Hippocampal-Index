use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_hippo");

fn hippo(root: &Path, args: &[&str], stdin: &str) -> Output {
    hippo_in(root, Path::new("/"), args, stdin)
}

/// 데이터 루트는 실행 파일 위치로 정하므로, `<root>/bin/hippo`에 빌드한 실행 파일의 복사본을 둔다.
/// 하드 링크는 inode를 공유하므로, 병렬 테스트가 새 경로로 동시에 실행하면 macOS AMFI가 서명 검사에
/// 실패해 프로세스를 SIGKILL로 끝내는 일이 있다. (FR-ROOT-2)
fn installed_bin(root: &Path) -> PathBuf {
    let bin = root.join("bin/hippo");
    if !bin.exists() {
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::copy(BIN, &bin).unwrap();
    }
    bin
}

fn hippo_in(root: &Path, cwd: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(installed_bin(root))
        .current_dir(cwd)
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

/// 테스트 데몬이 읽는 과거 transcript 디렉터리. 실제 `~/.claude/projects`를 읽지 않는다.
fn transcripts_dir(root: &Path) -> std::path::PathBuf {
    root.parent().unwrap().join("tx")
}

fn spawn_daemon(root: &Path) -> Daemon {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("config.toml"),
        format!(
            "[embedder]\nbackend = \"hash\"\ndimension = 64\n[vector]\nbackend = \"memory\"\n[import]\ntranscripts_dir = \"{}\"\n",
            transcripts_dir(root).to_string_lossy()
        ),
    )
    .unwrap();
    let child = Command::new(installed_bin(root))
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
    assert!(
        (cmd.starts_with('/') || cmd.starts_with("~/")) && cmd.ends_with(" hook stop"),
        "{cmd}"
    );
    let home = std::env::var("HOME").unwrap();
    assert!(
        !cmd.contains(&home),
        "홈 아래 경로는 ~로 씀 (FR-INS-10): {cmd}"
    );
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
        "데이터 루트 경로를 반영"
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
    assert!(text.contains("projects: 0"), "{text}");
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
        "No results"
    );
    let show = hippo(&root, &["show", "nope", "--with-results"], "");
    assert!(show.status.success());
    assert!(
        String::from_utf8(show.stdout)
            .unwrap()
            .contains("no records")
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
    let out = Command::new(installed_bin(dir.path()))
        .arg("daemon")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("localhost"));
}

// ---- IF-CLI-6, IF-CLI-13, IF-CLI-14, FR-INS-14 ----

/// 관리 실행 파일이 아닌 실행 파일(빌드 폴더)로 실행한다. HOME과 PATH는 격리한다.
fn unmanaged(home: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .args(args)
        .output()
        .unwrap()
}

fn run(exe: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(exe)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn unmanaged_binary_only_installs_and_root_option_is_gone() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    for args in [
        &["status"][..],
        &["search", "q"],
        &["uninstall"],
        &["purge", "--yes"],
    ] {
        let out = unmanaged(dir.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("hippo install"),
            "{args:?}: {out:?}"
        );
    }
    let hook = unmanaged(dir.path(), &["hook", "user-prompt-submit"]);
    assert!(hook.status.success() && hook.stdout.is_empty(), "{hook:?}");
    let root = unmanaged(dir.path(), &["--root", "/tmp/x", "status"]);
    assert_eq!(root.status.code(), Some(2), "--root는 없음");
    let help = unmanaged(dir.path(), &[]);
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("Usage"),
        "인자가 없으면 도움말: {help:?}"
    );
    assert!(!dir.path().join(".hippo").exists(), "아무것도 만들지 않음");
}

#[test]
fn install_uninstall_and_purge_through_the_cli() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let home = fs::canonicalize(dir.path()).unwrap();
    let claude = home.join(".claude");
    let root = home.join("data");
    let link = home.join(".local/bin/hippo");

    let dry = unmanaged(&home, &["install", "--data-dir", "data", "--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    assert!(!root.exists() && !claude.exists());

    let out = Command::new(BIN)
        .current_dir(&home)
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .args(["install", "--data-dir", "data"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains("is not on PATH"), "PATH 경고: {printed}");
    let exe = root.join("bin/hippo");
    assert!(exe.exists(), "실행 파일을 데이터 루트로 복사 (FR-INS-8)");
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();
    assert!(
        settings.contains("\"~/data/bin/hippo hook stop\""),
        "hook은 설치 경로: {settings}"
    );
    assert!(
        fs::read_to_string(claude.join("CLAUDE.md"))
            .unwrap()
            .contains("`~/data/bin/hippo search"),
        "안내 블록도 설치 경로"
    );
    assert_eq!(fs::read_link(&link).unwrap(), exe);

    let via_link = run(&link, &home, &["hooks-config"]);
    assert!(
        via_link.status.success(),
        "링크로 실행해도 루트를 찾음: {via_link:?}"
    );
    let again = run(&exe, &home, &["install", "--dry-run"]);
    let again_text = String::from_utf8_lossy(&again.stdout);
    assert!(
        again_text.contains("Binary up to date") && again_text.contains("No change"),
        "기본값은 자기 루트와 기록한 Claude 폴더: {again_text}"
    );

    let removed = run(&exe, &home, &["uninstall"]);
    assert!(removed.status.success(), "{removed:?}");
    assert!(!claude.join("settings.json").exists());
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(exe.exists(), "제거해도 실행 파일은 남음");

    let refused = run(&exe, &home, &["purge"]);
    assert_eq!(refused.status.code(), Some(1), "터미널이 아니고 --yes 없음");
    assert!(root.exists());
    let purged = run(&exe, &home, &["purge", "--yes"]);
    assert!(purged.status.success(), "{purged:?}");
    assert!(!root.exists(), "데이터 루트 삭제");
}

#[test]
fn install_refuses_invalid_settings() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let claude = dir.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("settings.json"), "{ broken").unwrap();
    let out = unmanaged(
        dir.path(),
        &[
            "install",
            "--data-dir",
            &dir.path().join("data").to_string_lossy(),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(claude.join("settings.json")).unwrap(),
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
    let input = serde_json::json!({"session_id": "afd070e6-4625-4b31-8c43-f1f553949222", "transcript_path": transcript, "cwd": "/w", "prompt_id": "8f64252a-0037-484d-a451-35aa2c0076ed"}).to_string();
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

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn project_commands_and_scopes_use_current_directory() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    let work = fs::canonicalize(dir.path()).unwrap().join("work");
    fs::create_dir_all(work.join("sub")).unwrap();
    let _daemon = spawn_daemon(&root);

    let fixture = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/s2_tools.jsonl"),
    )
    .unwrap();
    let body: String = fixture
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            if v.get("cwd").is_some() {
                v["cwd"] = Value::String(work.to_string_lossy().into_owned());
            }
            format!("{v}\n")
        })
        .collect();
    let transcript = dir.path().join("s.jsonl");
    fs::write(&transcript, &body).unwrap();
    let input = serde_json::json!({"session_id": "afd070e6-4625-4b31-8c43-f1f553949222", "transcript_path": transcript, "cwd": work, "prompt_id": "8f64252a-0037-484d-a451-35aa2c0076ed"}).to_string();
    assert!(hippo(&root, &["hook", "stop"], &input).status.success());

    let list = stdout(&hippo(&root, &["project", "list"], ""));
    assert!(list.contains(&*work.to_string_lossy()), "{list}");

    let here = stdout(&hippo_in(&root, &work, &["search", "병렬로"], ""));
    assert!(
        here.contains("[iteration]"),
        "기본 범위는 현재 프로젝트: {here}"
    );
    let relative = stdout(&hippo_in(
        &root,
        &work.join("sub"),
        &["search", "병렬로", "--scope", ".."],
        "",
    ));
    assert!(
        relative.contains("[iteration]"),
        "상대 경로 범위: {relative}"
    );
    let elsewhere = stdout(&hippo_in(&root, dir.path(), &["search", "병렬로"], ""));
    assert!(elsewhere.contains("No project"), "{elsewhere}");

    let shown = stdout(&hippo_in(
        &root,
        &work,
        &[
            "show",
            "8f64252a-0037-484d-a451-35aa2c0076ed",
            "--scope",
            "project",
        ],
        "",
    ));
    assert!(shown.contains("[1]"), "{shown}");

    let past_dir = transcripts_dir(&root).join("slug");
    fs::create_dir_all(&past_dir).unwrap();
    let past = body
        .replace("afd070e6-4625-4b31-8c43-f1f553949222", "old-session")
        .replace("\"id\":\"msg_", "\"id\":\"msg_old_")
        .replace(
            &*work.to_string_lossy(),
            &work.join("sub").to_string_lossy(),
        );
    fs::write(past_dir.join("old-session.jsonl"), past).unwrap();
    let dry = stdout(&hippo_in(
        &root,
        &work,
        &["import", "--dry-run", "--wait"],
        "",
    ));
    assert!(
        dry.contains("[1] ") && dry.contains("old-sess  new") && dry.contains("new 1"),
        "--dry-run: {dry}"
    );
    assert!(
        !dry.contains("[1/1]") && !dry.contains("Embedding complete"),
        "{dry}"
    );
    let imported = stdout(&hippo_in(&root, &work, &["import", "--wait"], ""));
    assert!(
        imported.contains("Embedding complete"),
        "--wait: {imported}"
    );
    assert!(
        imported.contains("[1/1] old-sess imported  iterations 2"),
        "{imported}"
    );
    let again = stdout(&hippo_in(&root, &work, &["import", "--from", "."], ""));
    assert!(again.contains("[1/1] old-sess unchanged"), "{again}");
    let config = fs::read_to_string(root.join("config.toml")).unwrap();
    fs::write(
        root.join("config.toml"),
        format!("{config}[ui]\nlanguage = \"ko\"\n"),
    )
    .unwrap();
    let korean = stdout(&hippo_in(&root, &work, &["import"], ""));
    assert!(
        korean.contains("[1/1] old-sess 변경 없음") && korean.contains("합계"),
        "{korean}"
    );
    let korean_none = stdout(&hippo_in(&root, dir.path(), &["search", "병렬로"], ""));
    assert!(
        korean_none.contains("프로젝트 없음"),
        "데몬 출력도 설정을 따름: {korean_none}"
    );
    fs::write(root.join("config.toml"), config).unwrap();
    let unknown = stdout(&hippo_in(
        &root,
        &work,
        &["import", "--from", "/no/such/source"],
        "",
    ));
    assert!(!unknown.contains("[1/"), "{unknown}");

    assert!(hippo(&root, &["reindex"], "").status.success());
    let status = stdout(&hippo(&root, &["status"], ""));
    assert!(
        status.contains("projects: 1") && status.contains("iterations: 4"),
        "{status}"
    );
}

// ---- IF-CLI-11, IF-CLI-12, FR-DEL-4 ----

#[test]
fn forget_and_project_remove_confirm_and_report() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    let work = fs::canonicalize(dir.path()).unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    let _daemon = spawn_daemon(&root);
    let session = "afd070e6-4625-4b31-8c43-f1f553949222";
    let prompt = "8f64252a-0037-484d-a451-35aa2c0076ed";
    let fixture = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/s2_tools.jsonl"),
    )
    .unwrap();
    let body: String = fixture
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            if v.get("cwd").is_some() {
                v["cwd"] = Value::String(work.to_string_lossy().into_owned());
            }
            format!("{v}\n")
        })
        .collect();
    let transcript = dir.path().join("s.jsonl");
    fs::write(&transcript, &body).unwrap();
    let input = serde_json::json!({"session_id": session, "transcript_path": transcript, "cwd": work, "prompt_id": prompt}).to_string();
    assert!(hippo(&root, &["hook", "stop"], &input).status.success());
    let iterations = |n: u32| {
        let list = stdout(&hippo(&root, &["project", "list"], ""));
        assert!(list.contains(&format!("iterations={n}")), "{list}");
    };
    iterations(2);

    let preview = stdout(&hippo(
        &root,
        &["forget", "--session", session, "--dry-run"],
        "",
    ));
    assert!(preview.contains(&*work.to_string_lossy()), "{preview}");
    assert!(preview.contains("iterations: 2"), "{preview}");
    iterations(2);

    let refused = hippo(&root, &["forget", "--session", session], "y\n");
    assert_eq!(refused.status.code(), Some(1), "터미널이 아니고 --yes 없음");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--yes"),
        "{refused:?}"
    );
    iterations(2);

    let both = hippo(
        &root,
        &["forget", "--session", session, "--prompt", prompt],
        "",
    );
    assert!(!both.status.success(), "둘 중 하나만 받음");
    assert!(!hippo(&root, &["forget"], "").status.success());

    let done = stdout(&hippo_in(
        &root,
        &work,
        &["forget", "--prompt", prompt, "--scope", "project", "--yes"],
        "",
    ));
    assert!(done.contains("Deleted"), "{done}");
    iterations(0);
    let none = stdout(&hippo(&root, &["forget", "--prompt", prompt, "--yes"], ""));
    assert!(none.contains("Nothing to delete"), "{none}");

    let project_preview = stdout(&hippo_in(
        &root,
        &work,
        &["project", "remove", ".", "--dry-run"],
        "",
    ));
    assert!(
        project_preview.contains("sessions: 1"),
        "수집 위치는 남아 있음: {project_preview}"
    );
    let removed = stdout(&hippo_in(
        &root,
        &work,
        &["project", "remove", ".", "--yes"],
        "",
    ));
    assert!(removed.contains("Deleted"), "{removed}");
    let list = stdout(&hippo(&root, &["project", "list"], ""));
    assert!(list.contains("No project"), "{list}");
    let missing = stdout(&hippo(
        &root,
        &[
            "project",
            "remove",
            "0123456789abcdef0123456789abcdef",
            "--yes",
        ],
        "",
    ));
    assert!(missing.contains("Nothing to delete"), "{missing}");
}

// ---- FR-DMN-12, IF-CLI-15 ----

#[test]
fn stop_shuts_down_running_daemon_and_reports_when_nothing_runs() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    let mut daemon = spawn_daemon(&root);
    let out = stdout(&hippo(&root, &["stop"], ""));
    assert!(out.contains("Stopped the daemon"), "{out}");
    assert!(!root.join("hippo.sock").exists(), "소켓 파일이 없어짐");
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "데몬 프로세스 종료");
        std::thread::sleep(Duration::from_millis(50));
    }
    let again = stdout(&hippo(&root, &["stop"], ""));
    assert!(again.contains("Nothing to stop"), "{again}");
}

// ---- FR-CAP-8 ----

#[test]
fn paused_capture_neither_contacts_daemon_nor_spools() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    fs::write(
        dir.path().join("config.toml"),
        "[daemon]\nautostart = false\n[capture]\nenabled = false\n",
    )
    .unwrap();
    let input = r#"{"session_id":"s","transcript_path":"/t/s.jsonl","cwd":"/w"}"#;
    let out = hippo(dir.path(), &["hook", "stop"], input);
    assert!(out.status.success() && out.stdout.is_empty());
    assert!(
        !dir.path().join("spool/pending.jsonl").exists(),
        "스풀에 쓰지 않음"
    );
    fs::write(
        dir.path().join("config.toml"),
        "[daemon]\nautostart = false\n",
    )
    .unwrap();
    assert!(hippo(dir.path(), &["hook", "stop"], input).status.success());
    assert!(
        dir.path().join("spool/pending.jsonl").exists(),
        "재생하면 다시 동작"
    );
}
