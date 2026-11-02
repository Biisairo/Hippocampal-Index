use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use smem::install::{
    Action, InstallError, Scope, apply, is_smem_command, merge_install, merge_uninstall,
    target_file,
};
use smem::setup::hooks_config;

fn ours(exe: &str) -> Value {
    hooks_config(Path::new(exe), 3)
}

fn other_stop_hook() -> Value {
    json!({"hooks": [{"type": "command", "command": "afplay done.wav"}]})
}

// ---- FR-INS-1 ----

#[test]
fn target_files() {
    assert_eq!(
        target_file(&Scope::Project(PathBuf::from("/p"))).unwrap(),
        PathBuf::from("/p/.claude/settings.local.json")
    );
    assert!(
        target_file(&Scope::User)
            .unwrap()
            .ends_with(".claude/settings.json")
    );
}

// ---- FR-INS-2 ----

#[test]
fn recognizes_only_smem_hook_commands() {
    for c in [
        "/opt/bin/smem hook stop",
        "smem hook user-prompt-submit",
        "/x/target/release/smem hook post-tool-batch",
    ] {
        assert!(is_smem_command(c), "{c}");
    }
    for c in [
        "afplay done.wav",
        "/opt/bin/notsmem hook stop",
        "smem hook unknown",
        "/opt/bin/smem search q",
        "echo smem hook stop; rm -rf /",
    ] {
        assert!(!is_smem_command(c), "{c}");
    }
}

// ---- FR-INS-3 ----

#[test]
fn install_into_empty_settings() {
    let mut s = json!({});
    merge_install(&mut s, &ours("/a/smem")).unwrap();
    assert_eq!(s, ours("/a/smem"));
}

#[test]
fn install_preserves_other_hooks_keys_and_order() {
    let mut s = json!({"model": "opus", "hooks": {"Stop": [other_stop_hook()], "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]}, "env": {"A": "1"}});
    merge_install(&mut s, &ours("/a/smem")).unwrap();
    let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "hooks", "env"], "최상위 키 순서 유지");
    let stop = s["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop[0], other_stop_hook(), "다른 hook 유지");
    assert_eq!(stop[1]["hooks"][0]["command"], "/a/smem hook stop");
    assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "guard");
    assert!(s["hooks"]["UserPromptSubmit"].is_array());
}

#[test]
fn reinstall_is_idempotent_and_replaces_old_path() {
    let mut s = json!({"hooks": {"Stop": [other_stop_hook()]}});
    merge_install(&mut s, &ours("/a/smem")).unwrap();
    let once = s.clone();
    merge_install(&mut s, &ours("/a/smem")).unwrap();
    assert_eq!(s, once);
    merge_install(&mut s, &ours("/b/smem")).unwrap();
    let text = s.to_string();
    assert!(!text.contains("/a/smem") && text.contains("/b/smem"));
    assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
}

#[test]
fn smem_hook_inside_shared_group_is_removed_but_group_kept() {
    let mut s = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "afplay x"}, {"type": "command", "command": "/old/smem hook stop"}]}]}});
    merge_uninstall(&mut s).unwrap();
    assert_eq!(
        s,
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "afplay x"}]}]}})
    );
}

// ---- FR-INS-4 ----

#[test]
fn uninstall_removes_only_smem_entries_and_empty_containers() {
    let mut s = json!({"model": "opus", "hooks": {"Stop": [other_stop_hook()]}});
    merge_install(&mut s, &ours("/a/smem")).unwrap();
    merge_uninstall(&mut s).unwrap();
    assert_eq!(
        s,
        json!({"model": "opus", "hooks": {"Stop": [other_stop_hook()]}})
    );

    let mut only = json!({"model": "opus"});
    merge_install(&mut only, &ours("/a/smem")).unwrap();
    merge_uninstall(&mut only).unwrap();
    assert_eq!(only, json!({"model": "opus"}), "비게 된 hooks 키 제거");
}

// ---- FR-INS-6 ----

#[test]
fn rejects_unexpected_shapes() {
    for bad in [
        json!([]),
        json!({"hooks": []}),
        json!({"hooks": {"Stop": {}}}),
    ] {
        let mut s = bad.clone();
        assert!(
            matches!(
                merge_install(&mut s, &ours("/a/smem")),
                Err(InstallError::Shape(_))
            ),
            "{bad}"
        );
        assert_eq!(s, bad, "실패 시 변경 없음");
    }
}

// ---- FR-INS-5, 6: 파일 ----

#[test]
fn creates_missing_file_and_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p/.claude/settings.local.json");
    let out = apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    assert!(out.changed && out.backup.is_none());
    let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written, ours("/a/smem"));
    assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
}

#[test]
fn backs_up_writes_and_reports_diff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let original = "{\n  \"model\": \"opus\"\n}\n";
    fs::write(&path, original).unwrap();
    let out = apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    assert!(out.changed);
    assert!(out.diff.contains("+") && out.diff.contains("/a/smem hook stop"));
    let backup = out.backup.unwrap();
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("settings.json.smem-backup-")
    );
    let again = apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    assert!(!again.changed && again.backup.is_none() && again.diff.is_empty());
}

#[test]
fn dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}\n").unwrap();
    let out = apply(&path, &Action::Install(ours("/a/smem")), true).unwrap();
    assert!(out.changed && out.backup.is_none());
    assert!(!out.diff.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        1,
        "백업도 만들지 않음"
    );
    let missing = apply(
        &dir.path().join("none.json"),
        &Action::Install(ours("/a/smem")),
        true,
    )
    .unwrap();
    assert!(missing.changed && missing.backup.is_none());
    assert!(!dir.path().join("none.json").exists());
}

#[test]
fn invalid_json_is_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{ invalid").unwrap();
    assert!(matches!(
        apply(&path, &Action::Install(ours("/a/smem")), false),
        Err(InstallError::Parse { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ invalid");
}

#[test]
fn uninstall_file_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{\"model\": \"opus\"}").unwrap();
    apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    let out = apply(&path, &Action::Uninstall, false).unwrap();
    assert!(out.changed);
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v, json!({"model": "opus"}));
    assert!(!apply(&path, &Action::Uninstall, false).unwrap().changed);
    assert!(
        !apply(&dir.path().join("none.json"), &Action::Uninstall, false)
            .unwrap()
            .changed,
        "파일 없으면 변경 없음"
    );
}

#[cfg(unix)]
#[test]
fn preserves_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// ---- 보안: 링크를 따라가 다른 파일을 덮어쓰지 않는다 ----

#[cfg(unix)]
#[test]
fn planted_temp_symlink_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim");
    fs::write(&victim, "keep").unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    std::os::unix::fs::symlink(&victim, dir.path().join("settings.json.smem-tmp")).unwrap();
    apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v, ours("/a/smem"));
}

#[cfg(unix)]
#[test]
fn symlinked_settings_file_or_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.json");
    fs::write(&real, "{}").unwrap();
    let link = dir.path().join("settings.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(
        apply(&link, &Action::Install(ours("/a/smem")), false),
        Err(InstallError::Symlink(_))
    ));
    assert_eq!(fs::read_to_string(&real).unwrap(), "{}");

    let real_dir = dir.path().join("elsewhere");
    fs::create_dir_all(&real_dir).unwrap();
    let project = dir.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    std::os::unix::fs::symlink(&real_dir, project.join(".claude")).unwrap();
    let target = project.join(".claude/settings.local.json");
    assert!(matches!(
        apply(&target, &Action::Install(ours("/a/smem")), false),
        Err(InstallError::Symlink(_))
    ));
    assert!(!real_dir.join("settings.local.json").exists());
}

#[cfg(unix)]
#[test]
fn backup_and_result_are_owner_only_when_new() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let out = apply(&path, &Action::Install(ours("/a/smem")), false).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o644, "기존 권한 유지");
    assert_eq!(mode(&out.backup.unwrap()), 0o600);
}
