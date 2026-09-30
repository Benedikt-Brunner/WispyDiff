//! Locally stored review comments and queued thread actions.

use serde::{Deserialize, Serialize};

use crate::github::Side;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftKind {
    /// On one line or a range of lines.
    Line,
    /// On a whole file.
    File,
    /// The PR-level review body (at most one per PR).
    Summary,
    /// A reply to an existing thread.
    Reply,
    /// Resolving an existing thread.
    Resolve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftStatus {
    Draft,
    /// Sent, outcome unknown (network failed mid-request); reconciled before the next submit.
    Posting,
    Posted,
    /// GitHub rejected it; `error` says why. Stays local until fixed or deleted.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    /// `owner/repo`.
    pub repo: String,
    pub pr: u64,
    pub kind: DraftKind,
    pub path: Option<String>,
    pub side: Option<Side>,
    /// Last line of the commented range, in the PR's own diff at `commit` (RIGHT) or `base` (LEFT).
    pub line: Option<u32>,
    /// First line of a multi-line range.
    pub start_line: Option<u32>,
    /// The PR head the anchor refers to (what was reviewed).
    pub commit: String,
    /// The start of the PR's diff when written (LEFT-side lines refer to it).
    pub base: String,
    pub body: String,
    /// For replies and resolves.
    pub thread_id: Option<String>,
    pub reply_to: Option<u64>,
    /// The user chose to post this (outdated) line comment as a file comment.
    #[serde(default)]
    pub as_file: bool,
    pub status: DraftStatus,
    pub error: Option<String>,
    /// Marker embedded in what was sent, to recognise it if the outcome was unknown.
    pub token: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Draft {
    pub fn is_pending(&self) -> bool {
        self.status != DraftStatus::Posted
    }
}

/// Fresh, practically unique ids without a UUID dependency.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    format!("{nanos:x}{:04x}{:04x}", std::process::id() & 0xffff, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff)
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}
