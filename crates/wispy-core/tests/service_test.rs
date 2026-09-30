mod support;

use support::OriginRepo;
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::pr_ref::PrRef;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;

async fn mount(server: &MockServer, route: &str, query: Option<(&str, &str)>, body: serde_json::Value) {
    let mut mock = Mock::given(path(route));
    if let Some((key, value)) = query {
        mock = mock.and(query_param(key, value));
    }
    mock.respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(server).await;
}

#[tokio::test]
async fn opens_a_stack_and_reopens_any_member_offline() {
    let origin = OriginRepo::init();
    origin.write("src/Order.php", "<?php\n$state = 'open';\n");
    origin.commit("base");
    origin.checkout_new("stack/1");
    origin.write("src/Order.php", "<?php\n$state = 'picked';\n");
    let head1 = origin.commit("pick");
    origin.publish_pr(41, "stack/1");
    origin.checkout_new("stack/2");
    origin.write("src/Bin.php", "<?php\n$bin = 'A-1';\n");
    let head2 = origin.commit("bins");
    origin.publish_pr(42, "stack/2");

    let url = origin.url();
    let pr41 = support::pull_request_json(41, "main", "stack/1", &head1, &url);
    let pr42 = support::pull_request_json(42, "stack/1", "stack/2", &head2, &url);
    let server = MockServer::start().await;
    mount(&server, "/repos/acme/shop/pulls/42", None, pr42.clone()).await;
    mount(&server, "/repos/acme/shop/pulls", Some(("head", "acme:stack/1")), serde_json::json!([pr41])).await;
    mount(&server, "/repos/acme/shop/pulls", Some(("base", "stack/2")), serde_json::json!([])).await;

    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new(server.uri(), "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let pr42_ref = PrRef::parse("acme/shop#42").unwrap();
    assert!(service.cached_stack(&pr42_ref).unwrap().is_none());

    let stack = service.discover(&pr42_ref).await.unwrap();
    let snapshot = service.fetch(stack).unwrap();
    assert_eq!(snapshot.heads, vec![head1.clone(), head2.clone()]);
    assert_eq!(snapshot.stack.focus, 1);

    let whole = service.range(&snapshot, 0, 1).unwrap();
    assert_eq!(whole.summary.files.len(), 2);
    assert!(service.precompute(&snapshot, 1, 1).unwrap(), "not cached yet");
    assert!(!service.precompute(&snapshot, 1, 1).unwrap(), "cached now");

    // Offline: the stack is known from either PR, focused on the one asked for.
    drop(server);
    let from_41 = service.cached_stack(&PrRef::parse("acme/shop#41").unwrap()).unwrap().unwrap();
    assert_eq!(from_41.stack.focus, 0);
    assert_eq!(from_41.heads, snapshot.heads);
    let cached = service.cached_range(&from_41, 0, 1).unwrap().expect("cached range");
    assert_eq!(cached, *whole);
    assert!(service.cached_range(&from_41, 0, 0).unwrap().is_none(), "never computed");
}
