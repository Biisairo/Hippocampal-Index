use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use hippo::layout::{
    InstallState, LinkState, OLLAMA_PID, create_link, daemon_lock_held, default_claude_dir,
    default_gui_root, forget_claude_dir, install_state, is_app_exe, link_state, path_has_dir,
    path_link, read_claude_dir, record_claude_dir, remove_link, remove_root, root_from_link,
    root_of_exe, stop_ollama,
};

fn real(dir: &Path) -> PathBuf {
    fs::canonicalize(dir).unwrap()
}

fn exe_at(path: &Path) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"bin").unwrap();
    path.to_path_buf()
}

// ---- FR-ROOT-1 ----

#[test]
fn root_comes_from_bin_hippo_after_resolving_links() {
    let dir = tempfile::tempdir().unwrap();
    let base = real(dir.path());
    let exe = exe_at(&base.join("data/bin/hippo"));
    assert_eq!(root_of_exe(&exe), Some(base.join("data")));

    let link = base.join("links/hippo");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&exe, &link).unwrap();
    assert_eq!(
        root_of_exe(&link),
        Some(base.join("data")),
        "PATH 링크로 실행해도 설치 경로를 봄"
    );

    for other in ["build/hippo", "data/bin/hippo-old", "data/sbin/hippo"] {
        let p = exe_at(&base.join(other));
        assert_eq!(root_of_exe(&p), None, "{other}");
    }
    assert_eq!(root_of_exe(&base.join("missing/bin/hippo")), None);
}

// ---- IF-CLI-14 ----

#[test]
fn app_executable_is_inside_bundle_macos_dir() {
    let dir = tempfile::tempdir().unwrap();
    let base = real(dir.path());
    let app = exe_at(&base.join("hippo.app/Contents/MacOS/hippo"));
    assert!(is_app_exe(&app));
    let link = base.join("hippo-link");
    symlink(&app, &link).unwrap();
    assert!(is_app_exe(&link), "링크를 풀어서 판정");
    for other in [
        "bin/hippo",
        "hippo.app/Contents/Resources/hippo",
        "x/Contents/MacOS/hippo",
    ] {
        assert!(!is_app_exe(&exe_at(&base.join(other))), "{other}");
    }
}

// ---- FR-INS-11 ----

#[test]
fn path_link_is_created_kept_refused_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let home = real(dir.path());
    let link = path_link(&home);
    assert_eq!(link, home.join(".local/bin/hippo"));
    let target = exe_at(&home.join("data/bin/hippo"));
    let other = exe_at(&home.join("other/bin/hippo"));

    assert_eq!(link_state(&link, &target), LinkState::Missing);
    assert!(create_link(&link, &target).unwrap(), "없으면 만듦");
    assert_eq!(
        fs::metadata(link.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(fs::read_link(&link).unwrap(), target);
    assert_eq!(link_state(&link, &target), LinkState::Ours);
    assert!(!create_link(&link, &target).unwrap(), "같은 대상이면 유지");

    assert_eq!(link_state(&link, &other), LinkState::Foreign);
    assert!(create_link(&link, &other).is_err(), "다른 대상이면 거부");
    assert!(
        !remove_link(&link, &other).unwrap(),
        "남의 링크는 지우지 않음"
    );
    assert!(link.exists());
    assert!(remove_link(&link, &target).unwrap());
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(!remove_link(&link, &target).unwrap(), "없으면 그대로");

    fs::write(&link, b"plain").unwrap();
    assert_eq!(link_state(&link, &target), LinkState::Foreign, "일반 파일");
    assert!(create_link(&link, &target).is_err());
    assert!(!remove_link(&link, &target).unwrap());
    assert_eq!(fs::read(&link).unwrap(), b"plain");
}

#[test]
fn path_check_finds_directory_in_path_variable() {
    let dir = Path::new("/h/.local/bin");
    let path = |s: &str| OsString::from(s);
    assert!(path_has_dir(&path("/usr/bin:/h/.local/bin"), dir));
    assert!(path_has_dir(&path("/h/.local/bin/"), dir), "끝 / 무시");
    assert!(!path_has_dir(&path("/usr/bin:/h/.local"), dir));
    assert!(!path_has_dir(&path(""), dir));
}

// ---- FR-INS-12 ----

#[test]
fn claude_dir_record_is_written_replaced_and_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    let home = dir.path().join("home");
    assert_eq!(read_claude_dir(&root), None);
    assert_eq!(default_claude_dir(&root, &home), home.join(".claude"));

    let a = dir.path().join("a/.claude");
    record_claude_dir(&root, &a).unwrap();
    let file = root.join("bin/claude-dir");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(read_claude_dir(&root), Some(a.clone()));
    assert_eq!(default_claude_dir(&root, &home), a);

    let b = dir.path().join("b");
    record_claude_dir(&root, &b).unwrap();
    assert_eq!(
        read_claude_dir(&root),
        Some(b.clone()),
        "마지막 값으로 바꿈"
    );

    assert!(!forget_claude_dir(&root, &a).unwrap(), "다른 폴더면 그대로");
    assert_eq!(read_claude_dir(&root), Some(b.clone()));
    assert!(forget_claude_dir(&root, &b).unwrap());
    assert!(!file.exists());

    fs::write(&file, "  \n").unwrap();
    assert_eq!(read_claude_dir(&root), None, "비어 있으면 없음");
}

// ---- FR-GUI-22 ----

#[test]
fn gui_root_prefers_own_root_then_link_then_default() {
    let dir = tempfile::tempdir().unwrap();
    let home = real(dir.path());
    let app = exe_at(&home.join("hippo.app/Contents/MacOS/hippo"));
    assert_eq!(default_gui_root(&app, &home), home.join(".hippo"), "기본값");

    let installed = exe_at(&home.join("custom/bin/hippo"));
    let link = path_link(&home);
    create_link(&link, &installed).unwrap();
    assert_eq!(root_from_link(&link), Some(home.join("custom")));
    assert_eq!(
        default_gui_root(&app, &home),
        home.join("custom"),
        "PATH 링크가 가리키는 루트"
    );

    let own = exe_at(&home.join("own/bin/hippo"));
    assert_eq!(
        default_gui_root(&own, &home),
        home.join("own"),
        "관리 실행 파일이면 그 루트"
    );

    fs::remove_file(&link).unwrap();
    let stray = exe_at(&home.join("stray/hippo"));
    symlink(&stray, &link).unwrap();
    assert_eq!(root_from_link(&link), None);
    assert_eq!(default_gui_root(&app, &home), home.join(".hippo"));
}

// ---- FR-GUI-23 ----

#[test]
fn install_state_compares_gui_binary_with_installed_one() {
    let dir = tempfile::tempdir().unwrap();
    let base = real(dir.path());
    let root = base.join("data");
    let gui = exe_at(&base.join("hippo.app/Contents/MacOS/hippo"));
    assert_eq!(install_state(&gui, &root), InstallState::NeedInstall);
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/hippo"), b"old").unwrap();
    assert_eq!(install_state(&gui, &root), InstallState::NeedUpdate);
    fs::write(root.join("bin/hippo"), b"bin").unwrap();
    assert_eq!(install_state(&gui, &root), InstallState::Installed);
    let own = root.join("bin/hippo");
    assert_eq!(
        install_state(&own, &root),
        InstallState::Installed,
        "설치 경로 자체"
    );
}

// ---- FR-INS-13, FR-EMB-4 ----

#[test]
fn remove_root_deletes_tree_but_refuses_dangerous_paths() {
    let dir = tempfile::tempdir().unwrap();
    let home = real(dir.path());
    let root = home.join("data");
    fs::create_dir_all(root.join("projects/x")).unwrap();
    fs::write(root.join("projects/x/records.sqlite"), b"x").unwrap();
    exe_at(&root.join("bin/hippo"));

    let link = home.join("data-link");
    symlink(&root, &link).unwrap();
    assert!(remove_root(&link, &home).is_err(), "링크 거부");
    assert!(remove_root(&home, &home).is_err(), "HOME 거부");
    assert!(remove_root(Path::new("/"), &home).is_err(), "/ 거부");
    assert!(root.exists());

    remove_root(&root, &home).unwrap();
    assert!(!root.exists());
    remove_root(&root, &home).unwrap();
}

#[test]
fn stop_ollama_ignores_missing_or_foreign_pid() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(!stop_ollama(root), "pid 파일 없음");
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    fs::write(root.join(OLLAMA_PID), child.id().to_string()).unwrap();
    assert!(!stop_ollama(root), "이름이 ollama가 아니면 종료하지 않음");
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
    fs::write(root.join(OLLAMA_PID), "garbage").unwrap();
    assert!(!stop_ollama(root));
}

// ---- FR-DMN-12: 데몬 프로세스가 끝났는지 ----

#[test]
fn daemon_lock_shows_whether_daemon_process_is_alive() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(!daemon_lock_held(root), "잠금 파일 없음");
    let file = fs::File::create(root.join("hippo.lock")).unwrap();
    assert!(!daemon_lock_held(root), "파일만 있고 잠금 없음");
    file.try_lock().unwrap();
    assert!(
        daemon_lock_held(root),
        "다른 프로세스(여기서는 다른 열린 파일)가 쥐고 있음"
    );
    file.unlock().unwrap();
    assert!(!daemon_lock_held(root), "풀림");
}
