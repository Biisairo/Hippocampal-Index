//! 경로 정규화와 git 정보. (FR-PRJ-2, FR-PRJ-3)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use hippo::project::{LocationKind, locate, normalize};

const TIMEOUT: Duration = Duration::from_secs(5);

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn repo_with_commit(dir: &Path) -> String {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(dir, &["rev-parse", "HEAD"])
}

fn real(p: &Path) -> String {
    fs::canonicalize(p).unwrap().to_string_lossy().into_owned()
}

#[test]
fn normalize_resolves_symlinks_and_keeps_missing_paths() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(normalize(&link), fs::canonicalize(&target).unwrap());
    let missing = PathBuf::from("/no/such/hippo/path");
    assert_eq!(normalize(&missing), missing);
}

#[test]
fn plain_directory_is_its_own_location() {
    let dir = tempfile::tempdir().unwrap();
    let loc = locate(dir.path(), TIMEOUT);
    assert_eq!(loc.kind, LocationKind::Dir);
    assert_eq!(loc.path, real(dir.path()));
    assert_eq!(loc.toplevel, None);
    assert!(loc.root_commits.is_empty());
}

#[test]
fn missing_path_is_a_directory_location_as_given() {
    let loc = locate(Path::new("/no/such/hippo/path"), TIMEOUT);
    assert_eq!(loc.kind, LocationKind::Dir);
    assert_eq!(loc.path, "/no/such/hippo/path");
}

#[test]
fn git_subdirectory_resolves_to_common_dir_and_root_commit() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let root = repo_with_commit(&repo);
    let sub = repo.join("src/deep");
    fs::create_dir_all(&sub).unwrap();
    let loc = locate(&sub, TIMEOUT);
    assert_eq!(loc.kind, LocationKind::Git);
    assert_eq!(loc.path, real(&repo.join(".git")));
    assert_eq!(loc.toplevel.as_deref(), Some(real(&repo).as_str()));
    assert_eq!(loc.root_commits, vec![root]);
}

#[test]
fn worktree_shares_the_location_of_the_main_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    repo_with_commit(&repo);
    let wt = dir.path().join("wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            wt.to_str().unwrap(),
        ],
    );
    let main = locate(&repo, TIMEOUT);
    let other = locate(&wt, TIMEOUT);
    assert_eq!(other.kind, LocationKind::Git);
    assert_eq!(other.path, main.path);
    assert_eq!(other.toplevel.as_deref(), Some(real(&wt).as_str()));
    assert_eq!(other.root_commits, main.root_commits);
}

#[test]
fn repository_without_commits_has_no_root_commits() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    let loc = locate(dir.path(), TIMEOUT);
    assert_eq!(loc.kind, LocationKind::Git);
    assert_eq!(loc.path, real(&dir.path().join(".git")));
    assert!(loc.root_commits.is_empty());
}

#[test]
fn moved_repository_keeps_root_commit_but_changes_location() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    let root = repo_with_commit(&before);
    let old = locate(&before, TIMEOUT);
    let after = dir.path().join("after");
    fs::rename(&before, &after).unwrap();
    let new = locate(&after, TIMEOUT);
    assert_ne!(new.path, old.path);
    assert_eq!(new.root_commits, vec![root]);
}
