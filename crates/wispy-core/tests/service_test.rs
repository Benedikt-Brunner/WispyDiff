mod support;

use support::OriginRepo;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::pr_ref::PrRef;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;

#[tokio::test]
async fn loads_a_pr_and_reopens_it_from_the_cache_offline() {
    let origin = OriginRepo::init();
    origin.write("src/Order.php", "<?php\n$state = 'open';\n");
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("src/Order.php", "<?php\n$state = 'picked';\n");
    let head = origin.commit("pick");
    origin.publish_pr(42, "feature");

    let server = MockServer::start().await;
    Mock::given(path("/repos/acme/shop/pulls/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(support::pull_request_json(
            42,
            "main",
            "feature",
            &head,
            &origin.url(),
        )))
        .mount(&server)
        .await;

    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new(server.uri(), "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let pr = PrRef::parse("acme/shop#42").unwrap();

    assert!(service.cached(&pr).unwrap().is_none());

    let pull_request = service.fetch_pull_request(&pr).await.unwrap();
    let loaded = service.load(&pr, pull_request).unwrap();
    assert_eq!(loaded.pull_request.head_sha, head);
    assert_eq!(loaded.view.summary.head_sha, head);
    assert_eq!(loaded.view.summary.files.len(), 1);

    // Offline: no server needed, same view comes back from SQLite.
    drop(server);
    let cached = service.cached(&pr).unwrap().expect("cached after load");
    assert_eq!(cached.pull_request, loaded.pull_request);
    assert_eq!(*cached.view, *loaded.view);
}

#[test]
fn cache_round_trips_snapshots_and_views_by_sha() {
    use wispy_core::cache::PrSnapshot;
    use wispy_core::github::PullRequest;
    use wispy_core::model::DiffView;

    let cache = Cache::open_in_memory().unwrap();
    let pr = PrRef::parse("acme/shop#1").unwrap();
    let view = DiffView::build("aaa", "bbb", &[], &Default::default(), &Highlighter::new());
    let snapshot = PrSnapshot {
        pull_request: PullRequest {
            number: 1,
            title: "t".into(),
            state: "open".into(),
            draft: false,
            html_url: "u".into(),
            author: "me".into(),
            base_ref: "main".into(),
            base_sha: "aaa".into(),
            head_ref: "f".into(),
            head_sha: "bbb".into(),
            clone_url: "c".into(),
        },
        diff_from: "aaa".into(),
        diff_to: "bbb".into(),
    };

    cache.put_diff_view("acme/shop", &view).unwrap();
    cache.put_snapshot(&pr, &snapshot).unwrap();

    assert_eq!(cache.snapshot(&pr).unwrap(), Some(snapshot));
    assert_eq!(cache.diff_view("acme/shop", "aaa", "bbb").unwrap(), Some(view));
    assert_eq!(cache.diff_view("acme/shop", "aaa", "ccc").unwrap(), None);
    assert_eq!(cache.diff_view("other/repo", "aaa", "bbb").unwrap(), None);
}
