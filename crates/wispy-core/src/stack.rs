//! Stack discovery: a stack is a chain of same-repo PRs where each PR's base branch is the
//! head branch of the PR below it, ending at a PR that targets the default branch (or a
//! branch no open PR provides). Several PRs can build on one head: the PRs then form a tree,
//! and a stack is one path through it, from the bottom up to the end of a branch.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::github::{GitHubClient, PullRequest};
use crate::pr_ref::PrRef;
use crate::range::RangeSpec;
use crate::repo_store::StackPoints;

/// Trees larger than this are assumed to be a cycle or a data problem.
const MAX_STACK: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// Bottom (targets the default branch) to top.
    pub prs: Vec<PullRequest>,
    /// Index of the PR the stack was discovered from.
    pub focus: usize,
    /// For each PR of `prs`: when several open PRs build on its head (a tree of PRs), every
    /// one of those branches, the one this stack goes on with included, lowest number first.
    /// Empty where the stack doesn't split (and in caches from before).
    #[serde(default)]
    pub forks: Vec<Vec<Branch>>,
}

impl Stack {
    /// The branches splitting off above stack PR `index` (empty: none).
    pub fn branches(&self, index: usize) -> &[Branch] {
        self.forks.get(index).map_or(&[], Vec::as_slice)
    }
}

/// One of several branches building on the same PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// The branch's first PR.
    pub number: u64,
    pub title: String,
    /// PRs in the branch: its first and everything built on it.
    pub size: usize,
}

/// Every open PR built on one bottom PR: a stack that may split into branches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackTree {
    /// Breadth-first from the bottom; each PR's children lowest number first.
    pub prs: Vec<PullRequest>,
    /// Index of each PR's parent (`None`: the bottom).
    pub parents: Vec<Option<usize>>,
}

impl StackTree {
    pub fn position(&self, number: u64) -> Option<usize> {
        self.prs.iter().position(|p| p.number == number)
    }

    fn children(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.prs.len()).filter(move |&i| self.parents[i] == Some(index))
    }

    fn size(&self, index: usize) -> usize {
        1 + self.children(index).map(|c| self.size(c)).sum::<usize>()
    }

    /// The stack PR `index` is shown in: the PRs below it, it, and above it the branch
    /// `preferred` names (PR numbers above it, bottom first, as last seen), or the oldest
    /// branch wherever it names none.
    pub fn stack(&self, index: usize, preferred: &[u64]) -> Stack {
        let mut path = vec![index];
        while let Some(parent) = self.parents[*path.last().unwrap()] {
            path.push(parent);
        }
        path.reverse();
        let focus = path.len() - 1;
        let mut preferred = preferred.iter();
        let mut wanted = preferred.next();
        loop {
            let children: Vec<usize> = self.children(*path.last().unwrap()).collect();
            let Some(&oldest) = children.first() else { break };
            let next = match children.iter().find(|&&c| Some(&self.prs[c].number) == wanted) {
                Some(&chosen) => {
                    wanted = preferred.next();
                    chosen
                }
                None => {
                    wanted = None;
                    oldest
                }
            };
            path.push(next);
        }
        let forks = path
            .iter()
            .map(|&i| {
                let children: Vec<usize> = self.children(i).collect();
                if children.len() < 2 {
                    return Vec::new();
                }
                children
                    .into_iter()
                    .map(|c| Branch { number: self.prs[c].number, title: self.prs[c].title.clone(), size: self.size(c) })
                    .collect()
            })
            .collect();
        Stack { prs: path.into_iter().map(|i| self.prs[i].clone()).collect(), focus, forks }
    }
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
            ignore_whitespace: false,
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

/// The stack `pr` is in, going on above it along the branch `preferred` names (see
/// [`StackTree::stack`]).
pub async fn discover_stack(github: &GitHubClient, pr: &PrRef, preferred: &[u64]) -> Result<Stack> {
    let (tree, index) = discover_tree(github, pr).await?;
    Ok(tree.stack(index, preferred))
}

/// The whole tree `pr` belongs to, and `pr`'s index in it.
pub async fn discover_tree(github: &GitHubClient, pr: &PrRef) -> Result<(StackTree, usize)> {
    let start = github.pull_request(pr).await?;
    let mut below: Vec<PullRequest> = Vec::new();
    let mut visited = std::collections::HashSet::from([start.number]);
    let cycle = |number: u64| Error::GitHub(format!("{pr}: stack contains a cycle at #{number}"));

    // Down: the open PR (in this repo) whose head branch is our base branch.
    let mut current = start.clone();
    while current.base_ref != current.default_branch && below.len() < MAX_STACK {
        let parents = github.open_pulls_with_head(&pr.owner, &pr.repo, &current.base_ref).await?;
        let Some(parent) = pick(parents, &current.base_ref, |p| &p.head_ref).into_iter().next() else { break };
        if !visited.insert(parent.number) {
            return Err(cycle(parent.number));
        }
        current = parent.clone();
        below.push(parent);
    }
    below.reverse();

    // Up, breadth-first from the bottom: every open PR built on a PR of the tree.
    let chain: Vec<PullRequest> = below.into_iter().chain(std::iter::once(start)).collect();
    let mut tree = StackTree { prs: vec![chain[0].clone()], parents: vec![None] };
    let mut next = 0;
    while next < tree.prs.len() && tree.prs.len() < MAX_STACK {
        let node = tree.prs[next].clone();
        if node.is_same_repo() {
            for child in pick(github.open_pulls_with_base(&pr.owner, &pr.repo, &node.head_ref).await?, &node.head_ref, |p| &p.base_ref) {
                // Two PRs can share a head branch: a PR built on it shows up once.
                if tree.position(child.number).is_none() && tree.prs.len() < MAX_STACK {
                    tree.prs.push(child);
                    tree.parents.push(Some(next));
                }
            }
        }
        next += 1;
    }
    // A fork PR (never a child) or one beyond the size limit still sits on the chain below it.
    for (k, pr) in chain.iter().enumerate().skip(1) {
        if tree.position(pr.number).is_none() {
            let parent = tree.position(chain[k - 1].number);
            tree.prs.push(pr.clone());
            tree.parents.push(parent);
        }
    }
    let index = tree.position(chain.last().unwrap().number).expect("the start PR is in its tree");
    Ok((tree, index))
}

/// Same-repo candidates whose `field` really is `branch` (the API filter is authoritative,
/// but fakes and edge cases shouldn't silently pull in a wrong PR), lowest number first.
fn pick(candidates: Vec<PullRequest>, branch: &str, field: impl Fn(&PullRequest) -> &String) -> Vec<PullRequest> {
    let mut picked: Vec<PullRequest> = candidates.into_iter().filter(|p| p.is_same_repo() && field(p) == branch).collect();
    picked.sort_by_key(|p| p.number);
    picked
}
