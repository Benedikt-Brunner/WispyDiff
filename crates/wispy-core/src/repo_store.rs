use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::git::Git;
use crate::github::PullRequest;

/// App-managed blobless bare clones, one per GitHub repository.
///
/// Reads (diff, cat-file, ...) run concurrently; anything that writes repository state —
/// setup and fetches — is serialized per repository, so the background prefetch, range
/// precomputation and the UI never trip over git's lock files.
pub struct RepoStore {
    root: PathBuf,
    github_token: Option<String>,
    /// Per repository: a write lock, and the remote URL it was last set up with.
    repos: Mutex<HashMap<PathBuf, Arc<Mutex<Option<String>>>>>,
}

/// Where each PR of a stack starts and ends, after fetching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackPoints {
    /// Head commit of each PR, bottom to top.
    pub heads: Vec<String>,
    /// The commit each PR's own diff starts from: the merge base with the default branch for
    /// the bottom PR, and with the PR below for the others (GitHub's three-dot diff).
    pub bases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPr {
    pub base_sha: String,
    pub head_sha: String,
    pub merge_base: String,
}

impl RepoStore {
    pub fn new(root: impl Into<PathBuf>, github_token: Option<String>) -> Self {
        RepoStore { root: root.into(), github_token, repos: Mutex::new(HashMap::new()) }
    }

    fn repo_lock(&self, dir: &PathBuf) -> Arc<Mutex<Option<String>>> {
        self.repos.lock().unwrap_or_else(|p| p.into_inner()).entry(dir.clone()).or_default().clone()
    }

    pub fn repo_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.root.join(owner).join(format!("{repo}.git"))
    }

    /// Opens the bare clone, creating it (as an empty promisor repo) if needed.
    /// Nothing is downloaded until [`RepoStore::fetch_pr`].
    pub fn ensure_repo(&self, owner: &str, repo: &str, clone_url: &str) -> Result<Git> {
        let dir = self.repo_dir(owner, repo);
        let git = Git::new(&dir, self.github_token.clone());
        let lock = self.repo_lock(&dir);
        let mut set_up_with = lock.lock().unwrap_or_else(|p| p.into_inner());
        if set_up_with.as_deref() == Some(clone_url) {
            return Ok(git);
        }
        if !dir.join("HEAD").exists() {
            std::fs::create_dir_all(&dir)?;
            git.run(&["init", "--bare", "--quiet"])?;
            git.run(&["config", "core.repositoryformatversion", "1"])?;
            git.run(&["config", "extensions.partialclone", "origin"])?;
            git.run(&["config", "remote.origin.promisor", "true"])?;
            git.run(&["config", "remote.origin.partialclonefilter", "blob:none"])?;
            git.run(&["config", "gc.auto", "0"])?;
        }
        let (_, current) = git.run_status(&["config", "--get", "remote.origin.url"])?;
        if String::from_utf8_lossy(&current).trim() != clone_url {
            git.run(&["config", "remote.origin.url", clone_url])?;
        }
        *set_up_with = Some(clone_url.to_string());
        Ok(git)
    }

    /// Fetches the PR head and its base branch (commits and trees only), and returns the
    /// SHAs the PR diff is computed between.
    pub fn fetch_pr(&self, git: &Git, number: u64, base_ref: &str) -> Result<FetchedPr> {
        let head_ref = format!("refs/wispy/pull/{number}/head");
        let base_local = format!("refs/wispy/heads/{base_ref}");
        let lock = self.repo_lock(&git.git_dir().to_path_buf());
        let _writing = lock.lock().unwrap_or_else(|p| p.into_inner());
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

    /// Fetches every PR head of a stack plus the bottom PR's base branch in one `git fetch`.
    pub fn fetch_stack(&self, git: &Git, prs: &[PullRequest]) -> Result<StackPoints> {
        let bottom_base = &prs[0].base_ref;
        let base_local = format!("refs/wispy/heads/{bottom_base}");
        let head_local = |number: u64| format!("refs/wispy/pull/{number}/head");
        let mut refspecs = vec![format!("+refs/heads/{bottom_base}:{base_local}")];
        refspecs.extend(prs.iter().map(|pr| format!("+refs/pull/{}/head:{}", pr.number, head_local(pr.number))));

        let mut args = vec!["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--filter=blob:none", "origin"];
        args.extend(refspecs.iter().map(String::as_str));
        {
            let lock = self.repo_lock(&git.git_dir().to_path_buf());
            let _writing = lock.lock().unwrap_or_else(|p| p.into_inner());
            git.run(&args)?;
        }

        let base_tip = git.run_string(&["rev-parse", &base_local])?;
        let heads = prs
            .iter()
            .map(|pr| git.run_string(&["rev-parse", &head_local(pr.number)]))
            .collect::<Result<Vec<_>>>()?;
        let bases = std::iter::once(&base_tip)
            .chain(&heads)
            .zip(&heads)
            .map(|(below, head)| git.run_string(&["merge-base", below, head]))
            .collect::<Result<Vec<_>>>()?;
        Ok(StackPoints { heads, bases })
    }
}
