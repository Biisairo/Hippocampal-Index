use std::fs;
use std::path::{Path, PathBuf};

use hippo::install::{
    Action, InstallError, apply, is_hippo_command, is_installed, merge_install, merge_uninstall,
    settings_file,
};
use hippo::setup::hooks_config;
use serde_json::{Value, json};

fn ours(exe: &str) -> Value {
    hooks_config(Path::new(exe), 3)
}

fn other_stop_hook() -> Value {
    json!({"hooks": [{"type": "command", "command": "afplay done.wav"}]})
}

// ---- FR-INS-1 ----

#[test]
fn settings_file_is_in_claude_folder() {
    assert_eq!(
        settings_file(Path::new("/c")),
        PathBuf::from("/c/settings.json")
    );
}

// ---- FR-INS-2 ----

#[test]
fn recognizes_only_hippo_hook_commands() {
    for c in [
        "/opt/bin/hippo hook stop",
        "hippo hook user-prompt-submit",
        "/x/target/release/hippo hook post-tool-batch",
    ] {
        assert!(is_hippo_command(c), "{c}");
    }
    for c in [
        "afplay done.wav",
        "/opt/bin/nothippo hook stop",
        "hippo hook unknown",
        "/opt/bin/hippo search q",
        "echo hippo hook stop; rm -rf /",
    ] {
        assert!(!is_hippo_command(c), "{c}");
    }
}

// ---- FR-INS-3 ----

#[test]
fn install_into_empty_settings() {
    let mut s = json!({});
    merge_install(&mut s, &ours("/a/hippo")).unwrap();
    assert_eq!(s, ours("/a/hippo"));
}

#[test]
fn install_preserves_other_hooks_keys_and_order() {
    let mut s = json!({"model": "opus", "hooks": {"Stop": [other_stop_hook()], "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]}, "env": {"A": "1"}});
    merge_install(&mut s, &ours("/a/hippo")).unwrap();
    let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "hooks", "env"], "최상위 키 순서 유지");
    let stop = s["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop[0], other_stop_hook(), "다른 hook 유지");
    assert_eq!(stop[1]["hooks"][0]["command"], "/a/hippo hook stop");
    assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "guard");
    assert!(s["hooks"]["UserPromptSubmit"].is_array());
}

#[test]
fn reinstall_is_idempotent_and_replaces_old_path() {
    let mut s = json!({"hooks": {"Stop": [other_stop_hook()]}});
    merge_install(&mut s, &ours("/a/hippo")).unwrap();
    let once = s.clone();
    merge_install(&mut s, &ours("/a/hippo")).unwrap();
    assert_eq!(s, once);
    merge_install(&mut s, &ours("/b/hippo")).unwrap();
    let text = s.to_string();
    assert!(!text.contains("/a/hippo") && text.contains("/b/hippo"));
    assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
}

#[test]
fn hippo_hook_inside_shared_group_is_removed_but_group_kept() {
    let mut s = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "afplay x"}, {"type": "command", "command": "/old/hippo hook stop"}]}]}});
    merge_uninstall(&mut s).unwrap();
    assert_eq!(
        s,
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "afplay x"}]}]}})
    );
}

// ---- FR-INS-4 ----

#[test]
fn uninstall_removes_only_hippo_entries_and_empty_containers() {
    let mut s = json!({"model": "opus", "hooks": {"Stop": [other_stop_hook()]}});
    merge_install(&mut s, &ours("/a/hippo")).unwrap();
    merge_uninstall(&mut s).unwrap();
    assert_eq!(
        s,
        json!({"model": "opus", "hooks": {"Stop": [other_stop_hook()]}})
    );

    let mut only = json!({"model": "opus"});
    merge_install(&mut only, &ours("/a/hippo")).unwrap();
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
                merge_install(&mut s, &ours("/a/hippo")),
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
    let out = apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    assert!(out.changed && out.backup.is_none());
    let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written, ours("/a/hippo"));
    assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
}

#[test]
fn backs_up_writes_and_reports_diff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let original = "{\n  \"model\": \"opus\"\n}\n";
    fs::write(&path, original).unwrap();
    let out = apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    assert!(out.changed);
    assert!(out.diff.contains("+") && out.diff.contains("/a/hippo hook stop"));
    let backup = out.backup.unwrap();
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("settings.json.hippo-backup-")
    );
    let again = apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    assert!(!again.changed && again.backup.is_none() && again.diff.is_empty());
}

#[test]
fn dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}\n").unwrap();
    let out = apply(&path, &Action::Install(ours("/a/hippo")), true).unwrap();
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
        &Action::Install(ours("/a/hippo")),
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
        apply(&path, &Action::Install(ours("/a/hippo")), false),
        Err(InstallError::Parse { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ invalid");
}

#[test]
fn uninstall_file_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{\"model\": \"opus\"}").unwrap();
    apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
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
    apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
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
    std::os::unix::fs::symlink(&victim, dir.path().join("settings.json.hippo-tmp")).unwrap();
    apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v, ours("/a/hippo"));
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
        apply(&link, &Action::Install(ours("/a/hippo")), false),
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
        apply(&target, &Action::Install(ours("/a/hippo")), false),
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
    let out = apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o644, "기존 권한 유지");
    assert_eq!(mode(&out.backup.unwrap()), 0o600);
}

// ---- FR-INS-7, IF-SKL-1: Claude 안내 블록 ----

const EXE: &str = "/opt/hippo bin/hippo";

use hippo::install::{
    GUIDE_BEGIN, GUIDE_END, apply_memory, guide_block, memory_file, remove_block, upsert_block,
};

#[test]
fn memory_file_is_in_claude_folder() {
    assert_eq!(memory_file(Path::new("/c")), PathBuf::from("/c/CLAUDE.md"));
}

#[test]
fn guide_block_is_english_and_marked() {
    let block = guide_block(Path::new(EXE));
    assert!(block.starts_with(&format!("{GUIDE_BEGIN}\n")));
    assert!(block.ends_with(&format!("{GUIDE_END}\n")));
    assert!(block.contains("hippo' search") && block.contains("hippo' show"));
    assert!(
        !block.chars().any(|c| ('가'..='힣').contains(&c)),
        "영어 고정 (FR-UI-2)"
    );
}

#[test]
fn install_then_remove_keeps_a_file_without_final_newline_byte_identical() {
    let block = guide_block(Path::new(EXE));
    let original = "# Mine\nlast line";
    let installed = upsert_block(original, &block).unwrap();
    assert!(!installed.ends_with('\n'), "끝 줄바꿈 없음을 유지");
    assert_eq!(
        upsert_block(&installed, &block).unwrap(),
        installed,
        "다시 넣어도 같음"
    );
    assert_eq!(remove_block(&installed).unwrap(), original);

    let middle = format!("top\n\n{block}\nbottom");
    assert_eq!(remove_block(&middle).unwrap(), "top\n\nbottom");
}

#[test]
fn install_then_remove_keeps_crlf_and_mixed_line_endings_byte_identical() {
    let block = guide_block(Path::new(EXE));
    for original in [
        "# Mine\r\nkeep this\r\n",
        "# Mine\r\nlast line",
        "# Mine\r\nlf line\nlast\r\n",
        "# Mine\r\n\r\n",
    ] {
        let installed = upsert_block(original, &block).unwrap();
        assert!(
            installed.starts_with("# Mine\r\n"),
            "기존 줄 끝 유지: {original:?}"
        );
        assert!(
            installed.contains(&format!("{GUIDE_BEGIN}\r\n")),
            "새 줄은 첫 줄의 줄 끝: {original:?}"
        );
        assert_eq!(
            upsert_block(&installed, &block).unwrap(),
            installed,
            "다시 넣어도 같음: {original:?}"
        );
        assert_eq!(remove_block(&installed).unwrap(), original);
    }
    let crlf_block = block.replace('\n', "\r\n");
    let middle = format!("top\r\n\r\n{crlf_block}bottom\r\n");
    assert_eq!(remove_block(&middle).unwrap(), "top\r\n\r\nbottom\r\n");
}

#[test]
fn block_is_appended_replaced_and_removed_without_touching_other_text() {
    let original = "# Mine\nkeep this\n";
    let installed = upsert_block(original, &guide_block(Path::new(EXE))).unwrap();
    assert_eq!(
        installed,
        format!("{original}\n{}", guide_block(Path::new(EXE)))
    );
    assert_eq!(
        upsert_block(&installed, &guide_block(Path::new(EXE))).unwrap(),
        installed,
        "다시 넣어도 같음"
    );
    let stale = installed.replace("hippo show", "old text");
    assert_eq!(
        upsert_block(&stale, &guide_block(Path::new(EXE))).unwrap(),
        installed,
        "이전 블록은 교체"
    );
    assert_eq!(remove_block(&installed).unwrap(), original);

    let middle = format!("top\n\n{}\nbottom\n", guide_block(Path::new(EXE)));
    assert_eq!(remove_block(&middle).unwrap(), "top\n\nbottom\n");
    assert_eq!(
        upsert_block("", &guide_block(Path::new(EXE))).unwrap(),
        guide_block(Path::new(EXE))
    );
    assert_eq!(remove_block("no block\n").unwrap(), "no block\n");
}

#[test]
fn unclosed_block_is_rejected() {
    let broken = format!("a\n{GUIDE_BEGIN}\nhalf\n");
    assert!(matches!(
        upsert_block(&broken, &guide_block(Path::new(EXE))),
        Err(InstallError::UnclosedBlock)
    ));
    assert!(matches!(
        remove_block(&broken),
        Err(InstallError::UnclosedBlock)
    ));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CLAUDE.md");
    fs::write(&path, &broken).unwrap();
    assert!(apply_memory(&path, Some(&guide_block(Path::new(EXE))), false).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), broken, "수정하지 않음");
}

#[test]
fn memory_file_roundtrip_backup_dry_run_and_delete_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CLAUDE.local.md");
    let dry = apply_memory(&path, Some(&guide_block(Path::new(EXE))), true).unwrap();
    assert!(dry.changed && !path.exists());

    let created = apply_memory(&path, Some(&guide_block(Path::new(EXE))), false).unwrap();
    assert!(created.changed && created.backup.is_none());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        guide_block(Path::new(EXE))
    );
    assert!(
        !apply_memory(&path, Some(&guide_block(Path::new(EXE))), false)
            .unwrap()
            .changed
    );

    let removed = apply_memory(&path, None, false).unwrap();
    assert!(
        removed.changed && removed.backup.is_none(),
        "지우는 파일은 백업하지 않음"
    );
    assert!(!path.exists(), "공백만 남으면 파일을 지움");
    assert!(!apply_memory(&path, None, false).unwrap().changed);

    fs::write(&path, "mine\n").unwrap();
    apply_memory(&path, Some(&guide_block(Path::new(EXE))), false).unwrap();
    apply_memory(&path, None, false).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine\n");
}

#[test]
fn symlinked_memory_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.md");
    fs::write(&real, "x\n").unwrap();
    let link = dir.path().join("CLAUDE.md");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(
        apply_memory(&link, Some(&guide_block(Path::new(EXE))), false),
        Err(InstallError::Symlink(_))
    ));
    assert_eq!(fs::read_to_string(&real).unwrap(), "x\n");
}

#[test]
fn guide_block_uses_absolute_quoted_executable_path() {
    let block = guide_block(Path::new(EXE));
    assert!(block.contains("`'/opt/hippo bin/hippo' search"), "{block}");
    assert!(block.contains("`'/opt/hippo bin/hippo' show"), "{block}");
    let plain = guide_block(Path::new("/usr/local/bin/hippo"));
    assert!(plain.contains("`/usr/local/bin/hippo search"), "{plain}");
    let quote = guide_block(Path::new("/it's/hippo"));
    assert!(quote.contains("`'/it'\\''s/hippo' search"), "{quote}");
}

// ---- FR-INS-4: 빈 설정 정리 ----

#[test]
fn uninstall_deletes_settings_left_empty_and_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p/.claude/settings.local.json");
    apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    let out = apply(&path, &Action::Uninstall, false).unwrap();
    assert!(
        out.changed && out.backup.is_none(),
        "지우는 파일은 백업하지 않음"
    );
    assert!(!path.exists());
    assert!(!dir.path().join("p/.claude").exists(), "빈 디렉터리도 지움");
    assert!(dir.path().join("p").exists(), "그 위 디렉터리는 그대로");

    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{\n  \"model\": \"opus\"\n}\n").unwrap();
    fs::write(path.with_file_name("other.json"), "{}").unwrap();
    apply(&path, &Action::Install(ours("/a/hippo")), false).unwrap();
    let kept = apply(&path, &Action::Uninstall, false).unwrap();
    assert!(kept.backup.is_some(), "다른 내용이 남는 파일은 백업");
    let left: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(left, json!({"model": "opus"}));
}

// ---- FR-INS-8: 실행 파일 복사 ----

use hippo::install::{install_binary, installed_exe};
use std::os::unix::fs::PermissionsExt;

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn binary_is_copied_to_data_root_once_per_content() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("build/hippo");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::write(&src, b"binary v1").unwrap();
    let root = dir.path().join("root");
    let target = installed_exe(&root);
    assert_eq!(target, root.join("bin/hippo"));

    let dry = install_binary(&src, &root, true).unwrap();
    assert!(
        dry.copied && dry.path == target && !target.exists(),
        "dry-run은 복사하지 않음"
    );

    let first = install_binary(&src, &root, false).unwrap();
    assert!(first.copied);
    assert_eq!(fs::read(&target).unwrap(), b"binary v1");
    assert_eq!(
        (mode(&target), mode(target.parent().unwrap())),
        (0o700, 0o700)
    );
    assert!(
        !install_binary(&src, &root, false).unwrap().copied,
        "같은 내용이면 생략"
    );

    fs::write(&src, b"binary v2").unwrap();
    assert!(install_binary(&src, &root, false).unwrap().copied);
    assert_eq!(fs::read(&target).unwrap(), b"binary v2");
    assert!(
        !install_binary(&target, &root, false).unwrap().copied,
        "설치된 복사본으로 실행하면 생략"
    );
}

#[test]
fn symlinked_install_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("hippo-src");
    fs::write(&src, b"bin").unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("bin")).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    fs::write(&elsewhere, b"keep").unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("bin/hippo")).unwrap();
    assert!(matches!(
        install_binary(&src, &root, false),
        Err(InstallError::Symlink(_))
    ));
    assert_eq!(fs::read(&elsewhere).unwrap(), b"keep");
}

// ---- FR-INS-10: 셸 표기 ----

use hippo::setup::shell_path;

#[test]
fn paths_under_home_are_written_with_tilde() {
    let home = Some(Path::new("/Users/someone"));
    assert_eq!(
        shell_path(Path::new("/Users/someone/.hippo/bin/hippo"), home),
        "~/.hippo/bin/hippo"
    );
    assert_eq!(
        shell_path(Path::new("/Users/someone/my tools/hippo"), home),
        "~/'my tools/hippo'",
        "~는 따옴표 밖에 둬야 펼쳐짐"
    );
    assert_eq!(
        shell_path(Path::new("/Users/someone2/hippo"), home),
        "/Users/someone2/hippo",
        "이름이 비슷한 다른 홈은 그대로"
    );
    assert_eq!(
        shell_path(Path::new("/opt/bin/hippo"), home),
        "/opt/bin/hippo"
    );
    assert_eq!(
        shell_path(Path::new("/opt/x y/hippo"), None),
        "'/opt/x y/hippo'"
    );
    assert_eq!(
        shell_path(Path::new("/Users/someone/it's/hippo"), home),
        "~/'it'\\''s/hippo'"
    );
}

#[test]
fn hooks_and_guide_use_tilde_paths_under_home() {
    let home = std::env::var("HOME").unwrap();
    let exe = Path::new(&home).join(".hippo/bin/hippo");
    let hooks = hooks_config(&exe, 3).to_string();
    assert!(
        hooks.contains("\"~/.hippo/bin/hippo hook stop\""),
        "{hooks}"
    );
    assert!(!hooks.contains(&home), "홈 경로를 드러내지 않음");
    assert!(
        is_hippo_command("~/.hippo/bin/hippo hook stop"),
        "설치 식별 (FR-INS-2)"
    );
    let block = guide_block(&exe);
    assert!(block.contains("`~/.hippo/bin/hippo search"), "{block}");
    assert!(!block.contains(&home));
}

// ---- FR-GUI-10 ----

#[test]
fn is_installed_detects_hippo_hooks_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    assert!(!is_installed(&path), "파일 없음");
    fs::write(
        &path,
        json!({"hooks": {"Stop": [other_stop_hook()]}}).to_string(),
    )
    .unwrap();
    assert!(!is_installed(&path));
    let mut settings = json!({});
    merge_install(&mut settings, &ours("/x/hippo")).unwrap();
    fs::write(&path, settings.to_string()).unwrap();
    assert!(is_installed(&path));
    fs::write(&path, "not json").unwrap();
    assert!(!is_installed(&path));
}

// ---- FR-INS-8, 9, 11~13: 설치·제거·완전 삭제 동작 ----

use hippo::commands::{InstallTarget, purge_data, run_install, run_uninstall, stop_services};
use hippo::i18n::Lang;

struct Env {
    _dir: tempfile::TempDir,
    target: InstallTarget,
    exe: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let exe = base.join("build/hippo");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, b"binary v1").unwrap();
    let target = InstallTarget {
        root: base.join("data"),
        claude_dir: base.join("home/.claude"),
        home: base.join("home"),
    };
    Env {
        _dir: dir,
        target,
        exe,
    }
}

fn install(e: &Env, dry_run: bool) -> Result<String, String> {
    let mut out = String::new();
    run_install(&e.target, dry_run, &e.exe, Lang::En, &mut out).map(|()| out)
}

fn uninstall(e: &Env, dry_run: bool) -> String {
    let mut out = String::new();
    run_uninstall(&e.target, dry_run, Lang::En, &mut out).unwrap();
    out
}

#[test]
fn install_copies_binary_writes_hooks_guide_link_and_record() {
    let e = env();
    let link = e.target.home.join(".local/bin/hippo");
    let installed = e.target.root.join("bin/hippo");

    let dry = install(&e, true).unwrap();
    assert!(dry.contains("(dry-run) Would link"), "{dry}");
    assert!(!installed.exists() && fs::symlink_metadata(&link).is_err());
    assert!(!e.target.claude_dir.exists(), "dry-run은 쓰지 않음");

    let out = install(&e, false).unwrap();
    assert_eq!(fs::read(&installed).unwrap(), b"binary v1");
    assert!(is_installed(&e.target.claude_dir.join("settings.json")));
    let memory = fs::read_to_string(e.target.claude_dir.join("CLAUDE.md")).unwrap();
    assert!(
        memory.contains("/data/bin/hippo search"),
        "안내 블록은 설치 경로: {memory}"
    );
    assert_eq!(fs::read_link(&link).unwrap(), installed);
    assert_eq!(
        fs::read_to_string(e.target.root.join("bin/claude-dir")).unwrap(),
        format!("{}\n", e.target.claude_dir.display())
    );
    assert!(out.contains("Linked:"), "{out}");

    let again = install(&e, false).unwrap();
    assert!(
        again.contains("Binary up to date") && again.contains("Link up to date"),
        "{again}"
    );
}

#[test]
fn install_refuses_foreign_path_link_before_changing_anything() {
    let e = env();
    let link = e.target.home.join(".local/bin/hippo");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("/elsewhere/bin/hippo", &link).unwrap();
    assert!(install(&e, false).is_err());
    assert!(!e.target.root.join("bin/hippo").exists(), "복사하지 않음");
    assert!(
        !e.target.claude_dir.exists(),
        "hook과 안내 블록도 쓰지 않음"
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        PathBuf::from("/elsewhere/bin/hippo")
    );
}

#[test]
fn uninstall_keeps_binary_and_removes_hooks_guide_link_and_record() {
    let e = env();
    install(&e, false).unwrap();
    let link = e.target.home.join(".local/bin/hippo");
    let dry = uninstall(&e, true);
    assert!(dry.contains("(dry-run) Would remove link"), "{dry}");
    assert!(fs::symlink_metadata(&link).is_ok());

    let out = uninstall(&e, false);
    assert!(out.contains("Removed link"), "{out}");
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(!e.target.claude_dir.join("settings.json").exists());
    assert!(!e.target.claude_dir.join("CLAUDE.md").exists());
    assert!(
        e.target.root.join("bin/hippo").exists(),
        "실행 파일은 남김 (FR-INS-9)"
    );
    assert!(!e.target.root.join("bin/claude-dir").exists());
    let none = uninstall(&e, false);
    assert!(none.contains("No change"), "{none}");
}

#[test]
fn uninstall_leaves_link_that_points_elsewhere() {
    let e = env();
    install(&e, false).unwrap();
    let link = e.target.home.join(".local/bin/hippo");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("/elsewhere/bin/hippo", &link).unwrap();
    let out = uninstall(&e, false);
    assert!(out.contains("does not point to this install"), "{out}");
    assert!(fs::symlink_metadata(&link).is_ok());
}

#[test]
fn purge_data_deletes_root_without_daemon() {
    let e = env();
    install(&e, false).unwrap();
    fs::create_dir_all(e.target.root.join("projects/x")).unwrap();
    uninstall(&e, false);
    let mut out = String::new();
    purge_data(&e.target.root, &e.target.home, Lang::En, &mut out).unwrap();
    assert!(!e.target.root.exists(), "{out}");
    assert!(out.contains("Deleted data root"), "{out}");
}

#[test]
fn stop_without_daemon_or_ollama_changes_nothing() {
    let e = env();
    fs::create_dir_all(&e.target.root).unwrap();
    let mut out = String::new();
    stop_services(&e.target.root, Lang::En, &mut out).unwrap();
    assert!(out.contains("Nothing to stop"), "{out}");
    assert!(e.target.root.exists());
}
