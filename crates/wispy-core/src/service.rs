use std::sync::Arc;

use crate::cache::Cache;
use crate::error::Result;
use crate::github::GitHubClient;
use crate::highlight::Highlighter;
use crate::highlight_cache::HighlightCache;
use crate::highlight::Language;
use crate::model::{file_slice, DiffView};
use crate::pr_ref::PrRef;
use crate::range::{blob_lines, compute_range};
use crate::repo_store::RepoStore;
use crate::split::{align, split_rows, SplitRow};
use crate::stack::{discover_stack, Stack, StackSnapshot};

/// Opens stacks: structure from GitHub, code from the local blobless clone, and precomputed
/// range views from the cache whenever the SHAs are unchanged.
pub struct PrService {
    github: GitHubClient,
    store: RepoStore,
    cache: Arc<Cache>,
    highlighter: Highlighter,
    highlights: HighlightCache,
}

impl PrService {
    pub fn new(github: GitHubClient, store: RepoStore, cache: Cache, highlighter: Highlighter) -> Self {
        let cache = Arc::new(cache);
        let highlights = HighlightCache::new(3_000_000, Some(cache.clone()));
        PrService { github, store, cache, highlighter, highlights }
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
        self.range_with(snapshot, lo, hi, false)
    }

    /// Like [`PrService::range`], optionally ignoring whitespace-only changes.
    pub fn range_with(&self, snapshot: &StackSnapshot, lo: usize, hi: usize, ignore_whitespace: bool) -> Result<Arc<DiffView>> {
        let spec = snapshot.range(lo, hi).with_ignore_whitespace(ignore_whitespace);
        if let Some(view) = self.cache.diff_view(snapshot.repo_slug(), &spec)? {
            return Ok(Arc::new(view));
        }
        self.compute(snapshot, spec).map(Arc::new)
    }

    /// Computes `lo..=hi` into the cache unless it's already there. Returns whether it computed.
    pub fn precompute(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<bool> {
        let spec = snapshot.range(lo, hi);
        if self.cache.has_diff_view(snapshot.repo_slug(), &spec)? {
            return Ok(false);
        }
        self.compute(snapshot, spec)?;
        Ok(true)
    }

    /// The full-file side-by-side rows of file `index` in `view` (a view of `snapshot`),
    /// excluding the header. Empty when the file has no side-by-side (binary, rename-only).
    pub fn split_rows(&self, snapshot: &StackSnapshot, view: &DiffView, index: usize) -> Result<Vec<SplitRow>> {
        let file = &view.summary.files[index];
        if file.split_rows == 0 {
            return Ok(Vec::new());
        }
        let git = self.git(snapshot)?;
        let language = Language::from_path(&file.path);
        let lines = |blob: &Option<String>| -> Result<crate::highlight_cache::Lines> {
            match blob {
                Some(blob) => blob_lines(&git, blob, language, &self.highlighter, &self.highlights),
                None => Ok(Arc::new(Vec::new())),
            }
        };
        let (old, new) = (lines(&file.old_blob)?, lines(&file.new_blob)?);
        let rows = file_slice(view, index);
        let alignment = align(rows, old.len() as u32, new.len() as u32);
        Ok(split_rows(rows, &alignment.pairs, &old, &new))
    }

    /// Glob patterns of files to collapse in this repo (set by the user).
    pub fn ignore_patterns(&self, repo_slug: &str) -> Result<Vec<String>> {
        let settings = self.cache.repo_settings(repo_slug)?.unwrap_or_default();
        Ok(settings["ignore"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect())
    }

    pub fn set_ignore_patterns(&self, repo_slug: &str, patterns: &[String]) -> Result<()> {
        let mut settings = self.cache.repo_settings(repo_slug)?.unwrap_or_else(|| serde_json::json!({}));
        settings["ignore"] = serde_json::json!(patterns);
        self.cache.put_repo_settings(repo_slug, &settings)
    }

    fn git(&self, snapshot: &StackSnapshot) -> Result<crate::git::Git> {
        let (owner, repo) = snapshot.repo_slug().split_once('/').unwrap_or((snapshot.repo_slug(), ""));
        self.store.ensure_repo(owner, repo, &snapshot.stack.prs[0].clone_url)
    }

    fn compute(&self, snapshot: &StackSnapshot, spec: crate::range::RangeSpec) -> Result<DiffView> {
        let git = self.git(snapshot)?;
        let view = compute_range(&git, &spec, &self.highlighter, &self.highlights)?;
        self.cache.put_diff_view(snapshot.repo_slug(), &spec, &view)?;
        Ok(view)
    }
}
