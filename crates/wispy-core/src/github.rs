use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::pr_ref::PrRef;

pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// The subset of a pull request the app needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub html_url: String,
    pub author: String,
    pub base_ref: String,
    pub base_sha: String,
    pub head_ref: String,
    pub head_sha: String,
    /// Clone URL of the base repository (PR heads are always fetchable from it via `refs/pull/N/head`).
    pub clone_url: String,
    /// `owner/repo` of the base repository.
    pub base_repo: String,
    /// `owner/repo` the head branch lives in; `None` if that fork was deleted.
    pub head_repo: Option<String>,
    pub default_branch: String,
}

impl PullRequest {
    /// Whether the head branch lives in the base repository (stacks never span forks).
    pub fn is_same_repo(&self) -> bool {
        self.head_repo.as_deref() == Some(self.base_repo.as_str())
    }
}

pub struct GitHubClient {
    api_base: String,
    token: String,
    http: reqwest::Client,
}

impl GitHubClient {
    pub fn new(api_base: impl Into<String>, token: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("WispyDiff/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(GitHubClient { api_base: api_base.into().trim_end_matches('/').to_string(), token: token.into(), http })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn pull_request(&self, pr: &PrRef) -> Result<PullRequest> {
        let path = format!("/repos/{}/{}/pulls/{}", pr.owner, pr.repo, pr.number);
        let raw: RawPullRequest = self.get(&path, &[]).await?;
        Ok(raw.into())
    }

    /// Open PRs in `owner/repo` whose base branch is `base`.
    pub async fn open_pulls_with_base(&self, owner: &str, repo: &str, base: &str) -> Result<Vec<PullRequest>> {
        self.open_pulls(owner, repo, &[("base", base.to_string())]).await
    }

    /// Open PRs in `owner/repo` whose head is branch `head` of that same repository.
    pub async fn open_pulls_with_head(&self, owner: &str, repo: &str, head: &str) -> Result<Vec<PullRequest>> {
        self.open_pulls(owner, repo, &[("head", format!("{owner}:{head}"))]).await
    }

    async fn open_pulls(&self, owner: &str, repo: &str, filter: &[(&str, String)]) -> Result<Vec<PullRequest>> {
        let mut query: Vec<(&str, String)> = vec![("state", "open".into()), ("per_page", "100".into())];
        query.extend(filter.iter().cloned());
        let raw: Vec<RawPullRequest> = self.get(&format!("/repos/{owner}/{repo}/pulls"), &query).await?;
        Ok(raw.into_iter().map(PullRequest::from).collect())
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.api_base))
            .query(query)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<ApiError>(&body).map(|e| e.message).unwrap_or(body);
            return Err(Error::GitHub(format!("GET {path}: {status} {message}")));
        }
        Ok(response.json().await?)
    }
}

impl From<RawPullRequest> for PullRequest {
    fn from(raw: RawPullRequest) -> Self {
        let base_repo = raw.base.repo.expect("a PR's base repository always exists");
        PullRequest {
            number: raw.number,
            title: raw.title,
            state: if raw.merged_at.is_some() { "merged".to_string() } else { raw.state },
            draft: raw.draft.unwrap_or(false),
            html_url: raw.html_url,
            author: raw.user.login,
            base_ref: raw.base.ref_name,
            base_sha: raw.base.sha,
            head_ref: raw.head.ref_name,
            head_sha: raw.head.sha,
            clone_url: base_repo.clone_url,
            base_repo: base_repo.full_name,
            head_repo: raw.head.repo.map(|r| r.full_name),
            default_branch: base_repo.default_branch,
        }
    }
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Deserialize)]
struct RawPullRequest {
    number: u64,
    title: String,
    state: String,
    draft: Option<bool>,
    merged_at: Option<String>,
    html_url: String,
    user: RawUser,
    base: RawBranch,
    head: RawBranch,
}

#[derive(Deserialize)]
struct RawUser {
    login: String,
}

#[derive(Deserialize)]
struct RawBranch {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
    repo: Option<RawRepo>,
}

#[derive(Deserialize)]
struct RawRepo {
    clone_url: String,
    full_name: String,
    default_branch: String,
}
