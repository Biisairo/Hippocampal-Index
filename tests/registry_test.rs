//! 목록 DB의 판정과 기록. (FR-PRJ-4, 5, 7, 8, 9)

use std::fs;

use hippo::project::{Location, LocationKind};
use hippo::registry::Registry;

fn dir_loc(path: &str) -> Location {
    Location {
        path: path.into(),
        kind: LocationKind::Dir,
        toplevel: None,
        root_commits: vec![],
    }
}

fn git_loc(common_dir: &str, toplevel: &str, roots: &[&str]) -> Location {
    Location {
        path: common_dir.into(),
        kind: LocationKind::Git,
        toplevel: Some(toplevel.into()),
        root_commits: roots.iter().map(|r| (*r).to_string()).collect(),
    }
}

fn is_project_id(id: &str) -> bool {
    id.len() == 32
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[test]
fn new_location_creates_project_and_same_location_reuses_it() {
    let mut r = Registry::open_in_memory().unwrap();
    let a = r.resolve(&dir_loc("/gone/a"), 1).unwrap();
    assert!(is_project_id(&a), "{a}");
    assert_eq!(r.resolve(&dir_loc("/gone/a"), 2).unwrap(), a);
    let b = r.resolve(&dir_loc("/gone/b"), 3).unwrap();
    assert_ne!(a, b);
    let row = r.project(&a).unwrap().unwrap();
    assert_eq!(
        (row.location.as_str(), row.kind),
        ("/gone/a", LocationKind::Dir)
    );
    assert_eq!(row.created_at, 1);
}

#[test]
fn worktrees_share_a_project_through_common_dir() {
    let mut r = Registry::open_in_memory().unwrap();
    let main = r
        .resolve(&git_loc("/gone/repo/.git", "/gone/repo", &["c1"]), 1)
        .unwrap();
    let wt = r
        .resolve(&git_loc("/gone/repo/.git", "/gone/wt", &["c1"]), 2)
        .unwrap();
    assert_eq!(main, wt);
    assert_eq!(r.projects().unwrap().len(), 1);
    assert_eq!(
        r.project(&main).unwrap().unwrap().location,
        "/gone/repo/.git"
    );
}

#[test]
fn git_init_in_known_directory_takes_over_its_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let mut r = Registry::open_in_memory().unwrap();
    let id = r.resolve(&dir_loc(&path), 1).unwrap();
    let common = format!("{path}/.git");
    let got = r.resolve(&git_loc(&common, &path, &[]), 2).unwrap();
    assert_eq!(got, id);
    let row = r.project(&id).unwrap().unwrap();
    assert_eq!(
        (row.location.as_str(), row.kind),
        (common.as_str(), LocationKind::Git)
    );
}

#[test]
fn moved_repository_is_followed_by_root_commit_when_old_location_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let new_common = dir.path().join(".git");
    fs::create_dir(&new_common).unwrap();
    let new_common = new_common.to_string_lossy().into_owned();
    let mut r = Registry::open_in_memory().unwrap();
    let id = r
        .resolve(&git_loc("/gone/old/.git", "/gone/old", &["c1"]), 1)
        .unwrap();
    let got = r
        .resolve(&git_loc(&new_common, "/new", &["c1", "c2"]), 2)
        .unwrap();
    assert_eq!(got, id);
    assert_eq!(r.projects().unwrap().len(), 1);
    assert_eq!(r.project(&id).unwrap().unwrap().location, new_common);
    let mut roots = r.root_commits(&id).unwrap();
    roots.sort();
    assert_eq!(roots, ["c1", "c2"]);
    assert_eq!(
        r.resolve(&git_loc("/gone/old/.git", "/gone/old", &["c1"]), 3)
            .unwrap()
            .len(),
        32
    );
    assert_eq!(r.projects().unwrap().len(), 2, "이전 위치는 남기지 않는다");
}

#[test]
fn second_clone_with_existing_original_is_a_separate_project() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join(".git");
    fs::create_dir(&original).unwrap();
    let original = original.to_string_lossy().into_owned();
    let mut r = Registry::open_in_memory().unwrap();
    let a = r.resolve(&git_loc(&original, "/x", &["c1"]), 1).unwrap();
    let b = r
        .resolve(&git_loc("/gone/clone/.git", "/gone/clone", &["c1"]), 2)
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(r.project(&a).unwrap().unwrap().location, original);
}

#[test]
fn move_picks_most_recently_active_candidate() {
    let mut r = Registry::open_in_memory().unwrap();
    let older = r
        .resolve(&git_loc("/gone/a/.git", "/gone/a", &["c1"]), 1)
        .unwrap();
    let newer = r
        .resolve(&git_loc("/gone/b/.git", "/gone/b", &["c9", "c1"]), 2)
        .unwrap();
    r.touch(&older, 5).unwrap();
    r.touch(&newer, 9).unwrap();
    let got = r
        .resolve(&git_loc("/gone/c/.git", "/gone/c", &["c1"]), 10)
        .unwrap();
    assert_eq!(got, newer);
}

#[test]
fn lookup_applies_rules_without_writing() {
    let mut r = Registry::open_in_memory().unwrap();
    assert_eq!(r.lookup(&dir_loc("/gone/x")).unwrap(), None);
    assert!(r.projects().unwrap().is_empty());

    let id = r
        .resolve(&git_loc("/gone/old/.git", "/gone/old", &["c1"]), 1)
        .unwrap();
    assert_eq!(
        r.lookup(&git_loc("/gone/old/.git", "/gone/wt", &["c1"]))
            .unwrap(),
        Some(id.clone())
    );
    assert_eq!(
        r.lookup(&git_loc("/gone/new/.git", "/gone/new", &["c1"]))
            .unwrap(),
        Some(id.clone())
    );
    assert_eq!(r.project(&id).unwrap().unwrap().location, "/gone/old/.git");
    assert_eq!(r.root_commits(&id).unwrap(), ["c1"]);
}

#[test]
fn session_binding_is_fixed_once_recorded() {
    let mut r = Registry::open_in_memory().unwrap();
    let a = r.resolve(&dir_loc("/gone/a"), 1).unwrap();
    assert_eq!(r.session_project("s1").unwrap(), None);
    r.bind_session("s1", &a).unwrap();
    assert_eq!(r.session_project("s1").unwrap(), Some(a));
}

#[test]
fn projects_are_listed_by_recent_activity() {
    let mut r = Registry::open_in_memory().unwrap();
    let a = r.resolve(&dir_loc("/gone/a"), 1).unwrap();
    let b = r.resolve(&dir_loc("/gone/b"), 2).unwrap();
    r.touch(&a, 10).unwrap();
    let ids: Vec<String> = r.projects().unwrap().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, [a.clone(), b]);
    assert_eq!(r.project(&a).unwrap().unwrap().last_active_at, 10);
}

#[test]
fn find_source_accepts_id_or_path() {
    let mut r = Registry::open_in_memory().unwrap();
    let d = r.resolve(&dir_loc("/gone/notes"), 1).unwrap();
    let g = r
        .resolve(&git_loc("/gone/repo/.git", "/gone/repo", &["c1"]), 2)
        .unwrap();
    assert_eq!(r.find_source(&d).unwrap(), Some(d.clone()));
    assert_eq!(r.find_source("/gone/notes").unwrap(), Some(d));
    assert_eq!(r.find_source("/gone/repo").unwrap(), Some(g.clone()));
    assert_eq!(r.find_source("/gone/repo/.git").unwrap(), Some(g));
    assert_eq!(r.find_source("/gone/other").unwrap(), None);
    assert_eq!(r.find_source(&"0".repeat(32)).unwrap(), None);
}

#[test]
fn relocate_replaces_location_and_adds_roots() {
    let mut r = Registry::open_in_memory().unwrap();
    let id = r.resolve(&dir_loc("/gone/notes"), 1).unwrap();
    r.relocate(&id, &git_loc("/gone/work/.git", "/gone/work", &["c7"]))
        .unwrap();
    let row = r.project(&id).unwrap().unwrap();
    assert_eq!(
        (row.location.as_str(), row.kind),
        ("/gone/work/.git", LocationKind::Git)
    );
    assert_eq!(r.root_commits(&id).unwrap(), ["c7"]);
    assert_eq!(r.lookup(&dir_loc("/gone/notes")).unwrap(), None);
}

#[test]
fn absorb_moves_sessions_and_roots_then_removes_source() {
    let mut r = Registry::open_in_memory().unwrap();
    let src = r
        .resolve(&git_loc("/gone/a/.git", "/gone/a", &["c1"]), 1)
        .unwrap();
    let dst = r
        .resolve(&git_loc("/gone/b/.git", "/gone/b", &["c2"]), 2)
        .unwrap();
    r.bind_session("s1", &src).unwrap();
    r.absorb(&src, &dst).unwrap();
    assert_eq!(r.session_project("s1").unwrap(), Some(dst.clone()));
    assert_eq!(r.project(&src).unwrap(), None);
    let mut roots = r.root_commits(&dst).unwrap();
    roots.sort();
    assert_eq!(roots, ["c1", "c2"]);
    assert_eq!(r.root_commits(&src).unwrap(), Vec::<String>::new());
}

#[test]
fn file_registry_persists_and_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.sqlite");
    let id = {
        let mut r = Registry::open(&path).unwrap();
        r.resolve(&dir_loc("/gone/a"), 1).unwrap()
    };
    let r = Registry::open(&path).unwrap();
    assert_eq!(r.lookup(&dir_loc("/gone/a")).unwrap(), Some(id));
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

// ---- FR-DEL-1, FR-DEL-2 ----

#[test]
fn remove_project_deletes_rows_and_same_location_becomes_new_project() {
    let mut r = Registry::open_in_memory().unwrap();
    let a = r
        .resolve(&git_loc("/gone/a/.git", "/gone/a", &["root-a"]), 1)
        .unwrap();
    let b = r.resolve(&dir_loc("/gone/b"), 2).unwrap();
    r.bind_session("sa", &a).unwrap();
    r.bind_session("sb", &b).unwrap();
    assert!(r.remove_project(&a).unwrap());
    assert!(r.project(&a).unwrap().is_none());
    assert!(r.root_commits(&a).unwrap().is_empty());
    assert_eq!(r.session_project("sa").unwrap(), None);
    assert_eq!(r.session_project("sb").unwrap(), Some(b.clone()));
    assert!(!r.remove_project(&a).unwrap(), "두 번째는 대상 없음");
    let again = r
        .resolve(&git_loc("/gone/a/.git", "/gone/a", &["root-a"]), 3)
        .unwrap();
    assert_ne!(again, a);
}

#[test]
fn unbind_session_removes_only_binding_to_that_project() {
    let mut r = Registry::open_in_memory().unwrap();
    let a = r.resolve(&dir_loc("/gone/a"), 1).unwrap();
    let b = r.resolve(&dir_loc("/gone/b"), 2).unwrap();
    r.bind_session("s", &a).unwrap();
    r.unbind_session("s", &b).unwrap();
    assert_eq!(r.session_project("s").unwrap(), Some(a.clone()));
    r.unbind_session("s", &a).unwrap();
    assert_eq!(r.session_project("s").unwrap(), None);
}

#[test]
fn project_folder_strips_git_dir() {
    use hippo::registry::project_folder;
    use std::path::PathBuf;
    assert_eq!(project_folder("/w/repo/.git"), PathBuf::from("/w/repo"));
    assert_eq!(project_folder("/w/notes"), PathBuf::from("/w/notes"));
    assert_eq!(project_folder("/w/x.git"), PathBuf::from("/w/x.git"));
}
