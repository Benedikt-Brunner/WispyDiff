use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::drafts::Draft;
use crate::error::{Error, Result};
use crate::github::ReviewThread;
use crate::highlight::Seg;
use crate::highlight_cache::{BlobStore, Lines};
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
            self.conn().query_row("SELECT data FROM drafts WHERE id = ?1", [id], |row| row.get(0)).optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    /// Drafts of the given PRs of `repo`, oldest first.
    pub fn drafts(&self, repo: &str, prs: &[u64]) -> Result<Vec<Draft>> {
        let conn = self.conn();
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
            .conn()
            .query_row("SELECT data FROM threads WHERE repo = ?1 AND pr = ?2", params![repo, pr as i64], |row| row.get(0))
            .optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Error::codec)).transpose()
    }

    /// Per-repo settings (a JSON value); `None` if never set.
    pub fn repo_settings(&self, repo_slug: &str) -> Result<Option<serde_json::Value>> {
        let data: Option<String> = self
            .conn()
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

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl BlobStore for Cache {
    fn get(&self, key: &str) -> Option<Vec<Vec<Seg>>> {
        let data: Vec<u8> = self
            .conn()
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
