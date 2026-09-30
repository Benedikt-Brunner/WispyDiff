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
        let url = format!("{}/repos/{}/{}/pulls/{}", self.api_base, pr.owner, pr.repo, pr.number);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<ApiError>(&body).map(|e| e.message).unwrap_or(body);
            return Err(Error::GitHub(format!("{pr}: {status} {message}")));
        }
        let raw: RawPullRequest = response.json().await?;
        Ok(PullRequest {
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
            clone_url: raw.base.repo.clone_url,
        })
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
    repo: RawRepo,
}

#[derive(Deserialize)]
struct RawRepo {
    clone_url: String,
}
