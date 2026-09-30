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

#[tokio::test]
async fn serves_side_by_side_rows_and_remembers_ignore_patterns_across_restarts() {
    let origin = OriginRepo::init();
    origin.write("src/Order.php", "<?php\n$a = 1;\n$b = 2;\n$c = 3;\n");
    origin.commit("base");
    origin.checkout_new("stack/1");
    origin.write("src/Order.php", "<?php\n$a = 1;\n$b = 20;\n$c = 3;\n$d = 4;\n");
    let head = origin.commit("change");
    origin.publish_pr(7, "stack/1");

    let url = origin.url();
    let server = MockServer::start().await;
    mount(&server, "/repos/acme/shop/pulls/7", None, support::pull_request_json(7, "main", "stack/1", &head, &url)).await;
    mount(&server, "/repos/acme/shop/pulls", Some(("base", "stack/1")), serde_json::json!([])).await;

    let data = tempfile::tempdir().unwrap();
    let api = server.uri();
    let open_service = || {
        PrService::new(
            GitHubClient::new(api.clone(), "t").unwrap(),
            RepoStore::new(data.path().join("repos"), None),
            Cache::open(&data.path().join("cache.sqlite")).unwrap(),
            Highlighter::new(),
        )
    };
    let service = open_service();
    let pr = PrRef::parse("acme/shop#7").unwrap();
    let snapshot = service.fetch(service.discover(&pr).await.unwrap()).unwrap();
    let view = service.range(&snapshot, 0, 0).unwrap();

    let rows = service.split_rows(&snapshot, &view, 0).unwrap();
    assert_eq!(rows.len() as u32 + 1, view.summary.files[0].split_rows);
    let texts: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.os.iter().map(|s| s.1.as_str()).collect(), r.ns.iter().map(|s| s.1.as_str()).collect()))
        .collect();
    assert_eq!(texts[2], ("$b = 2;".to_string(), "$b = 20;".to_string()));
    assert_eq!(texts[4], (String::new(), "$d = 4;".to_string()));
    assert_eq!((rows[2].oa, rows[2].na), (Some(0), Some(0)));
    assert!(rows[2].ns.iter().any(|(class, _)| *class != 0), "highlighted");

    service.set_ignore_patterns("acme/shop", &["src/Generated/**".to_string()]).unwrap();
    drop(service);

    // A fresh service (app restart) offline: settings and highlighted files come from SQLite.
    drop(server);
    let restarted = open_service();
    assert_eq!(restarted.ignore_patterns("acme/shop").unwrap(), vec!["src/Generated/**"]);
    assert!(restarted.ignore_patterns("other/repo").unwrap().is_empty());
    let cached = restarted.cached_stack(&pr).unwrap().unwrap();
    let cached_view = restarted.cached_range(&cached, 0, 0).unwrap().unwrap();
    assert_eq!(restarted.split_rows(&cached, &cached_view, 0).unwrap(), rows);
}