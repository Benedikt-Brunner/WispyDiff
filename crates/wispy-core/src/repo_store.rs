use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::Result;
use crate::git::Git;
use crate::github::PullRequest;

/// App-managed blobless bare clones, one per GitHub repository.
///
/// Reads (diff, cat-file, ...) run concurrently; anything that writes repository state —
/// setup and fetches — is serialized per repository, so the background prefetch, range
/// precomputation and the UI never trip over git's lock files. Reads never wait for a write:
/// a repository that is already set up is handed out while a fetch is running.
pub struct RepoStore {
    root: PathBuf,
    github_token: Option<String>,
    repos: Mutex<HashMap<PathBuf, Arc<RepoLocks>>>,
}

#[derive(Default)]
struct RepoLocks {
    /// The remote URL the repository was last set up with.
    set_up_with: Mutex<Option<String>>,
    /// Held while writing repository state (setup, fetches). Taken after `set_up_with`.
    writing: Mutex<()>,
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

    fn locks(&self, dir: &Path) -> Arc<RepoLocks> {
        self.repos.lock().unwrap_or_else(|p| p.into_inner()).entry(dir.to_path_buf()).or_default().clone()
    }

    /// Serializes writes to `git`'s repository with setup and fetches for as long as it's held.
    pub fn write_lock(&self, git: &Git) -> WriteLock {
        WriteLock(self.locks(git.git_dir()))
    }

    /// Where assistant checkouts of a repository live (next to the clones).
    pub fn worktrees_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.root.parent().unwrap_or(&self.root).join("worktrees").join(owner).join(repo)
    }

    pub fn repo_dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.root.join(owner).join(format!("{repo}.git"))
    }

    /// Opens the bare clone, creating it (as an empty promisor repo) if needed.
    /// Nothing is downloaded until [`RepoStore::fetch_pr`].
    pub fn ensure_repo(&self, owner: &str, repo: &str, clone_url: &str) -> Result<Git> {
        let dir = self.repo_dir(owner, repo);
        let git = Git::new(&dir, self.github_token.clone());
        let locks = self.locks(&dir);
        let mut set_up_with = lock(&locks.set_up_with);
        if set_up_with.as_deref() == Some(clone_url) {
            return Ok(git);
        }
        let _writing = lock(&locks.writing);
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
        let lock = self.write_lock(git);
        let _writing = lock.hold();
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
            let lock = self.write_lock(git);
            let _writing = lock.hold();
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

    /// Downloads every blob of `commit`'s tree that the partial clone doesn't have yet, in one
    /// batch (whole-repo search needs them; git would otherwise fetch them one by one).
    /// Returns how many were fetched.
    pub fn hydrate(&self, git: &Git, commit: &str) -> Result<usize> {
        let listing = git.run_string(&["rev-list", "--objects", "--missing=print", "--no-walk", commit])?;
        let missing: Vec<&str> = listing.lines().filter_map(|l| l.strip_prefix('?')).collect();
        if missing.is_empty() {
            return Ok(0);
        }
        let mut input = missing.join("\n");
        input.push('\n');
        let lock = self.write_lock(git);
        let _writing = lock.hold();
        git.run_with_stdin(
            &[
                "-c",
                "fetch.negotiationAlgorithm=noop",
                "fetch",
                "--quiet",
                "origin",
                "--no-tags",
                "--no-write-fetch-head",
                "--recurse-submodules=no",
                "--filter=blob:none",
                "--stdin",
            ],
            input.as_bytes(),
        )?;
        Ok(missing.len())
    }
}

/// A repository's write lock (see [`RepoStore::write_lock`]).
pub struct WriteLock(Arc<RepoLocks>);

impl WriteLock {
    pub fn hold(&self) -> MutexGuard<'_, ()> {
        lock(&self.0.writing)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}
