//! Review progress: local checkpoints and "only what changed since I last looked" interdiffs.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointEntry {
    pub pr: u64,
    pub head: String,
    /// Start of the PR's diff at the time (its three-dot base).
    pub base: String,
}

/// "I reviewed these PRs at these heads" — local only, never sent to GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub repo: String,
    pub created_at: i64,
    /// "manual" (Mark as reviewed) or "submit" (created by submitting a review).
    pub source: String,
    pub entries: Vec<CheckpointEntry>,
}

impl Checkpoint {
    pub fn entry(&self, pr: u64) -> Option<&CheckpointEntry> {
        self.entries.iter().find(|e| e.pr == pr)
    }
}

/// The tree "old changes replayed onto the new base": `old_from..old_to` applied to
/// `new_from` with a three-way merge. Diffing it against the new head leaves only what the
/// author changed, not what the rebase brought in. Returns the tree (or commit) to diff from,
/// and whether replaying conflicted (then the plain `old_to` is returned instead).
pub fn replayed_base(git: &Git, old_from: &str, old_to: &str, new_from: &str) -> Result<(String, bool)> {
    if old_from == new_from {
        return Ok((old_to.to_string(), false));
    }
    let merge_base = format!("--merge-base={old_from}");
    let (clean, out) = git.run_status(&["merge-tree", "--write-tree", "--no-messages", &merge_base, new_from, old_to])?;
    let tree = String::from_utf8_lossy(&out).lines().next().unwrap_or_default().trim().to_string();
    if clean && !tree.is_empty() {
        Ok((tree, false))
    } else {
        Ok((old_to.to_string(), true))
    }
}
