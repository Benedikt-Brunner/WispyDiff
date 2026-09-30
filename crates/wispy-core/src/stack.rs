//! Stack discovery: a stack is a chain of same-repo PRs where each PR's base branch is the
//! head branch of the PR below it, ending at a PR that targets the default branch (or a
//! branch no open PR provides).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::github::{GitHubClient, PullRequest};
use crate::pr_ref::PrRef;
use crate::range::RangeSpec;
use crate::repo_store::StackPoints;

/// Stacks longer than this are assumed to be a cycle or a data problem.
const MAX_STACK: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// Bottom (targets the default branch) to top.
    pub prs: Vec<PullRequest>,
    /// Index of the PR the stack was discovered from.
    pub focus: usize,
}

/// A discovered and fetched stack: everything needed to compute any range offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackSnapshot {
    pub stack: Stack,
    /// See [`StackPoints`].
    pub heads: Vec<String>,
    pub bases: Vec<String>,
}

impl StackSnapshot {
    pub fn new(stack: Stack, points: StackPoints) -> Self {
        StackSnapshot { stack, heads: points.heads, bases: points.bases }
    }

    pub fn len(&self) -> usize {
        self.stack.prs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.prs.is_empty()
    }

    /// `owner/repo` of the stack.
    pub fn repo_slug(&self) -> &str {
        &self.stack.prs[0].base_repo
    }

    pub fn pr_ref(&self, index: usize) -> PrRef {
        let (owner, repo) = self.repo_slug().split_once('/').unwrap_or((self.repo_slug(), ""));
        PrRef { owner: owner.to_string(), repo: repo.to_string(), number: self.stack.prs[index].number }
    }

    /// The range of PRs `lo..=hi` (stack indices, clamped).
    pub fn range(&self, lo: usize, hi: usize) -> RangeSpec {
        let hi = hi.min(self.len() - 1);
        let lo = lo.min(hi);
        RangeSpec {
            from: self.bases[lo].clone(),
            heads: self.heads[lo..=hi].to_vec(),
            prs: (lo..=hi).map(|i| i as u8).collect(),
        }
    }

    /// For each PR: true if it isn't based on the latest head of the PR below it.
    pub fn needs_rebase(&self) -> Vec<bool> {
        (0..self.len()).map(|i| i > 0 && self.bases[i] != self.heads[i - 1]).collect()
    }

    /// All ranges, most useful first: the focused PR, the whole stack, then the rest.
    pub fn ranges_by_priority(&self) -> Vec<(usize, usize)> {
        let n = self.len();
        let focus = self.stack.focus;
        let mut ranges = vec![(focus, focus), (0, n - 1)];
        ranges.extend((0..n).map(|i| (i, i)));
        for len in 2..n {
            ranges.extend((0..=n - len).map(|lo| (lo, lo + len - 1)));
        }
        let mut seen = std::collections::HashSet::new();
        ranges.retain(|r| seen.insert(*r));
        ranges
    }
}

pub async fn discover_stack(github: &GitHubClient, pr: &PrRef) -> Result<Stack> {
    let start = github.pull_request(pr).await?;
    let mut below: Vec<PullRequest> = Vec::new();
    let mut above: Vec<PullRequest> = Vec::new();
    let mut visited = std::collections::HashSet::from([start.number]);
    let cycle = |number: u64| Error::GitHub(format!("{pr}: stack contains a cycle at #{number}"));

    // Down: the open PR (in this repo) whose head branch is our base branch.
    let mut current = start.clone();
    while current.base_ref != current.default_branch && below.len() + above.len() < MAX_STACK {
        let parents = github.open_pulls_with_head(&pr.owner, &pr.repo, &current.base_ref).await?;
        let Some(parent) = pick(parents, &current.base_ref, |p| &p.head_ref) else { break };
        if !visited.insert(parent.number) {
            return Err(cycle(parent.number));
        }
        current = parent.clone();
        below.push(parent);
    }

    // Up: open PRs based on our head branch. Stacks are linear; if several PRs branch off
    // the same head, follow the oldest.
    let mut current = start.clone();
    while start.is_same_repo() && below.len() + above.len() < MAX_STACK {
        let children = github.open_pulls_with_base(&pr.owner, &pr.repo, &current.head_ref).await?;
        let Some(child) = pick(children, &current.head_ref, |p| &p.base_ref) else { break };
        if !visited.insert(child.number) {
            return Err(cycle(child.number));
        }
        current = child.clone();
        above.push(child);
    }

    below.reverse();
    let focus = below.len();
    let mut prs = below;
    prs.push(start);
    prs.extend(above);
    Ok(Stack { prs, focus })
}

/// Same-repo candidates whose `field` really is `branch` (the API filter is authoritative,
/// but fakes and edge cases shouldn't silently pull in a wrong PR), lowest number first.
fn pick(candidates: Vec<PullRequest>, branch: &str, field: impl Fn(&PullRequest) -> &String) -> Option<PullRequest> {
    candidates
        .into_iter()
        .filter(|p| p.is_same_repo() && field(p) == branch)
        .min_by_key(|p| p.number)
}
