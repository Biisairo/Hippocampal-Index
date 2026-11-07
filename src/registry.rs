//! 목록 DB: 프로젝트, 위치, 루트 커밋, 세션 고정값. (FR-PRJ-1, 4, 5, 7, 8, 9)

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::fsutil::{ensure_private_dir, open_private_append, set_private_file};
use crate::project::{Location, LocationKind, normalize};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("project not found: {0}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, RegistryError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub location: String,
    pub kind: LocationKind,
    pub created_at: i64,
    pub last_active_at: i64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY, location TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
    created_at INTEGER NOT NULL, last_active_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS project_roots (
    root_commit TEXT NOT NULL, project_id TEXT NOT NULL, PRIMARY KEY (root_commit, project_id)
);
CREATE TABLE IF NOT EXISTS session_projects (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL);
";

const PROJECT_COLUMNS: &str = "id, location, kind, created_at, last_active_at";
const PROJECT_ID_BYTES: usize = 16;
pub const GIT_DIR_NAME: &str = ".git";
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 판정 결과. 위치를 바꿔야 하는 규칙은 (b)와 (c)이다.
enum Match {
    Same(String),
    Takeover(String),
}

pub struct Registry {
    conn: Connection,
}

impl Registry {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            ensure_private_dir(dir)?;
        }
        open_private_append(path)?;
        set_private_file(path)?;
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// 기록을 포함한 판정. (FR-PRJ-4)
    pub fn resolve(&mut self, loc: &Location, now: i64) -> Result<String> {
        let tx = self.conn.transaction()?;
        let id = match find(&tx, loc)? {
            Some(Match::Same(id)) => id,
            Some(Match::Takeover(id)) => {
                set_location(&tx, &id, loc)?;
                id
            }
            None => {
                let id = new_project_id()?;
                tx.execute(
                    "INSERT INTO projects (id, location, kind, created_at, last_active_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![id, loc.path, loc.kind.as_str(), now],
                )?;
                id
            }
        };
        add_roots(&tx, &id, &loc.root_commits)?;
        tx.commit()?;
        Ok(id)
    }

    /// 조회 전용 판정. 목록 DB를 바꾸지 않는다. (FR-PRJ-5)
    pub fn lookup(&self, loc: &Location) -> Result<Option<String>> {
        Ok(find(&self.conn, loc)?.map(|m| match m {
            Match::Same(id) | Match::Takeover(id) => id,
        }))
    }

    pub fn session_project(&self, session_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT project_id FROM session_projects WHERE session_id = ?1",
                [session_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn bind_session(&mut self, session_id: &str, project_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO session_projects (session_id, project_id) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET project_id = excluded.project_id",
            params![session_id, project_id],
        )?;
        Ok(())
    }

    pub fn touch(&mut self, project_id: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET last_active_at = ?2 WHERE id = ?1",
            params![project_id, now],
        )?;
        Ok(())
    }

    /// 마지막 활동 시각이 최근인 순서. (FR-PRJ-6)
    pub fn projects(&self) -> Result<Vec<ProjectRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY last_active_at DESC, created_at DESC, id"
        ))?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn project(&self, id: &str) -> Result<Option<ProjectRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"),
                [id],
                row_to_project,
            )
            .optional()?)
    }

    pub fn root_commits(&self, id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT root_commit FROM project_roots WHERE project_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 가져오기의 원본: 프로젝트 id, 또는 위치가 경로나 `<경로>/.git`인 프로젝트. (FR-PRJ-7)
    pub fn find_source(&self, arg: &str) -> Result<Option<String>> {
        if is_project_id(arg) && self.project(arg)?.is_some() {
            return Ok(Some(arg.to_string()));
        }
        let path = normalize(Path::new(arg));
        let git_dir = path.join(GIT_DIR_NAME);
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM projects WHERE location = ?1 OR location = ?2",
                params![path.to_string_lossy(), git_dir.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 위치를 바꾸고 루트 커밋을 추가한다. 이전 위치는 남기지 않는다. (FR-PRJ-7 a)
    pub fn relocate(&mut self, project_id: &str, loc: &Location) -> Result<()> {
        let tx = self.conn.transaction()?;
        if set_location(&tx, project_id, loc)? == 0 {
            return Err(RegistryError::NotFound(project_id.to_string()));
        }
        add_roots(&tx, project_id, &loc.root_commits)?;
        tx.commit()?;
        Ok(())
    }

    /// 원본의 세션 고정값과 루트 커밋을 대상으로 옮기고 원본을 지운다. (FR-PRJ-9 3)
    pub fn absorb(&mut self, source: &str, target: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE session_projects SET project_id = ?2 WHERE project_id = ?1",
            params![source, target],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO project_roots (root_commit, project_id)
             SELECT root_commit, ?2 FROM project_roots WHERE project_id = ?1",
            params![source, target],
        )?;
        tx.execute("DELETE FROM project_roots WHERE project_id = ?1", [source])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [source])?;
        tx.commit()?;
        Ok(())
    }

    /// 프로젝트와 그 루트 커밋, 세션 고정값을 지운다. 프로젝트가 없었으면 false. (FR-DEL-1)
    pub fn remove_project(&mut self, id: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM session_projects WHERE project_id = ?1", [id])?;
        tx.execute("DELETE FROM project_roots WHERE project_id = ?1", [id])?;
        let removed = tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// 세션이 그 프로젝트에 고정되어 있으면 고정값을 지운다. (FR-DEL-2)
    pub fn unbind_session(&mut self, session_id: &str, project_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM session_projects WHERE session_id = ?1 AND project_id = ?2",
            params![session_id, project_id],
        )?;
        Ok(())
    }
}

fn row_to_project(r: &rusqlite::Row) -> rusqlite::Result<ProjectRow> {
    let kind: String = r.get(2)?;
    Ok(ProjectRow {
        id: r.get(0)?,
        location: r.get(1)?,
        kind: LocationKind::parse(&kind).unwrap_or(LocationKind::Dir),
        created_at: r.get(3)?,
        last_active_at: r.get(4)?,
    })
}

/// FR-PRJ-4의 (a)~(c). 맞는 규칙이 없으면 `None`.
fn find(conn: &Connection, loc: &Location) -> Result<Option<Match>> {
    let same: Option<String> = conn
        .query_row(
            "SELECT id FROM projects WHERE location = ?1",
            [&loc.path],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = same {
        return Ok(Some(Match::Same(id)));
    }
    if loc.kind != LocationKind::Git {
        return Ok(None);
    }
    if let Some(toplevel) = &loc.toplevel {
        let initialized: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE location = ?1 AND kind = ?2",
                params![toplevel, LocationKind::Dir.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = initialized {
            return Ok(Some(Match::Takeover(id)));
        }
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT p.id, p.location, p.last_active_at FROM projects p
         JOIN project_roots r ON r.project_id = p.id
         WHERE r.root_commit = ?1",
    )?;
    let mut candidates: Vec<(String, String, i64)> = Vec::new();
    for root in &loc.root_commits {
        let rows = stmt.query_map([root], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        for row in rows {
            let row = row?;
            if !candidates.iter().any(|c| c.0 == row.0) {
                candidates.push(row);
            }
        }
    }
    candidates.sort_by_key(|c| std::cmp::Reverse(c.2));
    Ok(candidates
        .into_iter()
        .find(|(_, location, _)| !Path::new(location).exists())
        .map(|(id, _, _)| Match::Takeover(id)))
}

fn set_location(tx: &Transaction, id: &str, loc: &Location) -> Result<usize> {
    Ok(tx.execute(
        "UPDATE projects SET location = ?2, kind = ?3 WHERE id = ?1",
        params![id, loc.path, loc.kind.as_str()],
    )?)
}

fn add_roots(tx: &Transaction, id: &str, roots: &[String]) -> Result<()> {
    for root in roots {
        tx.execute(
            "INSERT OR IGNORE INTO project_roots (root_commit, project_id) VALUES (?1, ?2)",
            params![root, id],
        )?;
    }
    Ok(())
}

pub fn is_project_id(s: &str) -> bool {
    s.len() == PROJECT_ID_BYTES * 2 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 프로젝트 위치의 폴더 경로. 위치가 `<경로>/.git`이면 `<경로>`이다. (FR-IMP-2, FR-GUI-7)
pub fn project_folder(location: &str) -> PathBuf {
    let location = Path::new(location);
    match (location.file_name(), location.parent()) {
        (Some(name), Some(parent)) if name == GIT_DIR_NAME => parent.to_path_buf(),
        _ => location.to_path_buf(),
    }
}

/// 무작위 128비트 값의 소문자 hex. (DR-1)
fn new_project_id() -> Result<String> {
    let mut bytes = [0u8; PROJECT_ID_BYTES];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
