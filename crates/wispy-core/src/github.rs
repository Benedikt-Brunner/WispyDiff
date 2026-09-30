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

/// Which side of a diff a line comment is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Comment,
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadComment {
    pub id: String,
    pub database_id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
}

/// A review thread as GitHub has it. `line` is on the PR's current head (`None` once outdated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub id: String,
    pub resolved: bool,
    pub outdated: bool,
    pub path: String,
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub original_line: Option<u32>,
    pub side: Side,
    /// A comment on the whole file rather than on lines.
    pub file_level: bool,
    pub comments: Vec<ThreadComment>,
}

/// A line comment inside a new review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewReviewComment {
    pub path: String,
    pub line: u32,
    pub side: Side,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<Side>,
    pub body: String,
}

const THREADS_QUERY: &str = "query($owner: String!, $repo: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id isResolved isOutdated path line startLine originalLine diffSide subjectType
          comments(first: 100) { nodes { id databaseId body createdAt url author { login } } }
        }
      }
    }
  }
}";

const SEARCH_QUERY: &str = "query($q: String!) {
  search(query: $q, type: ISSUE, first: 50) {
    nodes {
      ... on PullRequest {
        number title url isDraft updatedAt headRefName baseRefName headRefOid
        author { login }
        repository { nameWithOwner }
      }
    }
  }
}";

const RESOLVE_MUTATION: &str =
    "mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { id isResolved } } }";

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

    /// All review threads of a PR (paginated).
    pub async fn review_threads(&self, pr: &PrRef) -> Result<Vec<ReviewThread>> {
        let mut threads = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let data = self
                .graphql(
                    THREADS_QUERY,
                    serde_json::json!({ "owner": pr.owner, "repo": pr.repo, "number": pr.number, "after": after }),
                )
                .await?;
            let page = &data["repository"]["pullRequest"]["reviewThreads"];
            for node in page["nodes"].as_array().into_iter().flatten() {
                threads.push(parse_thread(node));
            }
            if page["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                return Ok(threads);
            }
            after = page["pageInfo"]["endCursor"].as_str().map(str::to_string);
        }
    }

    /// Creates a submitted review with line comments; returns the review id.
    pub async fn create_review(
        &self,
        pr: &PrRef,
        commit_id: &str,
        body: &str,
        verdict: Verdict,
        comments: &[NewReviewComment],
    ) -> Result<u64> {
        let path = format!("/repos/{}/{}/pulls/{}/reviews", pr.owner, pr.repo, pr.number);
        let payload = serde_json::json!({ "commit_id": commit_id, "body": body, "event": verdict, "comments": comments });
        let created: serde_json::Value = self.send(reqwest::Method::POST, &path, &payload).await?;
        Ok(created["id"].as_u64().unwrap_or_default())
    }

    /// A comment on a whole file (not part of a review; GitHub has no file comments in reviews).
    pub async fn create_file_comment(&self, pr: &PrRef, commit_id: &str, path: &str, body: &str) -> Result<u64> {
        let route = format!("/repos/{}/{}/pulls/{}/comments", pr.owner, pr.repo, pr.number);
        let payload = serde_json::json!({ "commit_id": commit_id, "path": path, "body": body, "subject_type": "file" });
        let created: serde_json::Value = self.send(reqwest::Method::POST, &route, &payload).await?;
        Ok(created["id"].as_u64().unwrap_or_default())
    }

    /// Replies to the thread containing review comment `comment_id`.
    pub async fn reply(&self, pr: &PrRef, comment_id: u64, body: &str) -> Result<u64> {
        let route = format!("/repos/{}/{}/pulls/{}/comments/{comment_id}/replies", pr.owner, pr.repo, pr.number);
        let created: serde_json::Value = self.send(reqwest::Method::POST, &route, &serde_json::json!({ "body": body })).await?;
        Ok(created["id"].as_u64().unwrap_or_default())
    }

    pub async fn resolve_thread(&self, thread_id: &str) -> Result<()> {
        self.graphql(RESOLVE_MUTATION, serde_json::json!({ "id": thread_id })).await.map(|_| ())
    }

    /// Bodies of the PR's reviews (used to recognise a review whose creation outcome is unknown).
    pub async fn review_bodies(&self, pr: &PrRef) -> Result<Vec<String>> {
        let path = format!("/repos/{}/{}/pulls/{}/reviews", pr.owner, pr.repo, pr.number);
        let reviews: Vec<serde_json::Value> = self.get(&path, &[("per_page", "100".into())]).await?;
        Ok(reviews.iter().filter_map(|r| r["body"].as_str().map(str::to_string)).collect())
    }

    /// Bodies of the PR's most recent review comments (for recognising unknown outcomes).
    pub async fn review_comment_bodies(&self, pr: &PrRef) -> Result<Vec<String>> {
        let path = format!("/repos/{}/{}/pulls/{}/comments", pr.owner, pr.repo, pr.number);
        let query = [("per_page", "100".to_string()), ("sort", "created".into()), ("direction", "desc".into())];
        let comments: Vec<serde_json::Value> = self.get(&path, &query).await?;
        Ok(comments.iter().filter_map(|c| c["body"].as_str().map(str::to_string)).collect())
    }

    /// Open PRs where the user's review is requested, and open PRs they authored.
    pub async fn inbox(&self) -> Result<Vec<crate::inbox::InboxPr>> {
        let mut prs = Vec::new();
        for (query, requested) in [("is:pr is:open archived:false review-requested:@me", true), ("is:pr is:open archived:false author:@me", false)] {
            let data = self.graphql(SEARCH_QUERY, serde_json::json!({ "q": query })).await?;
            for node in data["search"]["nodes"].as_array().into_iter().flatten() {
                let Some(number) = node["number"].as_u64() else { continue };
                let text = |key: &str| node[key].as_str().unwrap_or_default().to_string();
                prs.push(crate::inbox::InboxPr {
                    repo: node["repository"]["nameWithOwner"].as_str().unwrap_or_default().to_string(),
                    number,
                    title: text("title"),
                    url: text("url"),
                    draft: node["isDraft"].as_bool().unwrap_or(false),
                    author: node["author"]["login"].as_str().unwrap_or("ghost").to_string(),
                    base_ref: text("baseRefName"),
                    head_ref: text("headRefName"),
                    head_sha: text("headRefOid"),
                    updated_at: text("updatedAt"),
                    requested,
                    authored: !requested,
                });
            }
        }
        Ok(crate::inbox::merge(prs))
    }

    /// The login of the token's user.
    pub async fn viewer(&self) -> Result<String> {
        let user: serde_json::Value = self.get("/user", &[]).await?;
        Ok(user["login"].as_str().unwrap_or_default().to_string())
    }

    async fn graphql(&self, query: &str, variables: serde_json::Value) -> Result<serde_json::Value> {
        let response: serde_json::Value =
            self.send(reqwest::Method::POST, "/graphql", &serde_json::json!({ "query": query, "variables": variables })).await?;
        if let Some(errors) = response["errors"].as_array().filter(|e| !e.is_empty()) {
            let messages: Vec<&str> = errors.iter().filter_map(|e| e["message"].as_str()).collect();
            return Err(Error::GitHub(format!("GraphQL: {}", messages.join("; "))));
        }
        Ok(response["data"].clone())
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let request = self.http.get(format!("{}{path}", self.api_base)).query(query);
        self.execute(request, "GET", path).await
    }

    async fn send<T: serde::de::DeserializeOwned>(&self, method: reqwest::Method, path: &str, body: &serde_json::Value) -> Result<T> {
        let name = method.to_string();
        let request = self.http.request(method, format!("{}{path}", self.api_base)).json(body);
        self.execute(request, &name, path).await
    }

    /// An HTTP error status becomes [`Error::GitHub`] (the request was definitely rejected);
    /// a transport failure stays [`Error::Http`] (the outcome is unknown).
    async fn execute<T: serde::de::DeserializeOwned>(&self, request: reqwest::RequestBuilder, method: &str, path: &str) -> Result<T> {
        let response = request
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<ApiError>(&body).map(|e| e.message).unwrap_or(body);
            return Err(Error::GitHub(format!("{method} {path}: {status} {message}")));
        }
        Ok(response.json().await?)
    }
}

fn parse_thread(node: &serde_json::Value) -> ReviewThread {
    let number = |v: &serde_json::Value| v.as_u64().map(|n| n as u32);
    ReviewThread {
        id: node["id"].as_str().unwrap_or_default().to_string(),
        resolved: node["isResolved"].as_bool().unwrap_or(false),
        outdated: node["isOutdated"].as_bool().unwrap_or(false),
        path: node["path"].as_str().unwrap_or_default().to_string(),
        line: number(&node["line"]),
        start_line: number(&node["startLine"]),
        original_line: number(&node["originalLine"]),
        side: if node["diffSide"].as_str() == Some("LEFT") { Side::Left } else { Side::Right },
        file_level: node["subjectType"].as_str() == Some("FILE"),
        comments: node["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| ThreadComment {
                id: c["id"].as_str().unwrap_or_default().to_string(),
                database_id: c["databaseId"].as_u64().unwrap_or_default(),
                author: c["author"]["login"].as_str().unwrap_or("ghost").to_string(),
                body: c["body"].as_str().unwrap_or_default().to_string(),
                created_at: c["createdAt"].as_str().unwrap_or_default().to_string(),
                url: c["url"].as_str().unwrap_or_default().to_string(),
            })
            .collect(),
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
