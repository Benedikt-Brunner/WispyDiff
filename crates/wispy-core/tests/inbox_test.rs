mod support;

use support::OriginRepo;
use wiremock::matchers::{body_string_contains, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::inbox::{group, merge, InboxPr, MyReview};
use wispy_core::progress::{Checkpoint, CheckpointEntry};
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;

fn inbox_pr(repo: &str, number: u64, base: &str, head: &str, updated: &str) -> InboxPr {
    InboxPr {
        repo: repo.into(),
        number,
        title: format!("PR {number}"),
        url: String::new(),
        draft: false,
        author: "someone".into(),
        base_ref: base.into(),
        head_ref: head.into(),
        head_sha: format!("sha{number}"),
        updated_at: updated.into(),
        requested: true,
        authored: false,
        my_review: None,
        body_html: String::new(),
        auto_merge: false,
        stats: None,
    }
}

fn shape(groups: &[wispy_core::inbox::InboxGroup]) -> Vec<(String, Vec<u64>)> {
    groups.iter().map(|g| (g.repo.clone(), g.prs.iter().map(|p| p.number).collect())).collect()
}

#[test]
fn groups_stacked_prs_per_repo_bottom_first_most_recent_group_first() {
    let groups = group(vec![
        inbox_pr("acme/shop", 12, "stack/1", "stack/2", "2026-09-01"),
        inbox_pr("acme/shop", 11, "main", "stack/1", "2026-09-03"),
        inbox_pr("acme/shop", 13, "stack/2", "stack/3", "2026-09-02"),
        inbox_pr("acme/shop", 20, "main", "solo", "2026-09-05"),
        inbox_pr("acme/app", 5, "main", "stack/1", "2026-08-01"),
    ]);
    assert_eq!(
        shape(&groups),
        vec![
            ("acme/shop".into(), vec![20]),
            ("acme/shop".into(), vec![11, 12, 13]),
            ("acme/app".into(), vec![5]),
        ]
    );
    assert_eq!(groups[1].updated_at, "2026-09-03");
}

#[test]
fn a_stack_missing_its_bottom_still_groups_what_is_there() {
    // Only the top two of a teammate's stack request my review.
    let groups = group(vec![
        inbox_pr("acme/shop", 33, "stack/2", "stack/3", "1"),
        inbox_pr("acme/shop", 34, "stack/3", "stack/4", "1"),
    ]);
    assert_eq!(shape(&groups), vec![("acme/shop".into(), vec![33, 34])]);
}

#[test]
fn prs_branching_off_one_pr_stay_in_its_group_depth_first() {
    let groups = group(vec![
        inbox_pr("acme/shop", 1, "main", "a", "1"),
        inbox_pr("acme/shop", 3, "a", "c", "1"),
        inbox_pr("acme/shop", 4, "b", "d", "1"),
        inbox_pr("acme/shop", 2, "a", "b", "1"),
    ]);
    assert_eq!(shape(&groups), vec![("acme/shop".into(), vec![1, 2, 4, 3])]);
    assert_eq!(groups[0].parents, vec![None, Some(0), Some(1), Some(0)]);
    assert_eq!(groups[0].branches(), 2);
}

#[test]
fn a_plain_stack_is_one_branch_also_when_cached_before_trees() {
    let mut groups = group(vec![inbox_pr("acme/shop", 1, "main", "a", "1"), inbox_pr("acme/shop", 2, "a", "b", "1")]);
    assert_eq!(groups[0].parents, vec![None, Some(0)]);
    assert_eq!(groups[0].branches(), 1);
    groups[0].parents.clear();
    assert_eq!((groups[0].parent(0), groups[0].parent(1)), (None, Some(0)));
    assert_eq!(groups[0].branches(), 1);
}

#[test]
fn merges_requested_and_authored_results() {
    let mut authored = inbox_pr("acme/shop", 7, "main", "x", "1");
    authored.requested = false;
    authored.authored = true;
    let merged = merge(vec![inbox_pr("acme/shop", 7, "main", "x", "1"), authored]);
    assert_eq!(merged.len(), 1);
    assert!(merged[0].requested && merged[0].authored);
}

#[tokio::test]
async fn searches_review_requests_and_authored_prs() {
    let server = MockServer::start().await;
    let node = |number: u64| {
        serde_json::json!({
            "number": number, "title": format!("PR {number}"), "url": "u", "isDraft": false, "updatedAt": "2026-09-30T00:00:00Z",
            "headRefName": format!("h{number}"), "baseRefName": "main", "headRefOid": format!("sha{number}"),
            "author": { "login": "me" }, "repository": { "nameWithOwner": "acme/shop" }
        })
    };
    Mock::given(path("/graphql"))
        .and(body_string_contains("review-requested:@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": [node(1), node(2)] } } })))
        .mount(&server)
        .await;
    Mock::given(path("/graphql"))
        .and(body_string_contains("author:@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": [node(2), node(3)] } } })))
        .mount(&server)
        .await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let mut prs = github.inbox().await.unwrap();
    prs.sort_by_key(|p| p.number);
    let flags: Vec<(u64, bool, bool)> = prs.iter().map(|p| (p.number, p.requested, p.authored)).collect();
    assert_eq!(flags, vec![(1, true, false), (2, true, true), (3, false, true)]);
}

#[tokio::test]
async fn search_results_carry_my_latest_submitted_review() {
    let server = MockServer::start().await;
    let node = |number: u64, review: serde_json::Value| {
        serde_json::json!({
            "number": number, "title": format!("PR {number}"), "url": "u", "isDraft": false, "updatedAt": "2026-09-30T00:00:00Z",
            "headRefName": format!("h{number}"), "baseRefName": "main", "headRefOid": format!("sha{number}"),
            "author": { "login": "someone" }, "repository": { "nameWithOwner": "acme/shop" },
            "viewerLatestReview": review, "bodyHTML": format!("<p>About {number}</p>")
        })
    };
    let nodes = serde_json::json!([
        node(1, serde_json::json!({ "state": "APPROVED", "submittedAt": "2026-09-29T00:00:00Z", "commit": { "oid": "old1" } })),
        node(2, serde_json::json!({ "state": "PENDING", "submittedAt": null, "commit": { "oid": "sha2" } })),
        node(3, serde_json::Value::Null),
    ]);
    Mock::given(path("/graphql"))
        .and(body_string_contains("review-requested:@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": nodes } } })))
        .mount(&server)
        .await;
    Mock::given(path("/graphql"))
        .and(body_string_contains("author:@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": [] } } })))
        .mount(&server)
        .await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let mut prs = github.inbox().await.unwrap();
    prs.sort_by_key(|p| p.number);
    assert_eq!(prs[0].body_html, "<p>About 1</p>", "search results carry the rendered description");
    let reviews: Vec<Option<MyReview>> = prs.into_iter().map(|p| p.my_review).collect();
    assert_eq!(
        reviews,
        vec![
            Some(MyReview { state: "APPROVED".into(), commit: "old1".into(), submitted_at: "2026-09-29T00:00:00Z".into() }),
            None, // a pending review isn't submitted yet
            None,
        ]
    );
}

#[tokio::test]
async fn search_results_tell_whether_auto_merge_is_enabled() {
    let server = MockServer::start().await;
    let node = |number: u64, auto: serde_json::Value| {
        serde_json::json!({
            "number": number, "title": format!("PR {number}"), "url": "u", "isDraft": false, "updatedAt": "2026-09-30T00:00:00Z",
            "headRefName": format!("h{number}"), "baseRefName": "main", "headRefOid": format!("sha{number}"),
            "author": { "login": "someone" }, "repository": { "nameWithOwner": "acme/shop" }, "viewerLatestReview": null,
            "autoMergeRequest": auto
        })
    };
    let nodes = serde_json::json!([node(1, serde_json::json!({ "enabledAt": "2026-09-29T00:00:00Z" })), node(2, serde_json::Value::Null)]);
    Mock::given(path("/graphql"))
        .and(body_string_contains("autoMergeRequest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": nodes } } })))
        .mount(&server)
        .await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let prs = github.search_prs("x", &[]).await.unwrap();
    let auto: Vec<(u64, bool)> = prs.iter().map(|p| (p.number, p.auto_merge)).collect();
    assert_eq!(auto, vec![(1, true), (2, false)]);
}

#[test]
fn inbox_entries_tell_what_i_reviewed_and_whether_it_changed_since() {
    let data = tempfile::tempdir().unwrap();
    let cache_path = data.path().join("cache.sqlite");
    let service = PrService::new(
        GitHubClient::new("http://127.0.0.1:9", "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&cache_path).unwrap(),
        Highlighter::new(),
    );
    let checkpoints = Cache::open(&cache_path).unwrap();
    let checkpoint = |id: &str, created_at: i64, source: &str, entries: &[(u64, &str)]| Checkpoint {
        id: id.into(),
        repo: "acme/shop".into(),
        created_at,
        source: source.into(),
        entries: entries.iter().map(|&(pr, head)| CheckpointEntry { pr, head: head.into(), base: "base".into() }).collect(),
    };
    // Marked #1 and #2 reviewed; #2 has been pushed to since. Submitting #3 isn't a mark.
    checkpoints.put_checkpoint(&checkpoint("c1", 1, "manual", &[(1, "sha1"), (2, "old2")])).unwrap();
    checkpoints.put_checkpoint(&checkpoint("c2", 2, "submit", &[(3, "sha3")])).unwrap();

    let review = |state: &str, commit: &str| Some(MyReview { state: state.into(), commit: commit.into(), submitted_at: "t".into() });
    let mut prs = vec![
        inbox_pr("acme/shop", 1, "main", "a", "1"),
        inbox_pr("acme/shop", 2, "a", "b", "1"),
        inbox_pr("acme/shop", 3, "b", "c", "1"),
    ];
    prs[0].my_review = review("APPROVED", "sha1");
    prs[2].my_review = review("COMMENTED", "old3");
    let groups = group(prs);

    let progress = |service: &PrService| -> Vec<(Option<String>, bool, bool, bool)> {
        service.inbox_entries(groups.clone()).unwrap()[0]
            .progress
            .iter()
            .map(|p| (p.review.clone(), p.changed_since_review, p.marked, p.changed_since_marked))
            .collect()
    };
    assert_eq!(
        progress(&service),
        vec![
            (Some("APPROVED".into()), false, true, false),
            (None, false, true, true),
            (Some("COMMENTED".into()), true, false, false),
        ]
    );

    // Marking #2 again at its current head: nothing new since.
    checkpoints.put_checkpoint(&checkpoint("c3", 3, "manual", &[(2, "sha2")])).unwrap();
    assert_eq!(progress(&service)[1], (None, false, true, false));
}

#[tokio::test]
async fn prefetching_makes_a_group_ready_and_finished_prs_are_evicted() {
    let origin = OriginRepo::init();
    origin.write("a.php", "<?php\n$a = 1;\n");
    origin.write("untouched.php", "<?php\n$b = 1;\n");
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("a.php", "<?php\n$a = 2;\n");
    let head = origin.commit("change");
    origin.publish_pr(4, "feature");
    let url = origin.url();

    let server = MockServer::start().await;
    let pr_json = support::pull_request_json(4, "main", "feature", &head, &url);
    Mock::given(path("/repos/acme/shop/pulls/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_json.clone()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/repos/acme/shop/pulls")).and(query_param("base", "feature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    let threads = serde_json::json!({ "data": { "repository": { "pullRequest": { "reviewThreads": { "pageInfo": { "hasNextPage": false }, "nodes": [] } } } } });
    Mock::given(path("/graphql")).and(body_string_contains("reviewThreads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(threads))
        .mount(&server)
        .await;
    let search = |nodes: serde_json::Value| ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": nodes } } }));
    let node = serde_json::json!({
        "number": 4, "title": "PR 4", "url": "u", "isDraft": false, "updatedAt": "2026-09-30T00:00:00Z",
        "headRefName": "feature", "baseRefName": "main", "headRefOid": head,
        "author": { "login": "me" }, "repository": { "nameWithOwner": "acme/shop" }
    });
    Mock::given(path("/graphql")).and(body_string_contains("review-requested")).respond_with(search(serde_json::json!([node]))).mount(&server).await;
    Mock::given(path("/graphql")).and(body_string_contains("author:@me")).respond_with(search(serde_json::json!([]))).mount(&server).await;

    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new(server.uri(), "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );

    let groups = service.refresh_inbox().await.unwrap();
    assert_eq!(service.cached_inbox().unwrap(), groups);
    let before = &service.inbox_entries(groups.clone()).unwrap()[0];
    assert!(!before.ready);
    assert_eq!(before.fetched_at, None);

    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    service.prefetch(&groups[0]).await.unwrap();
    let entry = &service.inbox_entries(groups.clone()).unwrap()[0];
    assert!(entry.ready, "openable offline now");
    assert!(entry.fetched_at.is_some_and(|t| t >= started), "remembers when it was fetched");
    let pr = wispy_core::pr_ref::PrRef::parse("acme/shop#4").unwrap();
    let snapshot = service.cached_stack(&pr).unwrap().unwrap();
    assert!(service.cached_range(&snapshot, 0, 0).unwrap().is_some());
    let git = wispy_core::git::Git::new(data.path().join("repos/acme/shop.git"), None);
    let listing = git.run_string(&["rev-list", "--objects", "--missing=print", "--no-walk", &snapshot.heads[0]]).unwrap();
    assert!(!listing.contains('?'), "every file of the head is downloaded, so search works offline");

    // Still listed: kept.
    assert_eq!(service.evict_finished(&groups).await.unwrap(), 0);
    // Gone from the inbox because it was merged: evicted.
    let mut merged = pr_json;
    merged["state"] = "closed".into();
    merged["merged_at"] = "2026-10-01T00:00:00Z".into();
    Mock::given(path("/repos/acme/shop/pulls/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(merged))
        .mount(&server)
        .await;
    assert_eq!(service.evict_finished(&[]).await.unwrap(), 1);
    assert!(service.cached_stack(&pr).unwrap().is_none());
    assert!(service.cached_range(&snapshot, 0, 0).unwrap().is_none());
}

#[tokio::test]
async fn a_stopped_service_skips_prefetching() {
    let origin = OriginRepo::init();
    origin.write("a.php", "<?php\n$a = 1;\n");
    origin.write("untouched.php", "<?php\n$b = 1;\n");
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("a.php", "<?php\n$a = 2;\n");
    let head = origin.commit("change");
    origin.publish_pr(4, "feature");
    let url = origin.url();

    let server = MockServer::start().await;
    let pr_json = support::pull_request_json(4, "main", "feature", &head, &url);
    Mock::given(path("/repos/acme/shop/pulls/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_json.clone()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/repos/acme/shop/pulls")).and(query_param("base", "feature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    let threads = serde_json::json!({ "data": { "repository": { "pullRequest": { "reviewThreads": { "pageInfo": { "hasNextPage": false }, "nodes": [] } } } } });
    Mock::given(path("/graphql")).and(body_string_contains("reviewThreads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(threads))
        .mount(&server)
        .await;
    let search = |nodes: serde_json::Value| ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": nodes } } }));
    let node = serde_json::json!({
        "number": 4, "title": "PR 4", "url": "u", "isDraft": false, "updatedAt": "2026-09-30T00:00:00Z",
        "headRefName": "feature", "baseRefName": "main", "headRefOid": head,
        "author": { "login": "me" }, "repository": { "nameWithOwner": "acme/shop" }
    });
    Mock::given(path("/graphql")).and(body_string_contains("review-requested")).respond_with(search(serde_json::json!([node]))).mount(&server).await;
    Mock::given(path("/graphql")).and(body_string_contains("author:@me")).respond_with(search(serde_json::json!([]))).mount(&server).await;

    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new(server.uri(), "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );

    let groups = service.refresh_inbox().await.unwrap();

    // The app is closing: what is asked for afterwards is skipped.
    service.stop();
    service.prefetch(&groups[0]).await.unwrap();
    let entry = &service.inbox_entries(groups.clone()).unwrap()[0];
    assert!(!entry.ready, "nothing was computed");
    assert_eq!(entry.fetched_at, None, "fetched again on the next launch");
}

/// #1 ← {#2, #3}, all in the inbox.
#[tokio::test]
async fn prefetching_a_tree_keeps_each_pr_on_the_branch_it_was_last_seen_on() {
    let origin = OriginRepo::init();
    origin.write("a.php", "<?php\n$a = 1;\n");
    origin.commit("base");
    origin.checkout_new("s/1");
    origin.write("a.php", "<?php\n$a = 2;\n");
    let head1 = origin.commit("one");
    origin.publish_pr(1, "s/1");
    origin.checkout_new("s/2");
    origin.write("b.php", "<?php\n$b = 1;\n");
    let head2 = origin.commit("two");
    origin.publish_pr(2, "s/2");
    origin.checkout("s/1");
    origin.checkout_new("s/3");
    origin.write("c.php", "<?php\n$c = 1;\n");
    let head3 = origin.commit("three");
    origin.publish_pr(3, "s/3");
    let url = origin.url();

    let server = MockServer::start().await;
    let pr1 = support::pull_request_json(1, "main", "s/1", &head1, &url);
    let pr2 = support::pull_request_json(2, "s/1", "s/2", &head2, &url);
    let pr3 = support::pull_request_json(3, "s/1", "s/3", &head3, &url);
    let respond = |body: serde_json::Value| ResponseTemplate::new(200).set_body_json(body);
    for (number, body) in [(1, &pr1), (2, &pr2), (3, &pr3)] {
        Mock::given(path(format!("/repos/acme/shop/pulls/{number}"))).respond_with(respond(body.clone())).mount(&server).await;
    }
    for (key, value, body) in [
        ("head", "acme:s/1", serde_json::json!([pr1])),
        ("base", "s/1", serde_json::json!([pr3, pr2])),
        ("base", "s/2", serde_json::json!([])),
        ("base", "s/3", serde_json::json!([])),
    ] {
        Mock::given(path("/repos/acme/shop/pulls")).and(query_param(key, value)).respond_with(respond(body)).mount(&server).await;
    }
    let threads = serde_json::json!({ "data": { "repository": { "pullRequest": { "reviewThreads": { "pageInfo": { "hasNextPage": false }, "nodes": [] } } } } });
    Mock::given(path("/graphql")).and(body_string_contains("reviewThreads")).respond_with(respond(threads)).mount(&server).await;

    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new(server.uri(), "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let groups = group(vec![
        inbox_pr("acme/shop", 1, "main", "s/1", "1"),
        inbox_pr("acme/shop", 2, "s/1", "s/2", "1"),
        inbox_pr("acme/shop", 3, "s/1", "s/3", "1"),
    ]);
    assert_eq!(groups.len(), 1);
    let pr = |n: u64| wispy_core::pr_ref::PrRef::parse(&format!("acme/shop#{n}")).unwrap();
    let cached = |n: u64| -> Vec<u64> { service.cached_stack(&pr(n)).unwrap().unwrap().stack.prs.iter().map(|p| p.number).collect() };

    let snapshots = service.prefetch(&groups[0]).await.unwrap();
    assert_eq!(snapshots.len(), 2, "one stack per branch");
    assert_eq!((cached(1), cached(2), cached(3)), (vec![1, 2], vec![1, 2], vec![1, 3]));
    let three = service.cached_stack(&pr(3)).unwrap().unwrap();
    assert_eq!(three.stack.branches(0).iter().map(|b| b.number).collect::<Vec<_>>(), vec![2, 3]);
    assert!(service.cached_range(&three, 1, 1).unwrap().is_some(), "every member is precomputed");

    // Reading #3 makes its branch the one #1 opens on, and prefetching keeps it so.
    service.fetch(service.discover(&pr(3)).await.unwrap()).unwrap();
    assert_eq!(cached(1), vec![1, 3]);
    assert_eq!(service.discover(&pr(1)).await.unwrap().prs.iter().map(|p| p.number).collect::<Vec<_>>(), vec![1, 3]);
    service.prefetch(&groups[0]).await.unwrap();
    assert_eq!((cached(1), cached(2), cached(3)), (vec![1, 3], vec![1, 2], vec![1, 3]));
}

#[tokio::test]
async fn pr_search_scopes_to_known_repos_and_keeps_the_order() {
    let server = MockServer::start().await;
    let node = |number: u64| {
        serde_json::json!({
            "number": number, "title": format!("Pickable units {number}"), "url": "u", "isDraft": false, "updatedAt": "2026-09-30T00:00:00Z",
            "headRefName": format!("h{number}"), "baseRefName": "main", "headRefOid": format!("sha{number}"),
            "author": { "login": "someone" }, "repository": { "nameWithOwner": "acme/shop" }, "viewerLatestReview": null
        })
    };
    // Only answers a search for open PRs with the words, in both repos.
    Mock::given(path("/graphql"))
        .and(body_string_contains("is:pr is:open"))
        .and(body_string_contains("pickable units"))
        .and(body_string_contains("repo:acme/shop"))
        .and(body_string_contains("repo:acme/app"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "search": { "nodes": [node(9), node(4)] } } })))
        .mount(&server)
        .await;

    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let prs = github.search_prs(" pickable units ", &["acme/shop".into(), "acme/app".into()]).await.unwrap();
    let found: Vec<(String, u64)> = prs.iter().map(|p| (p.repo.clone(), p.number)).collect();
    assert_eq!(found, vec![("acme/shop".into(), 9), ("acme/shop".into(), 4)]);
    assert_eq!(prs[0].title, "Pickable units 9");
}
