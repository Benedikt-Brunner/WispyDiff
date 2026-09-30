mod support;

use serde_json::Value;
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::github::GitHubClient;
use wispy_core::pr_ref::PrRef;
use wispy_core::stack::discover_stack;
use wispy_core::Error;

const URL: &str = "https://github.com/acme/shop.git";

fn pr(number: u64, base: &str, head: &str) -> Value {
    support::pull_request_json(number, base, head, &format!("sha{number}"), URL)
}

fn fork_pr(number: u64, base: &str, head: &str) -> Value {
    let mut value = pr(number, base, head);
    value["head"]["repo"] = support::repo_json("https://github.com/other/shop.git", "other/shop");
    value
}

async fn serve_pr(server: &MockServer, body: Value) {
    let number = body["number"].as_u64().unwrap();
    Mock::given(path(format!("/repos/acme/shop/pulls/{number}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn serve_list(server: &MockServer, key: &str, value: &str, prs: Vec<Value>) {
    Mock::given(path("/repos/acme/shop/pulls"))
        .and(query_param("state", "open"))
        .and(query_param(key, value))
        .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(prs)))
        .mount(server)
        .await;
}

fn numbers(stack: &wispy_core::stack::Stack) -> Vec<u64> {
    stack.prs.iter().map(|p| p.number).collect()
}

#[tokio::test]
async fn discovers_the_whole_stack_from_a_middle_pr() {
    let server = MockServer::start().await;
    serve_pr(&server, pr(3, "stack/2", "stack/3")).await;
    serve_list(&server, "head", "acme:stack/2", vec![pr(2, "stack/1", "stack/2")]).await;
    serve_list(&server, "head", "acme:stack/1", vec![pr(1, "main", "stack/1")]).await;
    serve_list(&server, "base", "stack/3", vec![pr(4, "stack/3", "stack/4")]).await;
    serve_list(&server, "base", "stack/4", vec![]).await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let stack = discover_stack(&github, &PrRef::parse("acme/shop#3").unwrap()).await.unwrap();

    assert_eq!(numbers(&stack), vec![1, 2, 3, 4]);
    assert_eq!(stack.focus, 2);
}

#[tokio::test]
async fn a_pr_on_the_default_branch_without_children_is_a_stack_of_one() {
    let server = MockServer::start().await;
    serve_pr(&server, pr(5, "main", "feature")).await;
    serve_list(&server, "base", "feature", vec![]).await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let stack = discover_stack(&github, &PrRef::parse("acme/shop#5").unwrap()).await.unwrap();

    assert_eq!(numbers(&stack), vec![5]);
    assert_eq!(stack.focus, 0);
}

#[tokio::test]
async fn stops_at_a_base_branch_no_open_pr_provides() {
    let server = MockServer::start().await;
    serve_pr(&server, pr(8, "release/2026", "hotfix")).await;
    serve_list(&server, "head", "acme:release/2026", vec![]).await;
    serve_list(&server, "base", "hotfix", vec![]).await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let stack = discover_stack(&github, &PrRef::parse("acme/shop#8").unwrap()).await.unwrap();

    assert_eq!(numbers(&stack), vec![8]);
}

#[tokio::test]
async fn ignores_forks_and_follows_the_oldest_child_when_branching() {
    let server = MockServer::start().await;
    serve_pr(&server, pr(1, "main", "stack/1")).await;
    serve_list(
        &server,
        "base",
        "stack/1",
        vec![fork_pr(3, "stack/1", "stack/2"), pr(9, "stack/1", "b"), pr(7, "stack/1", "a")],
    )
    .await;
    serve_list(&server, "base", "a", vec![]).await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let stack = discover_stack(&github, &PrRef::parse("acme/shop#1").unwrap()).await.unwrap();

    assert_eq!(numbers(&stack), vec![1, 7]);
}

#[tokio::test]
async fn reports_cycles_instead_of_looping() {
    let server = MockServer::start().await;
    serve_pr(&server, pr(1, "b", "a")).await;
    serve_list(&server, "head", "acme:b", vec![pr(2, "a", "b")]).await;
    serve_list(&server, "head", "acme:a", vec![pr(1, "b", "a")]).await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let err = discover_stack(&github, &PrRef::parse("acme/shop#1").unwrap()).await.unwrap_err();

    assert!(matches!(&err, Error::GitHub(m) if m.contains("cycle")), "{err:?}");
}

#[tokio::test]
async fn does_not_look_for_children_of_a_fork_pr() {
    let server = MockServer::start().await;
    serve_pr(&server, fork_pr(4, "main", "feature")).await;
    // No list mocks: any child lookup would 404 and fail the test.

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let stack = discover_stack(&github, &PrRef::parse("acme/shop#4").unwrap()).await.unwrap();

    assert_eq!(numbers(&stack), vec![4]);
}
