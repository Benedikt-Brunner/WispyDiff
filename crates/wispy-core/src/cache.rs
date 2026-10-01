use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};

use crate::drafts::Draft;
use crate::error::{Error, Result};
use crate::github::ReviewThread;
use crate::highlight::Seg;
use crate::highlight_cache::{BlobStore, Lines};
use crate::model::DiffView;
use crate::pr_ref::PrRef;
use crate::progress::Checkpoint;
use crate::range::RangeSpec;
use crate::stack::StackSnapshot;

/// How long a connection waits for SQLite's own file locks before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Idle read connections kept open for reuse.
const IDLE_READERS: usize = 4;

/// Local SQLite store for stack snapshots and precomputed diff views.
///
/// Writes go through one connection; reads use their own connections (WAL lets them run
/// alongside a write), so opening a cached view never queues behind a background task
/// storing a multi-megabyte view or a batch of highlighted files.
pub struct Cache {
    writer: Mutex<Connection>,
    /// `None` for an in-memory cache, whose data only its one connection can see.
    readers: Option<Readers>,
}

struct Readers {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let readers = Readers { path: path.to_path_buf(), idle: Mutex::new(Vec::new()) };
        Self::init(Connection::open(path)?, Some(readers))
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, readers: Option<Readers>) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             DROP TABLE IF EXISTS pr_snapshots;
             CREATE TABLE IF NOT EXISTS stack_snapshots (
                 pr TEXT PRIMARY KEY,
                 data TEXT NOT NULL,
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS highlighted_blobs (
                 key TEXT PRIMARY KEY,
                 data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS drafts (
                 id TEXT PRIMARY KEY,
                 repo TEXT NOT NULL,
                 pr INTEGER NOT NULL,
                 data TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS drafts_by_pr ON drafts (repo, pr);
             CREATE TABLE IF NOT EXISTS threads (
                 repo TEXT NOT NULL,
                 pr INTEGER NOT NULL,
                 data TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (repo, pr)
             );
             CREATE TABLE IF NOT EXISTS viewed (
                 repo TEXT NOT NULL,
                 key TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (repo, key)
             );
             CREATE TABLE IF NOT EXISTS checkpoints (
                 id TEXT PRIMARY KEY,
                 repo TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS assistant_threads (
                 id TEXT PRIMARY KEY,
                 repo TEXT NOT NULL,
                 data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS app_state (
                 key TEXT PRIMARY KEY,
                 data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS repo_settings (
                 repo TEXT PRIMARY KEY,
                 data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS diff_views (
                 key TEXT PRIMARY KEY,
                 data BLOB NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Cache { writer: Mutex::new(conn), readers })
    }

    /// Stores the snapshot under every PR of the stack, so opening any of them is cached.
    pub fn put_stack(&self, snapshot: &StackSnapshot) -> Result<()> {
        let data = serde_json::to_string(snapshot).map_err(Error::codec)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for index in 0..snapshot.len() {
            tx.execute(
                "INSERT INTO stack_snapshots (pr, data) VALUES (?1, ?2)
                 ON CONFLICT(pr) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
                params![snapshot.pr_ref(index).to_string(), data],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The stack `pr` was last seen in, with `focus` pointing at `pr`.
    pub fn stack(&self, pr: &PrRef) -> Result<Option<StackSnapshot>> {
        let data: Option<String> = self
            .reader()?
            .query_row("SELECT data FROM stack_snapshots WHERE pr = ?1", [pr.to_string()], |row| row.get(0))
            .optional()?;
        let Some(mut snapshot) = data.map(|d| serde_json::from_str::<StackSnapshot>(&d).map_err(Error::codec)).transpose()? else {
            return Ok(None);
        };
        match snapshot.stack.prs.iter().position(|p| p.number == pr.number) {
            Some(focus) => {
                snapshot.stack.focus = focus;
                Ok(Some(snapshot))
            }
            None => Ok(None),
        }
    }

    pub fn put_diff_view(&self, repo_slug: &str, spec: &RangeSpec, view: &DiffView) -> Result<()> {
        let data = rmp_serde::to_vec(view).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO diff_views (key, data) VALUES (?1, ?2)",
            params![spec.cache_key(repo_slug), data],
        )?;
        Ok(())
    }

    pub fn diff_view(&self, repo_slug: &str, spec: &RangeSpec) -> Result<Option<DiffView>> {
        let data: Option<Vec<u8>> = self
            .reader()?
            .query_row("SELECT data FROM diff_views WHERE key = ?1", [spec.cache_key(repo_slug)], |row| row.get(0))
            .optional()?;
        data.map(|d| rmp_serde::from_slice(&d).map_err(Error::codec)).transpose()
    }

    pub fn has_diff_view(&self, repo_slug: &str, spec: &RangeSpec) -> Result<bool> {
        let found: Option<i64> = self
            .reader()?
            .query_row("SELECT 1 FROM diff_views WHERE key = ?1", [spec.cache_key(repo_slug)], |row| row.get(0))
            .optional()?;
        Ok(found.is_some())
    }

    pub fn put_draft(&self, draft: &Draft) -> Result<()> {
        let data = serde_json::to_string(draft).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO drafts (id, repo, pr, data) VALUES (?1, ?2, ?3, ?4)",
            params![draft.id, draft.repo, draft.pr as i64, data],
        )?;
        Ok(())
    }

    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM drafts WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn draft(&self, id: &str) -> Result<Option<Draft>> {
        let data: Option<String> =
            self.reader()?.query_row("SELECT data FROM drafts WHERE id = ?1", [id], |row| row.get(0)).optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    /// Drafts of the given PRs of `repo`, oldest first.
    pub fn drafts(&self, repo: &str, prs: &[u64]) -> Result<Vec<Draft>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare("SELECT data FROM drafts WHERE repo = ?1 AND pr = ?2")?;
        let mut drafts = Vec::new();
        for pr in prs {
            let rows = statement.query_map(params![repo, *pr as i64], |row| row.get::<_, String>(0))?;
            for data in rows {
                drafts.push(serde_json::from_str::<Draft>(&data?).map_err(Error::codec)?);
            }
        }
        drafts.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(drafts)
    }

    /// Whether any PR of `repo` in `prs` has something not yet posted.
    pub fn has_pending_drafts(&self, repo: &str, prs: &[u64]) -> Result<bool> {
        Ok(self.drafts(repo, prs)?.iter().any(Draft::is_pending))
    }

    pub fn put_threads(&self, repo: &str, pr: u64, threads: &[ReviewThread]) -> Result<()> {
        let data = serde_json::to_string(threads).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO threads (repo, pr, data) VALUES (?1, ?2, ?3)",
            params![repo, pr as i64, data],
        )?;
        Ok(())
    }

    pub fn threads(&self, repo: &str, pr: u64) -> Result<Option<Vec<ReviewThread>>> {
        let data: Option<String> = self
            .reader()?
            .query_row("SELECT data FROM threads WHERE repo = ?1 AND pr = ?2", params![repo, pr as i64], |row| row.get(0))
            .optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    pub fn viewed_keys(&self, repo: &str) -> Result<Vec<String>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare("SELECT key FROM viewed WHERE repo = ?1")?;
        let rows = statement.query_map([repo], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_viewed(&self, repo: &str, key: &str, viewed: bool) -> Result<()> {
        if viewed {
            self.conn().execute("INSERT OR IGNORE INTO viewed (repo, key) VALUES (?1, ?2)", params![repo, key])?;
        } else {
            self.conn().execute("DELETE FROM viewed WHERE repo = ?1 AND key = ?2", params![repo, key])?;
        }
        Ok(())
    }

    pub fn put_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        let data = serde_json::to_string(checkpoint).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO checkpoints (id, repo, created_at, data) VALUES (?1, ?2, ?3, ?4)",
            params![checkpoint.id, checkpoint.repo, checkpoint.created_at, data],
        )?;
        Ok(())
    }

    /// All checkpoints of a repo, newest first.
    pub fn checkpoints(&self, repo: &str) -> Result<Vec<Checkpoint>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare("SELECT data FROM checkpoints WHERE repo = ?1 ORDER BY created_at DESC, id DESC")?;
        let rows = statement.query_map([repo], |row| row.get::<_, String>(0))?;
        let mut checkpoints = Vec::new();
        for data in rows {
            checkpoints.push(serde_json::from_str(&data?).map_err(Error::codec)?);
        }
        Ok(checkpoints)
    }

    pub fn put_assistant_thread(&self, thread: &crate::assistant::Thread) -> Result<()> {
        let data = serde_json::to_string(thread).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO assistant_threads (id, repo, data) VALUES (?1, ?2, ?3)",
            params![thread.id, thread.repo, data],
        )?;
        Ok(())
    }

    pub fn assistant_thread(&self, id: &str) -> Result<Option<crate::assistant::Thread>> {
        let data: Option<String> =
            self.reader()?.query_row("SELECT data FROM assistant_threads WHERE id = ?1", [id], |row| row.get(0)).optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    /// A repo's assistant threads, oldest first.
    pub fn assistant_threads(&self, repo: &str) -> Result<Vec<crate::assistant::Thread>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare("SELECT data FROM assistant_threads WHERE repo = ?1")?;
        let rows = statement.query_map([repo], |row| row.get::<_, String>(0))?;
        let mut threads: Vec<crate::assistant::Thread> = Vec::new();
        for data in rows {
            threads.push(serde_json::from_str(&data?).map_err(Error::codec)?);
        }
        threads.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(threads)
    }

    pub fn delete_assistant_thread(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM assistant_threads WHERE id = ?1", [id])?;
        Ok(())
    }

    /// A small app-wide JSON value (e.g. the last inbox), `None` if never stored.
    pub fn app_value(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let data: Option<String> =
            self.reader()?.query_row("SELECT data FROM app_state WHERE key = ?1", [key], |row| row.get(0)).optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    pub fn put_app_value(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        self.conn().execute("INSERT OR REPLACE INTO app_state (key, data) VALUES (?1, ?2)", params![key, value.to_string()])?;
        Ok(())
    }

    /// PRs (`owner/repo#N`) with a cached stack snapshot.
    pub fn cached_prs(&self) -> Result<Vec<String>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare("SELECT pr FROM stack_snapshots")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Forgets a PR's cached stack, threads, and every diff view computed from `heads`.
    pub fn evict(&self, pr: &PrRef, heads: &[String]) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM stack_snapshots WHERE pr = ?1", [pr.to_string()])?;
        conn.execute("DELETE FROM threads WHERE repo = ?1 AND pr = ?2", params![pr.repo_slug(), pr.number as i64])?;
        for head in heads {
            conn.execute("DELETE FROM diff_views WHERE key LIKE ?1", [format!("%{head}%")])?;
        }
        Ok(())
    }

    /// Per-repo settings (a JSON value); `None` if never set.
    pub fn repo_settings(&self, repo_slug: &str) -> Result<Option<serde_json::Value>> {
        let data: Option<String> = self
            .reader()?
            .query_row("SELECT data FROM repo_settings WHERE repo = ?1", [repo_slug], |row| row.get(0))
            .optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    pub fn put_repo_settings(&self, repo_slug: &str, settings: &serde_json::Value) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO repo_settings (repo, data) VALUES (?1, ?2)",
            params![repo_slug, settings.to_string()],
        )?;
        Ok(())
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A connection for reading: an idle one, or a new one if all are in use.
    fn reader(&self) -> Result<Reader<'_>> {
        let Some(readers) = &self.readers else { return Ok(Reader::Writer(self.conn())) };
        let idle = readers.idle.lock().unwrap_or_else(|p| p.into_inner()).pop();
        let conn = match idle {
            Some(conn) => conn,
            None => {
                let conn = Connection::open(&readers.path)?;
                conn.busy_timeout(BUSY_TIMEOUT)?;
                conn.pragma_update(None, "query_only", true)?;
                conn
            }
        };
        Ok(Reader::Pooled(Some(conn), readers))
    }
}

enum Reader<'a> {
    Pooled(Option<Connection>, &'a Readers),
    Writer(MutexGuard<'a, Connection>),
}

impl std::ops::Deref for Reader<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Reader::Pooled(conn, _) => conn.as_ref().expect("only taken on drop"),
            Reader::Writer(conn) => conn,
        }
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Reader::Pooled(conn, readers) = self {
            let mut idle = readers.idle.lock().unwrap_or_else(|p| p.into_inner());
            if idle.len() < IDLE_READERS {
                idle.extend(conn.take());
            }
        }
    }
}

impl BlobStore for Cache {
    fn get(&self, key: &str) -> Option<Vec<Vec<Seg>>> {
        let data: Vec<u8> = self
            .reader().ok()?
            .query_row("SELECT data FROM highlighted_blobs WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .ok()??;
        rmp_serde::from_slice(&data).ok()
    }

    fn put_many(&self, items: &[(String, Lines)]) {
        let encoded: Vec<(&String, Vec<u8>)> = items
            .iter()
            .filter_map(|(key, lines)| Some((key, rmp_serde::to_vec(lines.as_ref()).ok()?)))
            .collect();
        let mut conn = self.conn();
        let Ok(tx) = conn.transaction() else { return };
        for (key, data) in &encoded {
            let _ = tx.execute("INSERT OR IGNORE INTO highlighted_blobs (key, data) VALUES (?1, ?2)", params![key, data]);
        }
        let _ = tx.commit();
    }
}
