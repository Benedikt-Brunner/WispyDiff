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

/// When the author kept building on `old_to` (no force push): the tree "`old_to` plus what the
/// merge commits since brought in from the base" — merging `main` (or the PR below) into the
/// branch, conflict resolutions included. Diffing it against `new_to` leaves only the author's
/// own commits. A merge counts as bringing in the base when every merged-in parent is already
/// part of `new_from`; merging some other branch is a real change and stays in the diff.
/// None if `new_to` doesn't build on `old_to` or replaying a merge conflicted.
pub fn merges_from_base(git: &Git, old_to: &str, new_to: &str, new_from: &str) -> Result<Option<String>> {
    let (descends, _) = git.run_status(&["merge-base", "--is-ancestor", old_to, new_to])?;
    if !descends {
        return Ok(None);
    }
    let (exclude_old, exclude_base) = (format!("^{old_to}"), format!("^{new_from}"));
    let merges = git.run_string(&[
        "rev-list", "--merges", "--topo-order", "--reverse", "--no-commit-header", "--format=%H %P", new_to, &exclude_old, &exclude_base,
    ])?;
    let in_base = |sha: &str| git.run_status(&["merge-base", "--is-ancestor", sha, new_from]).is_ok_and(|(yes, _)| yes);
    let mut tree = old_to.to_string();
    for line in merges.lines() {
        let mut shas = line.split_whitespace();
        let (Some(merge), Some(first_parent)) = (shas.next(), shas.next()) else { continue };
        if !shas.all(in_base) {
            continue;
        }
        let merge_base = format!("--merge-base={first_parent}");
        let (clean, out) = git.run_status(&["merge-tree", "--write-tree", "--no-messages", &merge_base, &tree, merge])?;
        let merged = String::from_utf8_lossy(&out).lines().next().unwrap_or_default().trim().to_string();
        if !clean || merged.is_empty() {
            return Ok(None);
        }
        tree = merged;
    }
    Ok(Some(tree))
}
