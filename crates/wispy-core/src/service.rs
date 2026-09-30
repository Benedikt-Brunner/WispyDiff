use std::sync::Arc;

use crate::cache::{Cache, PrSnapshot};
use crate::error::Result;
use crate::github::{GitHubClient, PullRequest};
use crate::highlight::Highlighter;
use crate::model::DiffView;
use crate::pr_ref::PrRef;
use crate::repo_store::RepoStore;

#[derive(Debug, Clone)]
pub struct LoadedPr {
    pub pr: PrRef,
    pub pull_request: PullRequest,
    pub view: Arc<DiffView>,
}

/// Opens pull requests: metadata from GitHub, code from the local blobless clone,
/// and a precomputed diff view from the cache whenever the SHAs are unchanged.
pub struct PrService {
    github: GitHubClient,
    store: RepoStore,
    cache: Cache,
    highlighter: Highlighter,
}

impl PrService {
    pub fn new(github: GitHubClient, store: RepoStore, cache: Cache, highlighter: Highlighter) -> Self {
        PrService { github, store, cache, highlighter }
    }

    /// The last diff computed for `pr`, without touching the network.
    pub fn cached(&self, pr: &PrRef) -> Result<Option<LoadedPr>> {
        let Some(snapshot) = self.cache.snapshot(pr)? else {
            return Ok(None);
        };
        let view = self.cache.diff_view(&pr.repo_slug(), &snapshot.diff_from, &snapshot.diff_to)?;
        Ok(view.map(|view| LoadedPr { pr: pr.clone(), pull_request: snapshot.pull_request, view: Arc::new(view) }))
    }

    pub async fn fetch_pull_request(&self, pr: &PrRef) -> Result<PullRequest> {
        self.github.pull_request(pr).await
    }

    /// Fetches the PR's commits into the local clone and computes (or reuses) its diff view.
    /// Blocking: call from a worker thread.
    pub fn load(&self, pr: &PrRef, pull_request: PullRequest) -> Result<LoadedPr> {
        let git = self.store.ensure_repo(&pr.owner, &pr.repo, &pull_request.clone_url)?;
        let fetched = self.store.fetch_pr(&git, pr.number, &pull_request.base_ref)?;
        let slug = pr.repo_slug();
        let view = match self.cache.diff_view(&slug, &fetched.merge_base, &fetched.head_sha)? {
            Some(view) => view,
            None => {
                let view = DiffView::compute(&git, &fetched.merge_base, &fetched.head_sha, &self.highlighter)?;
                self.cache.put_diff_view(&slug, &view)?;
                view
            }
        };
        let snapshot = PrSnapshot {
            pull_request: PullRequest { head_sha: fetched.head_sha.clone(), ..pull_request },
            diff_from: fetched.merge_base,
            diff_to: fetched.head_sha,
        };
        self.cache.put_snapshot(pr, &snapshot)?;
        Ok(LoadedPr { pr: pr.clone(), pull_request: snapshot.pull_request, view: Arc::new(view) })
    }
}
