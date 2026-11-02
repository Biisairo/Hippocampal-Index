use std::fs;

use smem::generations::{activate, active_dir, cleanup_inactive, new_generation};

#[test]
fn first_call_creates_and_points_to_initial_generation() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("vectors/default");
    let first = active_dir(&base).unwrap();
    assert!(first.is_dir());
    assert_eq!(first.parent().unwrap(), base);
    assert_eq!(active_dir(&base).unwrap(), first, "같은 세대를 계속 가리킴");
    assert!(base.join("ACTIVE").is_file());
}

#[test]
fn activate_switches_and_cleanup_removes_others() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("v");
    let first = active_dir(&base).unwrap();
    let second = new_generation(&base).unwrap();
    assert_ne!(first, second);
    assert!(second.is_dir());
    assert_eq!(
        active_dir(&base).unwrap(),
        first,
        "만들기만 해서는 전환되지 않음"
    );
    activate(&base, &second).unwrap();
    assert_eq!(active_dir(&base).unwrap(), second);
    let removed = cleanup_inactive(&base).unwrap();
    assert_eq!(removed, vec![first.clone()]);
    assert!(!first.exists());
    assert!(second.exists());
    assert!(base.join("ACTIVE").is_file(), "포인터 파일은 남김");
}

#[test]
fn pointer_to_missing_generation_falls_back_to_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("v");
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("ACTIVE"), "gone").unwrap();
    let active = active_dir(&base).unwrap();
    assert!(active.is_dir());
    assert_ne!(active.file_name().unwrap(), "gone");
}

#[test]
fn pointer_cannot_escape_base() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("v");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(dir.path().join("outside")).unwrap();
    fs::write(base.join("ACTIVE"), "../outside").unwrap();
    let active = active_dir(&base).unwrap();
    assert_eq!(active.parent().unwrap(), base);
    assert!(
        activate(&base, &dir.path().join("outside")).is_err(),
        "base 밖 세대는 활성화 불가"
    );
}
