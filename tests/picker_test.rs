use std::fs;
use std::path::{Path, PathBuf};

use hippo::gui::picker::{folder_script, parse_folder, start_folder};

// ---- FR-GUI-24 ----

#[test]
fn start_folder_uses_existing_folder_or_home() {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let home = base.join("home");
    let cwd = base.join("work");
    fs::create_dir_all(home.join("docs")).unwrap();
    fs::create_dir_all(cwd.join("sub")).unwrap();

    let abs = cwd.join("sub");
    assert_eq!(start_folder(&abs.to_string_lossy(), &cwd, &home), abs);
    assert_eq!(
        start_folder("sub", &cwd, &home),
        cwd.join("sub"),
        "상대 경로"
    );
    assert_eq!(
        start_folder("  sub  ", &cwd, &home),
        cwd.join("sub"),
        "앞뒤 공백"
    );
    assert_eq!(start_folder("~/docs", &cwd, &home), home.join("docs"), "~");
    assert_eq!(start_folder("~", &cwd, &home), home);
    for missing in ["", "project", "/no/such/dir", "~/none"] {
        assert_eq!(start_folder(missing, &cwd, &home), home, "{missing:?}");
    }
}

#[test]
fn script_escapes_quotes_and_backslashes() {
    let script = folder_script("Pick \"it\"", Path::new("/a b/c\\d\"e"));
    assert_eq!(
        script,
        r#"POSIX path of (choose folder with prompt "Pick \"it\"" default location (POSIX file "/a b/c\\d\"e") with invisibles)"#
    );
}

#[test]
fn output_is_trimmed_and_cancel_is_none() {
    assert_eq!(
        parse_folder(true, "/Users/me/work/\n"),
        Some(PathBuf::from("/Users/me/work"))
    );
    assert_eq!(parse_folder(true, "/\n"), Some(PathBuf::from("/")), "루트");
    assert_eq!(parse_folder(true, "\n"), None, "빈 출력");
    assert_eq!(parse_folder(false, "/Users/me/\n"), None, "취소나 실패");
}
