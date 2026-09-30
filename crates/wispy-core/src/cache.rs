use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::github::PullRequest;
use crate::model::{DiffView, MODEL_VERSION};
use crate::pr_ref::PrRef;

/// What we last knew about a PR, enough to reopen it offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrSnapshot {
    pub pull_request: PullRequest,
    /// The SHAs the cached diff was computed between.
    pub diff_from: String,
    pub diff_to: String,
}

/// Local SQLite store for PR snapshots and precomputed diff views.
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
             CREATE TABLE IF NOT EXISTS pr_snapshots (
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

    pub fn put_snapshot(&self, pr: &PrRef, snapshot: &PrSnapshot) -> Result<()> {
        let data = serde_json::to_string(snapshot).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT INTO pr_snapshots (pr, data) VALUES (?1, ?2)
             ON CONFLICT(pr) DO UPDATE SET data = excluded.data, updated_at = unixepoch()",
            params![pr.to_string(), data],
        )?;
        Ok(())
    }

    pub fn snapshot(&self, pr: &PrRef) -> Result<Option<PrSnapshot>> {
        let data: Option<String> = self
            .conn()
            .query_row("SELECT data FROM pr_snapshots WHERE pr = ?1", [pr.to_string()], |row| row.get(0))
            .optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    pub fn put_diff_view(&self, repo_slug: &str, view: &DiffView) -> Result<()> {
        let key = Self::diff_key(repo_slug, &view.summary.base_sha, &view.summary.head_sha);
        let data = rmp_serde::to_vec(view).map_err(Error::codec)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO diff_views (key, data) VALUES (?1, ?2)",
            params![key, data],
        )?;
        Ok(())
    }

    pub fn diff_view(&self, repo_slug: &str, from: &str, to: &str) -> Result<Option<DiffView>> {
        let key = Self::diff_key(repo_slug, from, to);
        let data: Option<Vec<u8>> = self
            .conn()
            .query_row("SELECT data FROM diff_views WHERE key = ?1", [key], |row| row.get(0))
            .optional()?;
        data.map(|d| rmp_serde::from_slice(&d).map_err(Error::codec)).transpose()
    }

    fn diff_key(repo_slug: &str, from: &str, to: &str) -> String {
        format!("v{MODEL_VERSION}:{repo_slug}:{from}..{to}")
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
