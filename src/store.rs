//! 원본 저장소(SQLite). 시스템의 기준 데이터를 보관한다. (SRS 3.2.4, DR-1)

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::bg::{BackgroundKind, TaskNotification, background_start};
use crate::embed_text::is_hippo_call;
use crate::transcript::{ExtractorState, Iteration, IterationInput};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// 마스킹을 마친 반복과 그 임베딩 텍스트.
#[derive(Debug, Clone)]
pub struct NewIteration {
    pub iteration: Iteration,
    pub embed_text: String,
}

/// 백그라운드 완료 알림과 읽어 온 결과 원문(마스킹 완료).
#[derive(Debug, Clone)]
pub struct NewCompletion {
    pub session_id: String,
    pub line_uuid: String,
    pub notification: TaskNotification,
    pub result_text: Option<String>,
    pub ts: String,
}

/// 한 트랜잭션으로 반영하는 처리 단위. (FR-STO-10)
#[derive(Debug, Clone)]
pub struct Batch {
    pub session_id: String,
    pub transcript_path: String,
    pub iterations: Vec<NewIteration>,
    pub completions: Vec<NewCompletion>,
    pub state: ExtractorState,
    /// 가져오기로 등록하는 임베딩 작업이면 true. (FR-IMP-5)
    pub bulk: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub is_error: bool,
    pub result_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredIteration {
    pub id: String,
    pub session_id: String,
    pub prompt_id: Option<String>,
    pub message_id: String,
    pub iteration_index: u32,
    pub cwd: String,
    pub ts: String,
    pub inputs: Vec<IterationInput>,
    pub tool_calls: Vec<StoredToolCall>,
    pub response_text: String,
    pub embed_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResult {
    pub id: String,
    pub tool_name: String,
    pub text: String,
    pub first_seen_ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgTask {
    pub tool_use_id: String,
    pub session_id: String,
    pub iteration_id: String,
    pub kind: BackgroundKind,
    pub task_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgCompletion {
    pub line_uuid: String,
    pub tool_use_id: String,
    pub seq: u32,
    pub status: String,
    pub summary: String,
    pub result_id: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecordKind {
    Iteration,
    Result,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        job_kind_str(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EmbedJob {
    pub kind: RecordKind,
    pub ref_id: String,
}

/// 작업과 등록 시각(유닉스 밀리초). (FR-DMN-6)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingJob {
    pub job: EmbedJob,
    pub enqueued_at: i64,
    /// 가져오기·병합으로 등록한 작업. (FR-STO-7)
    pub bulk: bool,
}

/// 병합으로 새로 추가한 반복, 결과, 임베딩 작업 수. (FR-PRJ-9)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub iterations: usize,
    pub results: usize,
    pub jobs: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FtsFilter {
    pub exclude_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FtsHit {
    pub id: String,
    pub rank: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub iterations: u64,
    pub results: u64,
    pub embed_jobs: u64,
}

/// 삭제 대상. 세션은 그 세션 전부, 턴은 같은 `prompt_id`의 반복 전부이다. (FR-DEL-2, FR-DEL-3)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ForgetTarget {
    Session(String),
    Prompt(String),
}

/// 지울(지운) 세션, 턴, 반복, 결과 수. (FR-DEL-4)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetCounts {
    pub sessions: u64,
    pub turns: u64,
    pub iterations: u64,
    pub results: u64,
}

impl ForgetCounts {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 삭제 결과. `jobs`는 지운 임베딩 작업, `vector_refs`는 벡터를 지울 레코드이다. (FR-DEL-5, FR-DEL-6)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgetPlan {
    pub counts: ForgetCounts,
    pub jobs: Vec<EmbedJob>,
    pub vector_refs: Vec<(RecordKind, String)>,
}

/// 기록 탭의 세션 한 줄. (FR-GUI-13)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    /// 첫 반복의 `ts`.
    pub started_at: String,
    /// 공백을 한 칸으로 줄이고 80자로 자른 첫 프롬프트.
    pub first_prompt: String,
    pub turns: u64,
    pub iterations: u64,
}

/// 기록 탭의 턴 한 줄. (FR-GUI-13)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSummary {
    pub prompt_id: Option<String>,
    pub ts: String,
    pub prompt: String,
    pub iterations: u64,
    /// 이 턴의 반복과 연결 결과 중 남은 임베딩 작업 수.
    pub pending: u64,
}

/// 전체 개수와 한 페이지. (FR-GUI-13)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub total: u64,
    pub items: Vec<T>,
}

pub struct RecordStore {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS iterations (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL, prompt_id TEXT, message_id TEXT NOT NULL,
    iteration_index INTEGER NOT NULL, cwd TEXT NOT NULL, ts TEXT NOT NULL, inputs TEXT NOT NULL,
    tool_calls TEXT NOT NULL, response_text TEXT NOT NULL, embed_text TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS iterations_prompt ON iterations(prompt_id, iteration_index);
CREATE INDEX IF NOT EXISTS iterations_message ON iterations(message_id);
CREATE TABLE IF NOT EXISTS results (
    id TEXT PRIMARY KEY, tool_name TEXT NOT NULL, text TEXT NOT NULL, first_seen_ts TEXT NOT NULL,
    indexed INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS iteration_results (
    iteration_id TEXT NOT NULL, tool_use_id TEXT NOT NULL, result_id TEXT NOT NULL,
    PRIMARY KEY (iteration_id, tool_use_id)
);
CREATE INDEX IF NOT EXISTS iteration_results_result ON iteration_results(result_id);
CREATE TABLE IF NOT EXISTS bg_tasks (
    tool_use_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, iteration_id TEXT NOT NULL,
    kind TEXT NOT NULL, task_id TEXT NOT NULL, status TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS bg_completions (
    line_uuid TEXT PRIMARY KEY, session_id TEXT NOT NULL, tool_use_id TEXT NOT NULL, seq INTEGER NOT NULL,
    status TEXT NOT NULL, summary TEXT NOT NULL, result_id TEXT, ts TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS bg_completions_tool ON bg_completions(tool_use_id, seq);
CREATE TABLE IF NOT EXISTS embed_jobs (
    kind TEXT NOT NULL, ref_id TEXT NOT NULL, enqueued_at INTEGER NOT NULL, bulk INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, ref_id)
);
CREATE TABLE IF NOT EXISTS sessions (session_id TEXT PRIMARY KEY, transcript_path TEXT NOT NULL, state TEXT NOT NULL);
CREATE VIRTUAL TABLE IF NOT EXISTS iterations_fts USING fts5(embed_text, content='iterations', tokenize='trigram');
CREATE VIRTUAL TABLE IF NOT EXISTS results_fts USING fts5(text, content='results', tokenize='trigram');
";

const PENDING_STATUS: &str = "pending";
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;
const UNKNOWN_BG_TOOL: &str = "background";
const MIN_FTS_TOKEN_CHARS: usize = 3;
const SUMMARY_CHARS: usize = 80;
const TRUNCATION_MARK: char = '…';

#[derive(Serialize, Deserialize)]
struct ToolCallRow {
    id: String,
    name: String,
    input: Value,
    is_error: bool,
}

impl RecordStore {
    pub fn open(path: &Path) -> Result<Self> {
        prepare_owner_only(path)?;
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

    pub fn apply(&mut self, batch: &Batch) -> Result<()> {
        let tx = self.conn.transaction()?;
        for it in &batch.iterations {
            insert_iteration(&tx, it, batch.bulk)?;
        }
        for c in &batch.completions {
            insert_completion(&tx, c, batch.bulk)?;
        }
        tx.execute(
            "INSERT INTO sessions (session_id, transcript_path, state) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET transcript_path = excluded.transcript_path, state = excluded.state",
            params![batch.session_id, batch.transcript_path, serde_json::to_string(&batch.state)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 같은 `message.id`의 반복이 다른 세션으로 이미 있는가. fork 복사본 판정에 쓴다. (FR-IMP-4)
    pub fn message_in_other_session(&self, message_id: &str, session_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM iterations WHERE message_id = ?1 AND session_id <> ?2 LIMIT 1",
                params![message_id, session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn session_state(&self, session_id: &str) -> Result<Option<ExtractorState>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM sessions WHERE session_id = ?1",
                [session_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    pub fn iteration(&self, id: &str) -> Result<Option<StoredIteration>> {
        Ok(self
            .load_iterations("WHERE id = ?1", id)?
            .into_iter()
            .next())
    }

    pub fn iterations_for_prompt(&self, prompt_id: &str) -> Result<Vec<StoredIteration>> {
        self.load_iterations(
            "WHERE prompt_id = ?1 ORDER BY iteration_index, ts",
            prompt_id,
        )
    }

    pub fn result(&self, id: &str) -> Result<Option<StoredResult>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, tool_name, text, first_seen_ts FROM results WHERE id = ?1",
                [id],
                |r| {
                    Ok(StoredResult {
                        id: r.get(0)?,
                        tool_name: r.get(1)?,
                        text: r.get(2)?,
                        first_seen_ts: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn iterations_referencing_result(&self, result_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT iteration_id FROM iteration_results WHERE result_id = ?1 ORDER BY iteration_id")?;
        let rows = stmt.query_map([result_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    pub fn bg_task(&self, tool_use_id: &str) -> Result<Option<BgTask>> {
        let row = self
            .conn
            .query_row(
                "SELECT tool_use_id, session_id, iteration_id, kind, task_id, status FROM bg_tasks WHERE tool_use_id = ?1",
                [tool_use_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.and_then(
            |(tool_use_id, session_id, iteration_id, kind, task_id, status)| {
                Some(BgTask {
                    tool_use_id,
                    session_id,
                    iteration_id,
                    kind: kind_from_str(&kind)?,
                    task_id,
                    status,
                })
            },
        ))
    }

    pub fn bg_completions(&self, tool_use_id: &str) -> Result<Vec<BgCompletion>> {
        let mut stmt = self.conn.prepare(
            "SELECT line_uuid, tool_use_id, seq, status, summary, result_id, ts
             FROM bg_completions WHERE tool_use_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([tool_use_id], |r| {
            Ok(BgCompletion {
                line_uuid: r.get(0)?,
                tool_use_id: r.get(1)?,
                seq: r.get(2)?,
                status: r.get(3)?,
                summary: r.get(4)?,
                result_id: r.get(5)?,
                ts: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn embed_jobs(&self, limit: usize) -> Result<Vec<EmbedJob>> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, ref_id FROM embed_jobs ORDER BY enqueued_at, rowid LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let raw = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(raw
            .into_iter()
            .filter_map(|(kind, ref_id)| {
                Some(EmbedJob {
                    kind: job_kind_from_str(&kind)?,
                    ref_id,
                })
            })
            .collect())
    }

    /// 모든 작업을 등록 순서대로 돌려준다. (FR-DMN-6)
    pub fn pending_jobs(&self) -> Result<Vec<PendingJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, ref_id, enqueued_at, bulk FROM embed_jobs ORDER BY enqueued_at, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        let raw = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(raw
            .into_iter()
            .filter_map(|(kind, ref_id, enqueued_at, bulk)| {
                Some(PendingJob {
                    job: EmbedJob {
                        kind: job_kind_from_str(&kind)?,
                        ref_id,
                    },
                    enqueued_at,
                    bulk,
                })
            })
            .collect())
    }

    /// 다른 원본 저장소의 기록을 가져온다. 같은 키의 기존 행은 바꾸지 않는다.
    /// `enqueue`면 새로 추가한 반복과 결과마다 임베딩 작업을 등록한다. 원본의 미완료 작업은 항상 옮긴다. (FR-PRJ-9)
    pub fn merge_from(&mut self, source: &Path, enqueue: bool) -> Result<MergeReport> {
        self.conn
            .execute("ATTACH DATABASE ?1 AS src", [source.to_string_lossy()])?;
        let merged = merge_attached(&mut self.conn, enqueue);
        self.conn.execute("DETACH DATABASE src", [])?;
        merged
    }

    pub fn complete_embed_jobs(&mut self, jobs: &[EmbedJob]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for job in jobs {
            tx.execute(
                "DELETE FROM embed_jobs WHERE kind = ?1 AND ref_id = ?2",
                params![job_kind_str(job.kind), job.ref_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn search_iterations_fts(
        &self,
        query: &str,
        filter: &FtsFilter,
        limit: usize,
    ) -> Result<Vec<FtsHit>> {
        self.search_fts(
            "SELECT i.id, bm25(iterations_fts) FROM iterations_fts
             JOIN iterations i ON i.rowid = iterations_fts.rowid
             WHERE iterations_fts MATCH ?1
               AND (?2 IS NULL OR i.session_id <> ?2)
             ORDER BY bm25(iterations_fts) LIMIT ?3",
            query,
            filter,
            limit,
        )
    }

    pub fn search_results_fts(
        &self,
        query: &str,
        filter: &FtsFilter,
        limit: usize,
    ) -> Result<Vec<FtsHit>> {
        self.search_fts(
            "SELECT r.id, bm25(results_fts) FROM results_fts
             JOIN results r ON r.rowid = results_fts.rowid
             WHERE results_fts MATCH ?1
               AND (?2 IS NULL OR EXISTS (
                   SELECT 1 FROM iteration_results ir JOIN iterations i ON i.id = ir.iteration_id
                   WHERE ir.result_id = r.id AND i.session_id <> ?2))
             ORDER BY bm25(results_fts) LIMIT ?3",
            query,
            filter,
            limit,
        )
    }

    /// 결과를 참조하는 반복 중 필터를 만족하는 것이 있는지. 필터가 비어 있으면 항상 참. (FR-RET-2)
    pub fn result_in_scope(&self, result_id: &str, filter: &FtsFilter) -> Result<bool> {
        let Some(exclude) = &filter.exclude_session else {
            return Ok(true);
        };
        Ok(self
            .conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM iteration_results ir JOIN iterations i ON i.id = ir.iteration_id
                 WHERE ir.result_id = ?1 AND i.session_id <> ?2)",
                params![result_id, exclude],
                |r| r.get(0),
            )?)
    }

    /// `rowid > after`인 반복 id를 rowid 순으로 최대 `limit`개 돌려준다. (재색인용)
    pub fn iteration_page(&self, after: i64, limit: usize) -> Result<Vec<(i64, String)>> {
        self.page("iterations", "", after, limit)
    }

    /// `rowid > after`인 결과 id를 rowid 순으로 최대 `limit`개 돌려준다. (재색인용)
    pub fn result_page(&self, after: i64, limit: usize) -> Result<Vec<(i64, String)>> {
        self.page("results", "AND indexed = 1", after, limit)
    }

    fn page(
        &self,
        table: &str,
        extra: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT rowid, id FROM {table} WHERE rowid > ?1 {extra} ORDER BY rowid LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![after, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn counts(&self) -> Result<Counts> {
        let count = |table: &str| -> Result<u64> {
            let n: i64 =
                self.conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
            Ok(n.unsigned_abs())
        };
        Ok(Counts {
            iterations: count("iterations")?,
            results: count("results")?,
            embed_jobs: count("embed_jobs")?,
        })
    }

    /// 저장소 전체의 세션, 턴, 반복, 결과 수. 프로젝트 삭제의 미리 보기에 쓴다. (FR-DEL-4)
    pub fn totals(&self) -> Result<ForgetCounts> {
        let count = |sql: &str| -> Result<u64> {
            let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
            Ok(n.unsigned_abs())
        };
        Ok(ForgetCounts {
            sessions: count(
                "SELECT COUNT(*) FROM (SELECT session_id FROM iterations UNION SELECT session_id FROM sessions)",
            )?,
            turns: count("SELECT COUNT(DISTINCT prompt_id) FROM iterations")?,
            iterations: count("SELECT COUNT(*) FROM iterations")?,
            results: count("SELECT COUNT(*) FROM results")?,
        })
    }

    /// 삭제 대상이 이 저장소에 있는가. 세션은 반복이나 수집 위치가 있으면 있다. (FR-DEL-2, FR-DEL-3)
    pub fn contains(&self, target: &ForgetTarget) -> Result<bool> {
        let (sql, id) = match target {
            ForgetTarget::Session(id) => (
                "SELECT EXISTS (SELECT 1 FROM iterations WHERE session_id = ?1)
                 OR EXISTS (SELECT 1 FROM sessions WHERE session_id = ?1)",
                id,
            ),
            ForgetTarget::Prompt(id) => (
                "SELECT EXISTS (SELECT 1 FROM iterations WHERE prompt_id = ?1)",
                id,
            ),
        };
        Ok(self.conn.query_row(sql, [id], |r| r.get(0))?)
    }

    /// 대상을 한 트랜잭션으로 지운다. `dry_run`이면 같은 과정을 거친 뒤 되돌려, 미리 보기와 실제 삭제가
    /// 같은 대상 선정 규칙을 쓰게 한다. (FR-DEL-2, FR-DEL-3, FR-DEL-5, FR-DEL-6)
    pub fn forget(&mut self, target: &ForgetTarget, dry_run: bool) -> Result<ForgetPlan> {
        let tx = self.conn.transaction()?;
        let plan = forget_in(&tx, target)?;
        if !dry_run {
            tx.commit()?;
        }
        Ok(plan)
    }

    /// 세션을 시작 시각 내림차순으로 한 페이지 돌려준다. 필터가 있으면 그 글자가 든 반복을 가진 세션만. (FR-GUI-13, FR-GUI-14)
    pub fn session_page(
        &self,
        filter: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<SessionSummary>> {
        let matched = filter_matches(filter);
        let scope = match &matched {
            Some((sql, _)) => format!(
                "WHERE session_id IN (SELECT session_id FROM iterations WHERE id IN ({sql}))"
            ),
            None => String::new(),
        };
        let filter_param = matched.as_ref().map(|(_, p)| p.as_str());
        let total = self.count_named(
            &format!("SELECT COUNT(DISTINCT session_id) FROM iterations {scope}"),
            filter_param,
            None,
        )?;
        let sql = format!(
            "SELECT session_id, MIN(ts) AS started, COUNT(DISTINCT COALESCE(prompt_id, id)), COUNT(*)
             FROM iterations {scope}
             GROUP BY session_id ORDER BY started DESC, session_id LIMIT :limit OFFSET :offset"
        );
        let rows: Vec<(String, String, i64, i64)> =
            self.query_named(&sql, filter_param, None, offset, limit, |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
        let mut items = Vec::with_capacity(rows.len());
        for (session_id, started_at, turns, iterations) in rows {
            let first_prompt = self.first_prompt("session_id = ?1", &[&session_id])?;
            items.push(SessionSummary {
                session_id,
                started_at,
                first_prompt,
                turns: turns.unsigned_abs(),
                iterations: iterations.unsigned_abs(),
            });
        }
        Ok(Page { total, items })
    }

    /// 세션의 턴을 시각 오름차순으로 한 페이지 돌려준다. 필터는 `session_page`와 같다. (FR-GUI-13, FR-GUI-14)
    pub fn turn_page(
        &self,
        session_id: &str,
        filter: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<TurnSummary>> {
        let matched = filter_matches(filter);
        let scope = match &matched {
            Some((sql, _)) => format!(
                "AND COALESCE(prompt_id, id) IN (SELECT COALESCE(prompt_id, id) FROM iterations
                 WHERE session_id = :session AND id IN ({sql}))"
            ),
            None => String::new(),
        };
        let filter_param = matched.as_ref().map(|(_, p)| p.as_str());
        let total = self.count_named(
            &format!(
                "SELECT COUNT(DISTINCT COALESCE(prompt_id, id)) FROM iterations
                 WHERE session_id = :session {scope}"
            ),
            filter_param,
            Some(session_id),
        )?;
        let sql = format!(
            "SELECT COALESCE(prompt_id, id) AS turn, MAX(prompt_id), MIN(ts) AS at, COUNT(*)
             FROM iterations WHERE session_id = :session {scope}
             GROUP BY turn ORDER BY at, turn LIMIT :limit OFFSET :offset"
        );
        let rows: Vec<(String, Option<String>, String, i64)> =
            self.query_named(&sql, filter_param, Some(session_id), offset, limit, |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
        let in_turn = "session_id = ?1 AND COALESCE(prompt_id, id) = ?2";
        let mut items = Vec::with_capacity(rows.len());
        for (turn, prompt_id, ts, iterations) in rows {
            let prompt = self.first_prompt(in_turn, &[&session_id, &turn])?;
            let pending: i64 = self.conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM embed_jobs
                     WHERE (kind = ?3 AND ref_id IN (SELECT id FROM iterations WHERE {in_turn}))
                        OR (kind = ?4 AND ref_id IN (SELECT result_id FROM iteration_results
                            WHERE iteration_id IN (SELECT id FROM iterations WHERE {in_turn})))"
                ),
                params![
                    session_id,
                    turn,
                    job_kind_str(RecordKind::Iteration),
                    job_kind_str(RecordKind::Result)
                ],
                |r| r.get(0),
            )?;
            items.push(TurnSummary {
                prompt_id,
                ts,
                prompt,
                iterations: iterations.unsigned_abs(),
                pending: pending.unsigned_abs(),
            });
        }
        Ok(Page { total, items })
    }

    /// 조건에 맞는 반복 중 가장 이른 프롬프트 입력. 없으면 빈 문자열. (FR-GUI-13)
    fn first_prompt(&self, condition: &str, args: &[&dyn rusqlite::ToSql]) -> Result<String> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT inputs FROM iterations WHERE {condition} ORDER BY ts, iteration_index"
        ))?;
        let mut rows = stmt.query(args)?;
        while let Some(row) = rows.next()? {
            let inputs: Vec<IterationInput> = serde_json::from_str(&row.get::<_, String>(0)?)?;
            if let Some(text) = inputs.iter().find_map(|i| match i {
                IterationInput::Prompt { text } => Some(text),
                _ => None,
            }) {
                return Ok(summary_line(text));
            }
        }
        Ok(String::new())
    }

    fn count_named(&self, sql: &str, filter: Option<&str>, session: Option<&str>) -> Result<u64> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut args: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
        if let Some(f) = &filter {
            args.push((":filter", f));
        }
        if let Some(s) = &session {
            args.push((":session", s));
        }
        let n: i64 = stmt.query_row(args.as_slice(), |r| r.get(0))?;
        Ok(n.unsigned_abs())
    }

    fn query_named<T>(
        &self,
        sql: &str,
        filter: Option<&str>,
        session: Option<&str>,
        offset: usize,
        limit: usize,
        map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare(sql)?;
        let (limit, offset) = (limit as i64, offset as i64);
        let mut args: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":limit", &limit), (":offset", &offset)];
        if let Some(f) = &filter {
            args.push((":filter", f));
        }
        if let Some(s) = &session {
            args.push((":session", s));
        }
        let rows = stmt.query_map(args.as_slice(), map)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn search_fts(
        &self,
        sql: &str,
        query: &str,
        filter: &FtsFilter,
        limit: usize,
    ) -> Result<Vec<FtsHit>> {
        let Some(fts_query) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(
            params![fts_query, filter.exclude_session, limit as i64],
            |r| {
                Ok(FtsHit {
                    id: r.get(0)?,
                    rank: -r.get::<_, f64>(1)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn load_iterations(&self, where_clause: &str, arg: &str) -> Result<Vec<StoredIteration>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, session_id, prompt_id, message_id, iteration_index, cwd, ts, inputs, tool_calls,
                    response_text, embed_text FROM iterations {where_clause}"
        ))?;
        type Raw = (
            String,
            String,
            Option<String>,
            String,
            u32,
            String,
            String,
            String,
            String,
            String,
            String,
        );
        let rows = stmt.query_map([arg], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        })?;
        let raw: Vec<Raw> = rows.collect::<rusqlite::Result<_>>()?;
        raw.into_iter()
            .map(
                |(
                    id,
                    session_id,
                    prompt_id,
                    message_id,
                    iteration_index,
                    cwd,
                    ts,
                    inputs,
                    calls,
                    response_text,
                    embed_text,
                )| {
                    let links = self.result_links(&id)?;
                    let rows: Vec<ToolCallRow> = serde_json::from_str(&calls)?;
                    let tool_calls = rows
                        .into_iter()
                        .map(|c| StoredToolCall {
                            result_id: links
                                .iter()
                                .find(|(t, _)| *t == c.id)
                                .map(|(_, r)| r.clone()),
                            id: c.id,
                            name: c.name,
                            input: c.input,
                            is_error: c.is_error,
                        })
                        .collect();
                    Ok(StoredIteration {
                        id,
                        session_id,
                        prompt_id,
                        message_id,
                        iteration_index,
                        cwd,
                        ts,
                        inputs: serde_json::from_str(&inputs)?,
                        tool_calls,
                        response_text,
                        embed_text,
                    })
                },
            )
            .collect()
    }

    fn result_links(&self, iteration_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool_use_id, result_id FROM iteration_results WHERE iteration_id = ?1",
        )?;
        let rows = stmt.query_map([iteration_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// 상위 디렉터리는 0700, DB 파일은 0600으로 만든다. SQLite는 -wal/-shm을 DB 파일 권한으로 만든다. (DR-2)
#[cfg(unix)]
fn prepare_owner_only(path: &Path) -> Result<()> {
    use std::fs::{DirBuilder, OpenOptions, Permissions, set_permissions};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty() && !d.exists())
    {
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(dir)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .open(path)?;
    set_permissions(path, Permissions::from_mode(FILE_MODE))?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn iteration_id(session_id: &str, message_id: &str) -> String {
    format!("{session_id}:{message_id}")
}

fn insert_iteration(tx: &Transaction, new: &NewIteration, bulk: bool) -> Result<()> {
    let it = &new.iteration;
    let id = iteration_id(&it.session_id, &it.message_id);
    let calls: Vec<ToolCallRow> = it
        .tool_calls
        .iter()
        .map(|c| ToolCallRow {
            id: c.id.clone(),
            name: c.name.clone(),
            input: c.input.clone(),
            is_error: c.result.as_ref().is_some_and(|r| r.is_error),
        })
        .collect();
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO iterations (id, session_id, prompt_id, message_id, iteration_index, cwd, ts,
             inputs, tool_calls, response_text, embed_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            it.session_id,
            it.prompt_id,
            it.message_id,
            it.iteration_index,
            it.cwd,
            it.ts,
            serde_json::to_string(&it.inputs)?,
            serde_json::to_string(&calls)?,
            it.texts.join("\n"),
            new.embed_text,
        ],
    )?;
    if inserted == 0 {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO iterations_fts (rowid, embed_text) VALUES (last_insert_rowid(), ?1)",
        [&new.embed_text],
    )?;
    enqueue(tx, RecordKind::Iteration, &id, bulk)?;
    for call in &it.tool_calls {
        if let Some(result) = &call.result
            && let Some(result_id) = insert_result(
                tx,
                &call.name,
                &result.text,
                &it.ts,
                bulk,
                !is_hippo_call(call),
            )?
        {
            tx.execute(
                "INSERT OR IGNORE INTO iteration_results (iteration_id, tool_use_id, result_id) VALUES (?1, ?2, ?3)",
                params![id, call.id, result_id],
            )?;
        }
        if let Some(start) = background_start(call) {
            tx.execute(
                "INSERT OR IGNORE INTO bg_tasks (tool_use_id, session_id, iteration_id, kind, task_id, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![start.tool_use_id, it.session_id, id, kind_str(start.kind), start.task_id, PENDING_STATUS],
            )?;
        }
    }
    Ok(())
}

fn insert_completion(tx: &Transaction, c: &NewCompletion, bulk: bool) -> Result<()> {
    let exists = tx
        .query_row(
            "SELECT 1 FROM bg_completions WHERE line_uuid = ?1",
            [&c.line_uuid],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        return Ok(());
    }
    let tool_use_id = &c.notification.tool_use_id;
    let kind: Option<String> = tx
        .query_row(
            "SELECT kind FROM bg_tasks WHERE tool_use_id = ?1",
            [tool_use_id],
            |r| r.get(0),
        )
        .optional()?;
    let tool_name = kind.as_deref().unwrap_or(UNKNOWN_BG_TOOL);
    let result_id = match &c.result_text {
        Some(text) => insert_result(tx, tool_name, text, &c.ts, bulk, true)?,
        None => None,
    };
    let seq: u32 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM bg_completions WHERE tool_use_id = ?1",
        [tool_use_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO bg_completions (line_uuid, session_id, tool_use_id, seq, status, summary, result_id, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            c.line_uuid,
            c.session_id,
            tool_use_id,
            seq,
            c.notification.status,
            c.notification.summary,
            result_id,
            c.ts
        ],
    )?;
    tx.execute(
        "UPDATE bg_tasks SET status = ?1 WHERE tool_use_id = ?2",
        params![c.notification.status, tool_use_id],
    )?;
    Ok(())
}

/// 원문을 저장하고 id(SHA-256 hex)를 돌려준다. 빈 원문은 저장하지 않는다. (FR-STO-3, 4)
fn insert_result(
    tx: &Transaction,
    tool_name: &str,
    text: &str,
    ts: &str,
    bulk: bool,
    index: bool,
) -> Result<Option<String>> {
    if text.is_empty() {
        return Ok(None);
    }
    let id: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO results (id, tool_name, text, first_seen_ts, indexed) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, tool_name, text, ts, index],
    )?;
    if inserted > 0 && index {
        tx.execute(
            "INSERT INTO results_fts (rowid, text) VALUES (last_insert_rowid(), ?1)",
            [text],
        )?;
        enqueue(tx, RecordKind::Result, &id, bulk)?;
    }
    Ok(Some(id))
}

fn enqueue(tx: &Transaction, kind: RecordKind, ref_id: &str, bulk: bool) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO embed_jobs (kind, ref_id, enqueued_at, bulk) VALUES (?1, ?2, ?3, ?4)",
        params![job_kind_str(kind), ref_id, now_millis(), bulk],
    )?;
    Ok(())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

const ITERATION_COLUMNS: &str = "id, session_id, prompt_id, message_id, iteration_index, cwd, ts, inputs, tool_calls, response_text, embed_text";
const RESULT_COLUMNS: &str = "id, tool_name, text, first_seen_ts, indexed";

/// `src`로 붙인 원본 저장소의 행을 한 트랜잭션으로 가져온다. FTS 행은 새로 추가한 행에만 만든다.
fn merge_attached(conn: &mut Connection, enqueue: bool) -> Result<MergeReport> {
    let tx = conn.transaction()?;
    let now = now_millis();
    let mut report = MergeReport::default();
    for (table, fts, text, kind, columns, indexed) in [
        (
            "iterations",
            "iterations_fts",
            "embed_text",
            RecordKind::Iteration,
            ITERATION_COLUMNS,
            "1",
        ),
        (
            "results",
            "results_fts",
            "text",
            RecordKind::Result,
            RESULT_COLUMNS,
            "indexed = 1",
        ),
    ] {
        tx.execute_batch(&format!(
            "DROP TABLE IF EXISTS temp.merged;
             CREATE TEMP TABLE merged (id TEXT PRIMARY KEY);
             INSERT INTO temp.merged SELECT id FROM src.{table} WHERE id NOT IN (SELECT id FROM main.{table});
             INSERT INTO main.{table} ({columns}) SELECT {columns} FROM src.{table} WHERE id IN (SELECT id FROM temp.merged);
             INSERT INTO main.{fts} (rowid, {text}) SELECT rowid, {text} FROM main.{table} WHERE id IN (SELECT id FROM temp.merged) AND {indexed};"
        ))?;
        let added: i64 = tx.query_row("SELECT COUNT(*) FROM temp.merged", [], |r| r.get(0))?;
        let added = added.unsigned_abs() as usize;
        match kind {
            RecordKind::Iteration => report.iterations = added,
            RecordKind::Result => report.results = added,
        }
        if enqueue {
            report.jobs += tx.execute(
                &format!(
                    "INSERT OR IGNORE INTO main.embed_jobs (kind, ref_id, enqueued_at, bulk)
                     SELECT ?1, id, ?2, 1 FROM main.{table} WHERE id IN (SELECT id FROM temp.merged) AND {indexed}"
                ),
                params![job_kind_str(kind), now],
            )?;
        }
        tx.execute_batch("DROP TABLE temp.merged;")?;
    }
    report.jobs += tx.execute(
        "INSERT OR IGNORE INTO main.embed_jobs (kind, ref_id, enqueued_at, bulk)
         SELECT kind, ref_id, enqueued_at, 1 FROM src.embed_jobs",
        [],
    )?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO main.iteration_results SELECT iteration_id, tool_use_id, result_id FROM src.iteration_results;
         INSERT OR IGNORE INTO main.bg_tasks SELECT tool_use_id, session_id, iteration_id, kind, task_id, status FROM src.bg_tasks;
         INSERT OR IGNORE INTO main.bg_completions SELECT line_uuid, session_id, tool_use_id, seq, status, summary, result_id, ts FROM src.bg_completions;
         INSERT OR IGNORE INTO main.sessions SELECT session_id, transcript_path, state FROM src.sessions;",
    )?;
    tx.commit()?;
    Ok(report)
}

const FORGET_TABLES: &str = "
DROP TABLE IF EXISTS temp.del_it; DROP TABLE IF EXISTS temp.del_bg; DROP TABLE IF EXISTS temp.del_done;
DROP TABLE IF EXISTS temp.cand_res; DROP TABLE IF EXISTS temp.del_res;
";

/// 대상을 임시 테이블로 정한 뒤 지운다. FTS 행은 원래 텍스트와 함께 지우고, 색인에 없는 행
/// (`indexed = 0`인 결과)에는 delete 명령을 보내지 않는다. (FR-DEL-2, FR-DEL-3, FR-DEL-5)
fn forget_in(tx: &Transaction, target: &ForgetTarget) -> Result<ForgetPlan> {
    let (column, id, session) = match target {
        ForgetTarget::Session(id) => ("session_id", id.as_str(), Some(id.as_str())),
        ForgetTarget::Prompt(id) => ("prompt_id", id.as_str(), None),
    };
    tx.execute_batch(&format!(
        "{FORGET_TABLES}
         CREATE TEMP TABLE del_it (id TEXT PRIMARY KEY);
         CREATE TEMP TABLE del_bg (tool_use_id TEXT PRIMARY KEY);
         CREATE TEMP TABLE del_done (line_uuid TEXT PRIMARY KEY);
         CREATE TEMP TABLE cand_res (id TEXT PRIMARY KEY);
         CREATE TEMP TABLE del_res (id TEXT PRIMARY KEY);"
    ))?;
    tx.execute(
        &format!("INSERT INTO temp.del_it SELECT id FROM iterations WHERE {column} = ?1"),
        [id],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO temp.del_bg SELECT tool_use_id FROM bg_tasks
         WHERE iteration_id IN (SELECT id FROM temp.del_it) OR session_id = ?1",
        params![session],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO temp.del_done SELECT line_uuid FROM bg_completions
         WHERE tool_use_id IN (SELECT tool_use_id FROM temp.del_bg) OR session_id = ?1",
        params![session],
    )?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO temp.cand_res SELECT result_id FROM iteration_results
             WHERE iteration_id IN (SELECT id FROM temp.del_it);
         INSERT OR IGNORE INTO temp.cand_res SELECT result_id FROM bg_completions
             WHERE line_uuid IN (SELECT line_uuid FROM temp.del_done) AND result_id IS NOT NULL;
         DELETE FROM iteration_results WHERE iteration_id IN (SELECT id FROM temp.del_it);
         DELETE FROM bg_completions WHERE line_uuid IN (SELECT line_uuid FROM temp.del_done);
         DELETE FROM bg_tasks WHERE tool_use_id IN (SELECT tool_use_id FROM temp.del_bg);
         INSERT INTO temp.del_res SELECT c.id FROM temp.cand_res c
             WHERE EXISTS (SELECT 1 FROM results WHERE id = c.id)
               AND NOT EXISTS (SELECT 1 FROM iteration_results WHERE result_id = c.id)
               AND NOT EXISTS (SELECT 1 FROM bg_completions WHERE result_id = c.id);",
    )?;
    let count = |sql: &str| -> Result<u64> {
        let n: i64 = tx.query_row(sql, [], |r| r.get(0))?;
        Ok(n.unsigned_abs())
    };
    let sessions: i64 = tx.query_row(
        "SELECT COUNT(*) FROM (SELECT session_id FROM iterations WHERE id IN (SELECT id FROM temp.del_it)
         UNION SELECT session_id FROM sessions WHERE session_id = ?1)",
        params![session],
        |r| r.get(0),
    )?;
    let counts = ForgetCounts {
        sessions: sessions.unsigned_abs(),
        turns: count(
            "SELECT COUNT(DISTINCT prompt_id) FROM iterations WHERE id IN (SELECT id FROM temp.del_it)",
        )?,
        iterations: count("SELECT COUNT(*) FROM temp.del_it")?,
        results: count("SELECT COUNT(*) FROM temp.del_res")?,
    };
    let ids = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    };
    let iterations = ids("SELECT id FROM temp.del_it ORDER BY id")?;
    let results = ids("SELECT id FROM temp.del_res ORDER BY id")?;
    let jobs = {
        let mut stmt = tx.prepare(
            "SELECT kind, ref_id FROM embed_jobs
             WHERE (kind = ?1 AND ref_id IN (SELECT id FROM temp.del_it))
                OR (kind = ?2 AND ref_id IN (SELECT id FROM temp.del_res))
             ORDER BY enqueued_at, rowid",
        )?;
        let rows = stmt.query_map(
            params![
                job_kind_str(RecordKind::Iteration),
                job_kind_str(RecordKind::Result)
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(kind, ref_id)| {
                Some(EmbedJob {
                    kind: job_kind_from_str(&kind)?,
                    ref_id,
                })
            })
            .collect()
    };
    tx.execute(
        "DELETE FROM embed_jobs
         WHERE (kind = ?1 AND ref_id IN (SELECT id FROM temp.del_it))
            OR (kind = ?2 AND ref_id IN (SELECT id FROM temp.del_res))",
        params![
            job_kind_str(RecordKind::Iteration),
            job_kind_str(RecordKind::Result)
        ],
    )?;
    tx.execute_batch(&format!(
        "INSERT INTO iterations_fts (iterations_fts, rowid, embed_text)
             SELECT 'delete', rowid, embed_text FROM iterations WHERE id IN (SELECT id FROM temp.del_it);
         INSERT INTO results_fts (results_fts, rowid, text)
             SELECT 'delete', rowid, text FROM results WHERE id IN (SELECT id FROM temp.del_res) AND indexed = 1;
         DELETE FROM iterations WHERE id IN (SELECT id FROM temp.del_it);
         DELETE FROM results WHERE id IN (SELECT id FROM temp.del_res);
         {FORGET_TABLES}"
    ))?;
    if let Some(session) = session {
        tx.execute("DELETE FROM sessions WHERE session_id = ?1", [session])?;
    }
    let vector_refs = iterations
        .into_iter()
        .map(|id| (RecordKind::Iteration, id))
        .chain(results.into_iter().map(|id| (RecordKind::Result, id)))
        .collect();
    Ok(ForgetPlan {
        counts,
        jobs,
        vector_refs,
    })
}

/// 3자 이상 토큰을 따옴표로 감싸 OR로 묶는다. (FR-STO-8)
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().count() >= MIN_FTS_TOKEN_CHARS)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// 기록 탭 필터에 맞는 반복 id를 고르는 SQL과 `:filter` 값. 3자 이상은 FTS 구절, 미만은 부분 문자열. (FR-GUI-14)
fn filter_matches(filter: &str) -> Option<(&'static str, String)> {
    let filter = filter.trim();
    if filter.is_empty() {
        return None;
    }
    Some(if filter.chars().count() >= MIN_FTS_TOKEN_CHARS {
        (
            "SELECT i.id FROM iterations_fts JOIN iterations i ON i.rowid = iterations_fts.rowid
             WHERE iterations_fts MATCH :filter",
            format!("\"{}\"", filter.replace('"', "\"\"")),
        )
    } else {
        (
            "SELECT id FROM iterations WHERE instr(embed_text, :filter) > 0",
            filter.to_string(),
        )
    })
}

/// 공백을 한 칸으로 줄이고 `SUMMARY_CHARS`자로 자른다. 잘렸으면 `…`를 붙인다. (FR-GUI-13)
fn summary_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SUMMARY_CHARS {
        return line;
    }
    let mut cut: String = line.chars().take(SUMMARY_CHARS).collect();
    cut.push(TRUNCATION_MARK);
    cut
}

fn kind_str(kind: BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Bash => "Bash",
        BackgroundKind::Agent => "Agent",
    }
}

fn kind_from_str(s: &str) -> Option<BackgroundKind> {
    match s {
        "Bash" => Some(BackgroundKind::Bash),
        "Agent" => Some(BackgroundKind::Agent),
        _ => None,
    }
}

fn job_kind_str(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Iteration => "iteration",
        RecordKind::Result => "result",
    }
}

fn job_kind_from_str(s: &str) -> Option<RecordKind> {
    match s {
        "iteration" => Some(RecordKind::Iteration),
        "result" => Some(RecordKind::Result),
        _ => None,
    }
}
