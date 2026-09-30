use std::path::PathBuf;

use crate::error::Result;
use crate::git::Git;

/// App-managed blobless bare clones, one per GitHub repository.
pub struct RepoStore {
    root: PathBuf,
    github_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPr {
    pub base_sha: String,
    pub head_sha: String,
    pub merge_base: String,
}

impl RepoStore {
    pub fn new(root: impl Into<PathBuf>, github_token: Option<String>) -> Self {
        RepoStore { root: root.into(), github_token }
    }

    pub fn repo_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.root.join(owner).join(format!("{repo}.git"))
    }

    /// Opens the bare clone, creating it (as an empty promisor repo) if needed.
    /// Nothing is downloaded until [`RepoStore::fetch_pr`].
    pub fn ensure_repo(&self, owner: &str, repo: &str, clone_url: &str) -> Result<Git> {
        let dir = self.repo_dir(owner, repo);
        let git = Git::new(&dir, self.github_token.clone());
        if !dir.join("HEAD").exists() {
            std::fs::create_dir_all(&dir)?;
            git.run(&["init", "--bare", "--quiet"])?;
            git.run(&["config", "core.repositoryformatversion", "1"])?;
            git.run(&["config", "extensions.partialclone", "origin"])?;
            git.run(&["config", "remote.origin.promisor", "true"])?;
            git.run(&["config", "remote.origin.partialclonefilter", "blob:none"])?;
            git.run(&["config", "gc.auto", "0"])?;
        }
        git.run(&["config", "remote.origin.url", clone_url])?;
        Ok(git)
    }

    /// Fetches the PR head and its base branch (commits and trees only), and returns the
    /// SHAs the PR diff is computed between.
    pub fn fetch_pr(&self, git: &Git, number: u64, base_ref: &str) -> Result<FetchedPr> {
        let head_ref = format!("refs/wispy/pull/{number}/head");
        let base_local = format!("refs/wispy/heads/{base_ref}");
        git.run(&[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--filter=blob:none",
            "origin",
            &format!("+refs/pull/{number}/head:{head_ref}"),
            &format!("+refs/heads/{base_ref}:{base_local}"),
        ])?;
        let head_sha = git.run_string(&["rev-parse", &head_ref])?;
        let base_sha = git.run_string(&["rev-parse", &base_local])?;
        let merge_base = git.run_string(&["merge-base", &base_sha, &head_sha])?;
        Ok(FetchedPr { base_sha, head_sha, merge_base })
    }
}
