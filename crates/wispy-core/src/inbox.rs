//! The home screen: PRs where the user's review is requested or that they authored, grouped
//! into stacks.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxPr {
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_sha: String,
    pub updated_at: String,
    /// The user's review is requested.
    pub requested: bool,
    /// The user opened it.
    pub authored: bool,
    /// The user's latest submitted review on GitHub (from any client).
    #[serde(default)]
    pub my_review: Option<MyReview>,
    /// GitHub's rendering of the description (empty when there is none, or in caches from before).
    #[serde(default)]
    pub body_html: String,
    /// Auto-merge is enabled: GitHub merges it once its checks pass.
    #[serde(default)]
    pub auto_merge: bool,
    /// The PR's size as GitHub counts it (none in caches from before).
    #[serde(default)]
    pub stats: Option<DiffStats>,
}

/// How big a PR's diff is: changed files and lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStats {
    pub files: u64,
    pub additions: u64,
    pub deletions: u64,
}

/// A review the user submitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MyReview {
    /// GitHub's state: `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED` or `DISMISSED`.
    pub state: String,
    /// The head it was submitted on.
    pub commit: String,
    pub submitted_at: String,
}

/// PRs of one repo that stack on each other, or a single PR. Several PRs can build on the same
/// PR, so a group is a tree: bottom first, then depth-first (each PR's branches lowest number
/// first, every branch in full before the next).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxGroup {
    pub repo: String,
    pub prs: Vec<InboxPr>,
    pub updated_at: String,
    /// Index (in `prs`) of the PR each one builds on; `None` for the bottom. Empty in caches
    /// from before trees: each PR then builds on the one before it.
    #[serde(default)]
    pub parents: Vec<Option<usize>>,
}

impl InboxGroup {
    pub fn key(&self) -> String {
        format!("{}#{}", self.repo, self.prs[0].number)
    }

    /// The PR (index) stack PR `index` builds on.
    pub fn parent(&self, index: usize) -> Option<usize> {
        match self.parents.get(index) {
            Some(parent) => *parent,
            None => index.checked_sub(1),
        }
    }

    /// How many branches the group splits into (1: a plain stack).
    pub fn branches(&self) -> usize {
        let has_child = |i: usize| (0..self.prs.len()).any(|c| self.parent(c) == Some(i));
        (0..self.prs.len()).filter(|&i| !has_child(i)).count()
    }
}

/// Merges search results (the same PR may be both requested and authored).
pub fn merge(results: impl IntoIterator<Item = InboxPr>) -> Vec<InboxPr> {
    let mut by_key: HashMap<(String, u64), InboxPr> = HashMap::new();
    for pr in results {
        by_key
            .entry((pr.repo.clone(), pr.number))
            .and_modify(|existing| {
                existing.requested |= pr.requested;
                existing.authored |= pr.authored;
            })
            .or_insert(pr);
    }
    by_key.into_values().collect()
}

/// Groups PRs whose base branch is another inbox PR's head branch (same repo) into trees,
/// listed depth-first. Groups are ordered by most recent activity.
pub fn group(prs: Vec<InboxPr>) -> Vec<InboxGroup> {
    let mut groups = Vec::new();
    let mut by_repo: HashMap<String, Vec<InboxPr>> = HashMap::new();
    for pr in prs {
        by_repo.entry(pr.repo.clone()).or_default().push(pr);
    }
    for (repo, mut prs) in by_repo {
        prs.sort_by_key(|p| p.number);
        // Two PRs may share a head branch: the oldest is the one PRs build on.
        let mut by_head: HashMap<&str, usize> = HashMap::new();
        for (i, pr) in prs.iter().enumerate() {
            by_head.entry(pr.head_ref.as_str()).or_insert(i);
        }
        let parent: Vec<Option<usize>> =
            prs.iter().enumerate().map(|(i, pr)| by_head.get(pr.base_ref.as_str()).copied().filter(|&p| p != i)).collect();
        let mut used = vec![false; prs.len()];
        // Bottoms first, then whatever is left (PRs building on each other in a cycle).
        let mut starts: Vec<usize> = (0..prs.len()).filter(|&i| parent[i].is_none()).collect();
        starts.extend(0..prs.len());
        for start in starts {
            if used[start] {
                continue;
            }
            // Depth-first; `order` holds (PR index, its parent's position in the group).
            let mut order: Vec<(usize, Option<usize>)> = Vec::new();
            let mut pending = vec![(start, None)];
            while let Some((i, up)) = pending.pop() {
                if used[i] {
                    continue;
                }
                used[i] = true;
                let position = order.len();
                order.push((i, up));
                // Pushed highest first, so the lowest-numbered branch comes out first.
                pending.extend((0..prs.len()).rev().filter(|&c| parent[c] == Some(i) && !used[c]).map(|c| (c, Some(position))));
            }
            let members: Vec<InboxPr> = order.iter().map(|&(i, _)| prs[i].clone()).collect();
            let parents = order.iter().map(|&(_, up)| up).collect();
            let updated_at = members.iter().map(|p| p.updated_at.clone()).max().unwrap_or_default();
            groups.push(InboxGroup { repo: repo.clone(), prs: members, updated_at, parents });
        }
    }
    groups.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.key().cmp(&b.key())));
    groups
}
