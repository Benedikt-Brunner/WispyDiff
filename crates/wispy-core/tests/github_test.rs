mod support;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::github::GitHubClient;
use wispy_core::pr_ref::PrRef;
use wispy_core::Error;

fn pr_ref(number: u64) -> PrRef {
    PrRef { owner: "acme".into(), repo: "shop".into(), number }
}

#[tokio::test]
async fn fetches_pull_request_metadata_with_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/shop/pulls/12"))
        .and(header("authorization", "Bearer secret-token"))
        // The full media type adds the rendered description.
        .and(header("accept", "application/vnd.github.full+json"))
        .respond_with(ResponseTemplate::new(200).set_body_json({
            let mut body = support::pull_request_json(12, "main", "feature/pick-lists", "abc123", "https://github.com/acme/shop.git");
            body["body_html"] = "<p>Adds <strong>pick lists</strong>.</p>".into();
            body
        }))
        .expect(1)
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), "secret-token").unwrap();
    let pr = client.pull_request(&pr_ref(12)).await.unwrap();

    assert_eq!(pr.number, 12);
    assert_eq!(pr.title, "PR 12");
    assert_eq!(pr.author, "octocat");
    assert_eq!(pr.base_ref, "main");
    assert_eq!(pr.head_ref, "feature/pick-lists");
    assert_eq!(pr.head_sha, "abc123");
    assert_eq!(pr.clone_url, "https://github.com/acme/shop.git");
    assert_eq!(pr.state, "open");
    assert_eq!(pr.body_html, "<p>Adds <strong>pick lists</strong>.</p>");
}

#[tokio::test]
async fn a_pull_request_without_a_description_has_an_empty_one() {
    let server = MockServer::start().await;
    let mut body = support::pull_request_json(4, "main", "x", "abc", "https://github.com/acme/shop.git");
    body["body_html"] = serde_json::Value::Null;
    Mock::given(path("/repos/acme/shop/pulls/4")).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(&server).await;

    let client = GitHubClient::new(server.uri(), "t").unwrap();
    assert_eq!(client.pull_request(&pr_ref(4)).await.unwrap().body_html, "");
}

#[tokio::test]
async fn reports_merged_pull_requests_as_merged() {
    let server = MockServer::start().await;
    let mut body = support::pull_request_json(3, "main", "x", "abc", "https://github.com/acme/shop.git");
    body["state"] = "closed".into();
    body["merged_at"] = "2026-09-01T10:00:00Z".into();
    Mock::given(path("/repos/acme/shop/pulls/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), "t").unwrap();
    assert_eq!(client.pull_request(&pr_ref(3)).await.unwrap().state, "merged");
}

#[tokio::test]
async fn tells_whether_auto_merge_is_enabled() {
    let server = MockServer::start().await;
    let mut enabled = support::pull_request_json(5, "main", "x", "abc", "https://github.com/acme/shop.git");
    enabled["auto_merge"] = serde_json::json!({ "enabled_by": { "login": "octocat" }, "merge_method": "squash" });
    let mut disabled = support::pull_request_json(6, "main", "y", "def", "https://github.com/acme/shop.git");
    disabled["auto_merge"] = serde_json::Value::Null;
    Mock::given(path("/repos/acme/shop/pulls/5")).respond_with(ResponseTemplate::new(200).set_body_json(enabled)).mount(&server).await;
    Mock::given(path("/repos/acme/shop/pulls/6")).respond_with(ResponseTemplate::new(200).set_body_json(disabled)).mount(&server).await;

    let client = GitHubClient::new(server.uri(), "t").unwrap();
    assert!(client.pull_request(&pr_ref(5)).await.unwrap().auto_merge);
    assert!(!client.pull_request(&pr_ref(6)).await.unwrap().auto_merge);
}

#[tokio::test]
async fn surfaces_github_error_messages() {
    let server = MockServer::start().await;
    Mock::given(path("/repos/acme/shop/pulls/404"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({ "message": "Not Found" })))
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), "t").unwrap();
    let err = client.pull_request(&pr_ref(404)).await.unwrap_err();
    match err {
        Error::GitHub(message) => assert!(message.contains("404") && message.contains("Not Found"), "{message}"),
        other => panic!("unexpected error {other:?}"),
    }
}
