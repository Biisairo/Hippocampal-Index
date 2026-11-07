use std::fs;

use hippo::logfile::{ROTATE_BYTES, append_line};

fn is_stamped(line: &str, message: &str) -> bool {
    let (stamp, rest) = line.split_at(19.min(line.len()));
    let shape = stamp.bytes().enumerate().all(|(i, b)| match i {
        4 | 7 => b == b'-',
        10 => b == b' ',
        13 | 16 => b == b':',
        _ => b.is_ascii_digit(),
    });
    shape && rest == format!(" {message}")
}

#[test]
fn line_starts_with_local_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hippo.log");
    append_line(&path, "first");
    append_line(&path, "second");
    let text = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(is_stamped(lines[0], "first"), "{}", lines[0]);
    assert!(is_stamped(lines[1], "second"), "{}", lines[1]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn file_over_limit_is_rotated_to_dot_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hook.log");
    let rotated = dir.path().join("hook.log.1");
    fs::write(&rotated, "old backup\n").unwrap();
    let big = "x".repeat(ROTATE_BYTES as usize);
    fs::write(&path, &big).unwrap();
    append_line(&path, "after");
    assert_eq!(
        fs::read_to_string(&rotated).unwrap(),
        big,
        "기존 .1은 바꿔 씀"
    );
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(is_stamped(text.lines().next().unwrap(), "after"));
}

#[test]
fn file_under_limit_is_not_rotated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hippo.log");
    fs::write(&path, "x".repeat(ROTATE_BYTES as usize - 1)).unwrap();
    append_line(&path, "kept");
    assert!(!dir.path().join("hippo.log.1").exists());
}

#[test]
fn missing_directory_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    append_line(&dir.path().join("none/hippo.log"), "lost");
}
