use std::sync::Arc;

use crate::cache::Cache;
use crate::error::Result;
use crate::github::GitHubClient;
use crate::highlight::Highlighter;
use crate::highlight_cache::HighlightCache;
use crate::model::DiffView;
use crate::pr_ref::PrRef;
use crate::range::compute_range;
use crate::repo_store::RepoStore;
use crate::stack::{discover_stack, Stack, StackSnapshot};

/// Opens stacks: structure from GitHub, code from the local blobless clone, and precomputed
/// range views from the cache whenever the SHAs are unchanged.
pub struct PrService {
    github: GitHubClient,
    store: RepoStore,
    cache: Cache,
    highlighter: Highlighter,
    highlights: HighlightCache,
}

impl PrService {
    pub fn new(github: GitHubClient, store: RepoStore, cache: Cache, highlighter: Highlighter) -> Self {
        PrService { github, store, cache, highlighter, highlights: HighlightCache::default() }
    }

    /// The stack `pr` was last seen in, without touching the network.
    pub fn cached_stack(&self, pr: &PrRef) -> Result<Option<StackSnapshot>> {
        self.cache.stack(pr)
    }

    pub async fn discover(&self, pr: &PrRef) -> Result<Stack> {
        discover_stack(&self.github, pr).await
    }

    /// Fetches every PR of `stack` into the local clone and records the snapshot.
    /// Blocking: call from a worker thread.
    pub fn fetch(&self, stack: Stack) -> Result<StackSnapshot> {
        let bottom = &stack.prs[0];
        let (owner, repo) = bottom.base_repo.split_once('/').unwrap_or((&bottom.base_repo, ""));
        let git = self.store.ensure_repo(owner, repo, &bottom.clone_url)?;
        let points = self.store.fetch_stack(&git, &stack.prs)?;
        let snapshot = StackSnapshot::new(stack, points);
        self.cache.put_stack(&snapshot)?;
        Ok(snapshot)
    }

    /// The cached view of `lo..=hi`, if it was computed before (no git involved).
    pub fn cached_range(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<Option<DiffView>> {
        self.cache.diff_view(snapshot.repo_slug(), &snapshot.range(lo, hi))
    }

    /// The view of `lo..=hi`, computing and caching it if needed. Blocking.
    pub fn range(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<Arc<DiffView>> {
        if let Some(view) = self.cached_range(snapshot, lo, hi)? {
            return Ok(Arc::new(view));
        }
        self.compute(snapshot, lo, hi).map(Arc::new)
    }

    /// Computes `lo..=hi` into the cache unless it's already there. Returns whether it computed.
    pub fn precompute(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<bool> {
        if self.cache.has_diff_view(snapshot.repo_slug(), &snapshot.range(lo, hi))? {
            return Ok(false);
        }
        self.compute(snapshot, lo, hi)?;
        Ok(true)
    }

    fn compute(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<DiffView> {
        let spec = snapshot.range(lo, hi);
        let bottom = &snapshot.stack.prs[0];
        let (owner, repo) = snapshot.repo_slug().split_once('/').unwrap_or((snapshot.repo_slug(), ""));
        let git = self.store.ensure_repo(owner, repo, &bottom.clone_url)?;
        let view = compute_range(&git, &spec, &self.highlighter, &self.highlights)?;
        self.cache.put_diff_view(snapshot.repo_slug(), &spec, &view)?;
        Ok(view)
    }
}
