//! The home screen: PRs where the user's review is requested or that they authored, grouped
//! into stacks.

use std::collections::{HashMap, HashSet};

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
}

/// PRs of one repo that stack on each other (bottom first), or a single PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxGroup {
    pub repo: String,
    pub prs: Vec<InboxPr>,
    pub updated_at: String,
}

impl InboxGroup {
    pub fn key(&self) -> String {
        format!("{}#{}", self.repo, self.prs[0].number)
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

/// Chains PRs whose base branch is another inbox PR's head branch (same repo). Where several
/// PRs branch off one head, the oldest continues the chain and the others start their own.
/// Groups are ordered by most recent activity.
pub fn group(prs: Vec<InboxPr>) -> Vec<InboxGroup> {
    let mut groups = Vec::new();
    let mut by_repo: HashMap<String, Vec<InboxPr>> = HashMap::new();
    for pr in prs {
        by_repo.entry(pr.repo.clone()).or_default().push(pr);
    }
    for (repo, mut prs) in by_repo {
        prs.sort_by_key(|p| p.number);
        let heads: HashSet<&str> = prs.iter().map(|p| p.head_ref.as_str()).collect();
        let mut used = vec![false; prs.len()];
        // Bottoms first (base isn't another PR's head), then whatever is left (branch-offs).
        let mut starts: Vec<usize> = (0..prs.len()).filter(|&i| !heads.contains(prs[i].base_ref.as_str())).collect();
        starts.extend(0..prs.len());
        for start in starts {
            if used[start] {
                continue;
            }
            let mut chain = vec![start];
            used[start] = true;
            loop {
                let head = &prs[*chain.last().unwrap()].head_ref;
                match (0..prs.len()).find(|&i| !used[i] && &prs[i].base_ref == head) {
                    Some(next) => {
                        used[next] = true;
                        chain.push(next);
                    }
                    None => break,
                }
            }
            let members: Vec<InboxPr> = chain.into_iter().map(|i| prs[i].clone()).collect();
            let updated_at = members.iter().map(|p| p.updated_at.clone()).max().unwrap_or_default();
            groups.push(InboxGroup { repo: repo.clone(), prs: members, updated_at });
        }
    }
    groups.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.key().cmp(&b.key())));
    groups
}
