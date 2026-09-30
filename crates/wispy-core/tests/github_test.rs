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
        .and(header("accept", "application/vnd.github+json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(support::pull_request_json(
            12,
            "main",
            "feature/pick-lists",
            "abc123",
            "https://github.com/acme/shop.git",
        )))
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
