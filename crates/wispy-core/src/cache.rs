use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Result};
use crate::model::DiffView;
use crate::pr_ref::PrRef;
use crate::range::RangeSpec;
use crate::stack::StackSnapshot;

/// Local SQLite store for stack snapshots and precomputed diff views.
pub struct Cache {
    conn: Mutex<Connection>,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             DROP TABLE IF EXISTS pr_snapshots;
             CREATE TABLE IF NOT EXISTS stack_snapshots (
                 pr TEXT PRIMARY KEY,
                 data TEXT NOT NULL,
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS diff_views (
                 key TEXT PRIMARY KEY,
                 data BLOB NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Cache { conn: Mutex::new(conn) })
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
            .conn()
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
            .conn()
            .query_row("SELECT data FROM diff_views WHERE key = ?1", [spec.cache_key(repo_slug)], |row| row.get(0))
            .optional()?;
        data.map(|d| rmp_serde::from_slice(&d).map_err(Error::codec)).transpose()
    }

    pub fn has_diff_view(&self, repo_slug: &str, spec: &RangeSpec) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row("SELECT 1 FROM diff_views WHERE key = ?1", [spec.cache_key(repo_slug)], |row| row.get(0))
            .optional()?;
        Ok(found.is_some())
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
