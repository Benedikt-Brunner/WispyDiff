//! Merging a PR from the submit sheet: at once when its checks are green, otherwise by enabling
//! auto-merge, with the merge method the repository allows and a commit message to edit.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::github::GitHubClient;
use crate::pr_ref::PrRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

/// Everything the sheet shows before merging a PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeInfo {
    /// GraphQL node id (the merge mutations take it).
    pub id: String,
    pub base_ref: String,
    pub default_branch: String,
    /// The methods the repository allows, the preferred one first.
    pub methods: Vec<MergeMethod>,
    pub auto_merge_allowed: bool,
    /// Auto-merge is already enabled on the PR.
    pub auto_merge_enabled: bool,
    /// GitHub's `mergeStateStatus` (`CLEAN`, `BLOCKED`, `BEHIND`, `UNSTABLE`, `DIRTY`, …).
    pub merge_state: String,
    /// The head commit's combined check state (`SUCCESS`, `PENDING`, `FAILURE`, …; empty without checks).
    pub checks: String,
    pub draft: bool,
    pub open: bool,
    /// Prefilled commit title: the PR's title with its number, as GitHub writes squash commits.
    pub title: String,
    /// Prefilled commit body: the issue references in the PR's description.
    pub body: String,
}

impl MergeInfo {
    /// Green and mergeable: merges at once rather than enabling auto-merge.
    pub fn ready(&self) -> bool {
        matches!(self.merge_state.as_str(), "CLEAN" | "HAS_HOOKS")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    pub method: MergeMethod,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeOutcome {
    Merged,
    AutoMergeEnabled,
}

const INFO_QUERY: &str = "query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    squashMergeAllowed mergeCommitAllowed rebaseMergeAllowed autoMergeAllowed
    defaultBranchRef { name }
    pullRequest(number: $number) {
      id number title body state isDraft baseRefName mergeStateStatus viewerDefaultMergeMethod
      autoMergeRequest { enabledAt }
      commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
    }
  }
}";

const MERGE_MUTATION: &str = "mutation($id: ID!, $method: PullRequestMergeMethod!, $title: String, $body: String, $head: GitObjectID) {
  mergePullRequest(input: { pullRequestId: $id, mergeMethod: $method, commitHeadline: $title, commitBody: $body, expectedHeadOid: $head }) {
    pullRequest { merged }
  }
}";

const AUTO_MERGE_MUTATION: &str = "mutation($id: ID!, $method: PullRequestMergeMethod!, $title: String, $body: String, $head: GitObjectID) {
  enablePullRequestAutoMerge(input: { pullRequestId: $id, mergeMethod: $method, commitHeadline: $title, commitBody: $body, expectedHeadOid: $head }) {
    pullRequest { autoMergeRequest { enabledAt } }
  }
}";

/// What the sheet needs to offer merging `pr`.
pub async fn info(github: &GitHubClient, pr: &PrRef) -> Result<MergeInfo> {
    let data = github
        .graphql(INFO_QUERY, serde_json::json!({ "owner": pr.owner, "repo": pr.repo, "number": pr.number }))
        .await?;
    let repo = &data["repository"];
    let node = &repo["pullRequest"];
    if node.is_null() {
        return Err(Error::GitHub(format!("{}/{}#{} not found", pr.owner, pr.repo, pr.number)));
    }
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let allowed: Vec<MergeMethod> = [
        (MergeMethod::Squash, "squashMergeAllowed"),
        (MergeMethod::Merge, "mergeCommitAllowed"),
        (MergeMethod::Rebase, "rebaseMergeAllowed"),
    ]
    .into_iter()
    .filter(|(_, key)| repo[*key].as_bool() == Some(true))
    .map(|(method, _)| method)
    .collect();
    // The method GitHub's merge button would preselect for this user, if the repo allows it.
    let preferred = serde_json::from_value::<MergeMethod>(node["viewerDefaultMergeMethod"].clone()).ok().filter(|m| allowed.contains(m));
    let methods = preferred.into_iter().chain(allowed.iter().copied().filter(|m| Some(*m) != preferred)).collect();
    let title = text(&node["title"]);
    Ok(MergeInfo {
        id: text(&node["id"]),
        base_ref: text(&node["baseRefName"]),
        default_branch: text(&repo["defaultBranchRef"]["name"]),
        methods,
        auto_merge_allowed: repo["autoMergeAllowed"].as_bool().unwrap_or(false),
        auto_merge_enabled: !node["autoMergeRequest"].is_null(),
        merge_state: text(&node["mergeStateStatus"]),
        checks: text(&node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"]),
        draft: node["isDraft"].as_bool().unwrap_or(false),
        open: node["state"].as_str() == Some("OPEN"),
        title: format!("{title} (#{})", pr.number),
        body: issue_references(node["body"].as_str().unwrap_or_default()),
    })
}

/// Merges `pr` at `head` now if its checks are green and nothing blocks it, else enables
/// auto-merge. The state is read again first: a review just submitted may have unblocked it.
pub async fn merge(github: &GitHubClient, pr: &PrRef, head: &str, request: &MergeRequest) -> Result<MergeOutcome> {
    let info = info(github, pr).await?;
    if !info.open {
        return Err(Error::GitHub(format!("#{} is not open", pr.number)));
    }
    if info.draft {
        return Err(Error::GitHub(format!("#{} is a draft", pr.number)));
    }
    if !info.methods.contains(&request.method) {
        return Err(Error::GitHub(format!("the repository doesn't allow {:?} merges", request.method).to_lowercase()));
    }
    // Rebasing keeps the commits as they are: no message to set.
    let message = |s: &str| (request.method != MergeMethod::Rebase).then(|| s.to_string());
    let variables = serde_json::json!({
        "id": info.id, "method": request.method, "title": message(request.title.trim()), "body": message(request.body.trim()), "head": head,
    });
    if info.ready() {
        github.graphql(MERGE_MUTATION, variables).await?;
        return Ok(MergeOutcome::Merged);
    }
    if !info.auto_merge_allowed {
        return Err(Error::GitHub(format!("#{} can't be merged yet ({}) and the repository doesn't allow auto-merge", pr.number, describe(&info))));
    }
    github.graphql(AUTO_MERGE_MUTATION, variables).await?;
    Ok(MergeOutcome::AutoMergeEnabled)
}

fn describe(info: &MergeInfo) -> String {
    match info.merge_state.as_str() {
        "BLOCKED" => "blocked".into(),
        "BEHIND" => "behind its base".into(),
        "DIRTY" => "conflicts".into(),
        "UNSTABLE" => "checks failing".into(),
        _ if info.checks == "PENDING" => "checks running".into(),
        other => other.to_lowercase(),
    }
}

/// The lines of a PR description that link issues ("Fixes #12", "Related to acme/shop#3",
/// "Closes https://github.com/acme/shop/issues/4"), one per line, in order, without repeats.
pub fn issue_references(body: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved", "related to", "relates to", "part of",
    ];
    let mut found: Vec<String> = Vec::new();
    for line in body.lines() {
        // Markdown list markers and emphasis around the reference.
        let line = line.trim().trim_start_matches(['-', '*', '+', '>']).trim().trim_matches('*').trim_matches('_').trim();
        let lower = line.to_lowercase();
        let Some(keyword) = KEYWORDS.iter().filter(|k| lower.starts_with(*k)).max_by_key(|k| k.len()) else { continue };
        let rest = line[keyword.len()..].trim_start_matches(':').trim();
        let is_ref = |r: &str| {
            let r = r.trim_end_matches([',', '.', ';']);
            r.split_once('#').is_some_and(|(repo, n)| (repo.is_empty() || repo.split('/').count() == 2) && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                || (r.starts_with("https://github.com/") && (r.contains("/issues/") || r.contains("/pull/")))
        };
        let refs: Vec<&str> = rest.split_whitespace().filter(|w| !w.eq_ignore_ascii_case("and")).collect();
        if !refs.is_empty() && refs.iter().all(|r| is_ref(r)) && !found.iter().any(|f| f == line) {
            found.push(line.to_string());
        }
    }
    found.join("\n")
}
