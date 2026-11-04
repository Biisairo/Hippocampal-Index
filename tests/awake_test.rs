use std::time::{Duration, Instant};

use hippo::awake::SleepGuard;

const WAIT: Duration = Duration::from_secs(5);

fn sleeper() -> SleepGuard {
    SleepGuard::new("sleep", &["30"])
}

/// 프로세스가 있으면(좀비 제외 전 포함) true.
fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn gone_within(pid: u32) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn holds_while_busy_and_releases_when_idle() {
    let mut guard = sleeper();
    assert_eq!(guard.update(true), None);
    let pid = guard.pid().expect("busy면 자식 프로세스가 있어야 함");
    assert!(alive(pid));
    assert_eq!(guard.update(true), None);
    assert_eq!(guard.pid(), Some(pid), "이미 잡고 있으면 다시 띄우지 않음");
    assert_eq!(guard.update(false), None);
    assert_eq!(guard.pid(), None);
    assert!(gone_within(pid));
}

#[test]
fn idle_without_hold_spawns_nothing() {
    let mut guard = sleeper();
    assert_eq!(guard.update(false), None);
    assert_eq!(guard.pid(), None);
}

#[test]
fn respawns_when_child_exits() {
    let mut guard = sleeper();
    guard.update(true);
    let first = guard.pid().unwrap();
    unsafe {
        libc::kill(first as i32, libc::SIGKILL);
    }
    let deadline = Instant::now() + WAIT;
    loop {
        guard.update(true);
        if guard.pid().is_some_and(|p| p != first) {
            break;
        }
        assert!(Instant::now() < deadline, "다시 띄우지 않음");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(alive(guard.pid().unwrap()));
}

#[test]
fn spawn_failure_is_reported_once() {
    let mut guard = SleepGuard::new("/nonexistent/hippo-caffeinate", &[]);
    assert!(guard.update(true).is_some());
    assert_eq!(guard.pid(), None);
    assert_eq!(guard.update(true), None, "같은 실패는 한 번만 알림");
    assert_eq!(guard.update(false), None);
}

#[test]
fn drop_releases_hold() {
    let mut guard = sleeper();
    guard.update(true);
    let pid = guard.pid().unwrap();
    drop(guard);
    assert!(gone_within(pid));
}
