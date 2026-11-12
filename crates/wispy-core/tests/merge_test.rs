use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::github::GitHubClient;
use wispy_core::merge::{self, issue_references, MergeMethod, MergeOutcome, MergeRequest};
use wispy_core::pr_ref::PrRef;

fn pr_ref() -> PrRef {
    PrRef { owner: "acme".into(), repo: "shop".into(), number: 12 }
}

fn info_json(merge_state: &str, auto_merge_allowed: bool) -> serde_json::Value {
    serde_json::json!({ "data": { "repository": {
        "squashMergeAllowed": true, "mergeCommitAllowed": true, "rebaseMergeAllowed": false, "autoMergeAllowed": auto_merge_allowed,
        "defaultBranchRef": { "name": "main" },
        "pullRequest": {
            "id": "PR_node12", "number": 12, "title": "Add pick lists", "state": "OPEN", "isDraft": false,
            "body": "Adds pick lists.\n\nFixes #1234\n- Related to acme/other#7\nSee the design doc.\nCloses #9, #10",
            "baseRefName": "stack/1", "mergeStateStatus": merge_state, "viewerDefaultMergeMethod": "MERGE",
            "autoMergeRequest": null,
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "PENDING" } } }] }
        }
    } } })
}

async fn mount_info(server: &MockServer, merge_state: &str, auto_merge_allowed: bool) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("viewerDefaultMergeMethod"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info_json(merge_state, auto_merge_allowed)))
        .mount(server)
        .await;
}

fn request() -> MergeRequest {
    MergeRequest { method: MergeMethod::Squash, title: " Add pick lists (#12) ".into(), body: "Fixes #1234".into() }
}

#[test]
fn keeps_only_the_lines_that_link_issues() {
    let body = "Intro text that mentions #5 in passing.\n\n* Fixes #1234\n- **Related to** #77\nCloses: https://github.com/acme/shop/issues/4\nfixes #1234\nFix the flaky test first\nResolves #3 and #4.";
    assert_eq!(
        issue_references(body),
        "Fixes #1234\nCloses: https://github.com/acme/shop/issues/4\nfixes #1234\nResolves #3 and #4."
    );
}

#[tokio::test]
async fn info_prefers_the_viewers_default_method_and_prefills_the_message() {
    let server = MockServer::start().await;
    mount_info(&server, "BLOCKED", true).await;

    let client = GitHubClient::new(server.uri(), "token").unwrap();
    let info = merge::info(&client, &pr_ref()).await.unwrap();

    assert_eq!(info.methods, vec![MergeMethod::Merge, MergeMethod::Squash]);
    assert_eq!(info.base_ref, "stack/1");
    assert_eq!(info.default_branch, "main");
    assert_eq!(info.checks, "PENDING");
    assert!(!info.ready());
    assert_eq!(info.title, "Add pick lists (#12)");
    assert_eq!(info.body, "Fixes #1234\nRelated to acme/other#7\nCloses #9, #10");
}

#[tokio::test]
async fn merges_at_once_when_green() {
    let server = MockServer::start().await;
    mount_info(&server, "CLEAN", true).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("mergePullRequest"))
        .and(body_partial_json(serde_json::json!({ "variables": {
            "id": "PR_node12", "method": "SQUASH", "title": "Add pick lists (#12)", "body": "Fixes #1234", "head": "abc123"
        } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "mergePullRequest": { "pullRequest": { "merged": true } } } })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), "token").unwrap();
    let outcome = merge::merge(&client, &pr_ref(), "abc123", &request()).await.unwrap();

    assert_eq!(outcome, MergeOutcome::Merged);
}

#[tokio::test]
async fn enables_auto_merge_while_checks_run() {
    let server = MockServer::start().await;
    mount_info(&server, "BLOCKED", true).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("enablePullRequestAutoMerge"))
        .and(body_partial_json(serde_json::json!({ "variables": { "id": "PR_node12", "method": "SQUASH", "head": "abc123" } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "enablePullRequestAutoMerge": { "pullRequest": { "autoMergeRequest": { "enabledAt": "now" } } } } })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(body_string_contains("mergePullRequest(")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&server).await;

    let client = GitHubClient::new(server.uri(), "token").unwrap();
    let outcome = merge::merge(&client, &pr_ref(), "abc123", &request()).await.unwrap();

    assert_eq!(outcome, MergeOutcome::AutoMergeEnabled);
}

#[tokio::test]
async fn refuses_when_not_ready_and_the_repo_has_no_auto_merge() {
    let server = MockServer::start().await;
    mount_info(&server, "BLOCKED", false).await;

    let client = GitHubClient::new(server.uri(), "token").unwrap();
    let error = merge::merge(&client, &pr_ref(), "abc123", &request()).await.unwrap_err();

    assert!(error.to_string().contains("doesn't allow auto-merge"), "{error}");
}

#[tokio::test]
async fn refuses_a_method_the_repo_disallows() {
    let server = MockServer::start().await;
    mount_info(&server, "CLEAN", true).await;

    let client = GitHubClient::new(server.uri(), "token").unwrap();
    let rebase = MergeRequest { method: MergeMethod::Rebase, ..request() };
    let error = merge::merge(&client, &pr_ref(), "abc123", &rebase).await.unwrap_err();

    assert!(error.to_string().contains("doesn't allow rebase merges"), "{error}");
}
