//! 프로젝트 위치: 경로 정규화와 git 정보. (SRS 3.2.10, FR-PRJ-2, FR-PRJ-3)

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GIT_POLL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    Git,
    Dir,
}

impl LocationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Dir => "dir",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "git" => Some(Self::Git),
            "dir" => Some(Self::Dir),
            _ => None,
        }
    }
}

/// 프로젝트 위치. git 저장소면 git common dir, 아니면 경로 자체이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub kind: LocationKind,
    /// git 작업 트리 최상위. git이 아니면 없다.
    pub toplevel: Option<String>,
    pub root_commits: Vec<String>,
}

/// 심볼릭 링크를 풀어 절대 경로로 바꾼다. 경로가 없으면 그대로 돌려준다. (FR-PRJ-2)
pub fn normalize(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 경로의 프로젝트 위치를 구한다. git 실행이 실패하거나 시간 제한을 넘으면 git이 아닌 경로로 본다. (FR-PRJ-3, A-5)
pub fn locate(path: &Path, git_timeout: Duration) -> Location {
    let path = normalize(path);
    let revs = run_git(
        &path,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ],
        git_timeout,
    );
    let mut lines = revs.as_deref().unwrap_or_default().lines();
    match (lines.next(), lines.next()) {
        (Some(toplevel), Some(common_dir)) => Location {
            path: lossy(&normalize(Path::new(common_dir))),
            kind: LocationKind::Git,
            toplevel: Some(lossy(&normalize(Path::new(toplevel)))),
            root_commits: run_git(&path, &["rev-list", "--max-parents=0", "HEAD"], git_timeout)
                .map(|out| out.lines().map(str::to_string).collect())
                .unwrap_or_default(),
        },
        _ => Location {
            path: lossy(&path),
            kind: LocationKind::Dir,
            toplevel: None,
            root_commits: Vec::new(),
        },
    }
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// git을 실행해 성공하면 stdout을 돌려준다. 시간 제한을 넘기면 프로세스를 끝내고 `None`.
fn run_git(dir: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(GIT_POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}
