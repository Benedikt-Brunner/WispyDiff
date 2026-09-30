mod support;

use support::{pull_request, OriginRepo};
use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wispy_core::anchors::{quoted_body, Coverage};
use wispy_core::cache::Cache;
use wispy_core::drafts::{new_id, Draft, DraftKind, DraftStatus};
use wispy_core::git::Git;
use wispy_core::github::{GitHubClient, Side, Verdict};
use wispy_core::pr_ref::PrRef;
use wispy_core::repo_store::RepoStore;
use wispy_core::review::{payload, plan, reconcile, send, Plan, Target};

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// main: a.txt with 30 lines. PR (stack/1): line 10 → "ten", line 25 → "twenty-five".
struct Fixture {
    origin: OriginRepo,
    git: Git,
    store: RepoStore,
    base: String,
    head: String,
    _root: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let origin = OriginRepo::init();
    origin.write("a.txt", &lines(30));
    let base = origin.commit("base");
    origin.checkout_new("stack/1");
    origin.write("a.txt", &lines(30).replace("line 10\n", "ten\n").replace("line 25\n", "twenty-five\n"));
    let head = origin.commit("pr");
    origin.publish_pr(1, "stack/1");
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    store.fetch_stack(&git, &[pull_request(1, "main", "stack/1", &origin.url())]).unwrap();
    Fixture { origin, git, store, base, head, _root: root }
}

fn line_draft(fx: &Fixture, line: u32, start: Option<u32>, body: &str) -> Draft {
    Draft {
        id: new_id(),
        repo: "acme/shop".into(),
        pr: 1,
        kind: DraftKind::Line,
        path: Some("a.txt".into()),
        side: Some(Side::Right),
        line: Some(line),
        start_line: start,
        commit: fx.head.clone(),
        base: fx.base.clone(),
        body: body.into(),
        thread_id: None,
        reply_to: None,
        as_file: false,
        status: DraftStatus::Draft,
        error: None,
        token: None,
        created_at: 1,
        updated_at: 1,
    }
}

fn pr() -> PrRef {
    PrRef::parse("acme/shop#1").unwrap()
}

#[test]
fn coverage_accepts_lines_inside_hunks_only() {
    let fx = fixture();
    let coverage = Coverage::compute(&fx.git, &fx.base, &fx.head).unwrap();
    assert!(coverage.accepts("a.txt", Side::Right, 10, 10));
    assert!(coverage.accepts("a.txt", Side::Right, 7, 13), "context lines count");
    assert!(!coverage.accepts("a.txt", Side::Right, 17, 17), "between hunks");
    assert!(!coverage.accepts("a.txt", Side::Right, 12, 23), "spans two hunks");
    assert!(coverage.accepts("a.txt", Side::Left, 10, 10));
    assert!(!coverage.accepts("b.txt", Side::Right, 1, 1));
}

#[test]
fn plans_line_file_and_outdated_drafts_against_the_current_head() {
    let fx = fixture();
    let on_change = line_draft(&fx, 10, None, "why ten?");
    let outside = line_draft(&fx, 17, None, "this should change too");
    let on_twenty_five = line_draft(&fx, 25, Some(24), "range");
    let pr = pr();

    // The author pushes: a line inserted at the top shifts everything; line 25 is rewritten.
    fx.origin.checkout("stack/1");
    fx.origin.write("a.txt", &format!("header\n{}", lines(30).replace("line 10\n", "ten\n").replace("line 25\n", "25!\n")));
    let new_head = fx.origin.commit("fixup");
    fx.origin.publish_pr(1, "stack/1");
    fx.store.fetch_stack(&fx.git, &[pull_request(1, "main", "stack/1", &fx.origin.url())]).unwrap();

    let coverage = Coverage::compute(&fx.git, &fx.base, &new_head).unwrap();
    let target = Target { pr: &pr, head: &new_head, base: &fx.base, coverage: &coverage };
    let mut as_file = on_twenty_five.clone();
    as_file.as_file = true;
    let planned = plan(&fx.git, &target, &[on_change, outside, on_twenty_five, as_file]).unwrap();
    let plans: Vec<&Plan> = planned.iter().map(|p| &p.plan).collect();

    assert_eq!(plans[0], &Plan::Line { path: "a.txt".into(), side: Side::Right, line: 11, start_line: None }, "moved down one");
    assert_eq!(plans[1], &Plan::File { path: "a.txt".into(), quoted: true }, "outside the diff");
    assert_eq!(plans[2], &Plan::Outdated, "the commented line was rewritten");
    assert_eq!(plans[3], &Plan::File { path: "a.txt".into(), quoted: true }, "user chose file comment");
}

#[test]
fn quotes_the_commented_lines_with_a_permalink() {
    let fx = fixture();
    let pr = pr();
    let coverage = Coverage::compute(&fx.git, &fx.base, &fx.head).unwrap();
    let target = Target { pr: &pr, head: &fx.head, base: &fx.base, coverage: &coverage };
    let planned = plan(&fx.git, &target, &[line_draft(&fx, 17, Some(16), "also rename these")]).unwrap();
    let payload = payload(&fx.git, &target, &planned, None).unwrap();

    let (_, path, body) = &payload.file_comments[0];
    assert_eq!(path, "a.txt");
    let expected = quoted_body("acme/shop", &fx.head, "a.txt", 16, 17, &["line 16".into(), "line 17".into()], "also rename these");
    assert_eq!(body, &expected);
    assert!(body.starts_with(&format!("[L16–17](https://github.com/acme/shop/blob/{}/a.txt#L16-L17)", fx.head)));
}

async fn service_fixture() -> (Fixture, MockServer, GitHubClient, Cache) {
    let fx = fixture();
    let server = MockServer::start().await;
    let github = GitHubClient::new(server.uri(), "t").unwrap();
    (fx, server, github, Cache::open_in_memory().unwrap())
}

#[tokio::test]
async fn posts_one_review_plus_file_comments_replies_and_resolves() {
    let (fx, server, github, cache) = service_fixture().await;
    let line = line_draft(&fx, 10, None, "why ten?");
    let outside = line_draft(&fx, 17, None, "and this");
    let mut reply = line_draft(&fx, 1, None, "fixed, thanks");
    reply.kind = DraftKind::Reply;
    reply.reply_to = Some(4242);
    let mut resolve = line_draft(&fx, 1, None, "");
    resolve.kind = DraftKind::Resolve;
    resolve.thread_id = Some("PRRT_1".into());
    for d in [&line, &outside, &reply, &resolve] {
        cache.put_draft(d).unwrap();
    }

    Mock::given(method("POST"))
        .and(path("/repos/acme/shop/pulls/1/reviews"))
        .and(body_partial_json(serde_json::json!({
            "commit_id": fx.head, "event": "APPROVE",
            "comments": [{ "path": "a.txt", "line": 10, "side": "RIGHT", "body": "why ten?" }]
        })))
        .and(body_string_contains("Looks good"))
        .and(body_string_contains("<!-- wispydiff:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/shop/pulls/1/comments"))
        .and(body_partial_json(serde_json::json!({ "path": "a.txt", "subject_type": "file" })))
        .and(body_string_contains("and this"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({ "id": 2 })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/shop/pulls/1/comments/4242/replies"))
        .and(body_string_contains("fixed, thanks"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({ "id": 3 })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("resolveReviewThread"))
        .and(body_string_contains("PRRT_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "resolveReviewThread": { "thread": { "id": "PRRT_1", "isResolved": true } } } })))
        .expect(1)
        .mount(&server)
        .await;

    let pr = pr();
    let coverage = Coverage::compute(&fx.git, &fx.base, &fx.head).unwrap();
    let target = Target { pr: &pr, head: &fx.head, base: &fx.base, coverage: &coverage };
    let drafts = cache.drafts("acme/shop", &[1]).unwrap();
    let planned = plan(&fx.git, &target, &drafts).unwrap();
    let payload = payload(&fx.git, &target, &planned, Some("Looks good")).unwrap();
    let outcome = send(&github, &cache, &target, payload, Verdict::Approve).await.unwrap();

    assert_eq!((outcome.posted, outcome.failed.len(), outcome.unknown), (4, 0, 0));
    assert!(cache.drafts("acme/shop", &[1]).unwrap().iter().all(|d| d.status == DraftStatus::Posted));
}

#[tokio::test]
async fn keeps_rejected_drafts_locally_with_the_error() {
    let (fx, server, github, cache) = service_fixture().await;
    let line = line_draft(&fx, 10, None, "why ten?");
    cache.put_draft(&line).unwrap();
    Mock::given(path("/repos/acme/shop/pulls/1/reviews"))
        .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({ "message": "Validation Failed" })))
        .mount(&server)
        .await;

    let pr = pr();
    let coverage = Coverage::compute(&fx.git, &fx.base, &fx.head).unwrap();
    let target = Target { pr: &pr, head: &fx.head, base: &fx.base, coverage: &coverage };
    let planned = plan(&fx.git, &target, &cache.drafts("acme/shop", &[1]).unwrap()).unwrap();
    let outcome = send(&github, &cache, &target, payload(&fx.git, &target, &planned, None).unwrap(), Verdict::Comment).await.unwrap();

    assert_eq!(outcome.failed.len(), 1);
    let stored = cache.draft(&line.id).unwrap().unwrap();
    assert_eq!(stored.status, DraftStatus::Failed);
    assert!(stored.error.unwrap().contains("Validation Failed"));
    assert_eq!(stored.body, "why ten?");
}

#[tokio::test]
async fn an_interrupted_submit_is_reconciled_instead_of_posted_twice() {
    let (fx, server, github, cache) = service_fixture().await;
    let posted = line_draft(&fx, 10, None, "made it");
    let lost = line_draft(&fx, 25, None, "never arrived");
    cache.put_draft(&posted).unwrap();
    cache.put_draft(&lost).unwrap();
    let pr = pr();
    let coverage = Coverage::compute(&fx.git, &fx.base, &fx.head).unwrap();
    let target = Target { pr: &pr, head: &fx.head, base: &fx.base, coverage: &coverage };

    // The connection drops mid-request: GitHub may or may not have created the review.
    let unreachable = GitHubClient::new("http://127.0.0.1:9", "t").unwrap();
    let planned = plan(&fx.git, &target, &[posted.clone()]).unwrap();
    let outcome = send(&unreachable, &cache, &target, payload(&fx.git, &target, &planned, None).unwrap(), Verdict::Comment).await.unwrap();
    assert_eq!(outcome.unknown, 1);
    let posting = cache.draft(&posted.id).unwrap().unwrap();
    assert_eq!(posting.status, DraftStatus::Posting);
    let token = posting.token.clone().unwrap();

    // It did arrive: GitHub has a review carrying the draft's marker.
    Mock::given(path("/repos/acme/shop/pulls/1/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{ "id": 1, "body": format!("\n\n<!-- wispydiff:{token} -->") }])))
        .mount(&server)
        .await;
    Mock::given(path("/repos/acme/shop/pulls/1/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    let mut interrupted_lost = lost.clone();
    interrupted_lost.status = DraftStatus::Posting;
    interrupted_lost.token = Some("gone".into());
    cache.put_draft(&interrupted_lost).unwrap();

    reconcile(&github, &cache, &pr, &cache.drafts("acme/shop", &[1]).unwrap()).await.unwrap();
    assert_eq!(cache.draft(&posted.id).unwrap().unwrap().status, DraftStatus::Posted);
    assert_eq!(cache.draft(&lost.id).unwrap().unwrap().status, DraftStatus::Draft, "will be sent again");
}

#[tokio::test]
async fn fetches_review_threads_over_graphql() {
    let server = MockServer::start().await;
    Mock::given(path("/graphql"))
        .and(body_string_contains("reviewThreads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [{
                "id": "PRRT_1", "isResolved": false, "isOutdated": false, "path": "a.txt", "line": 10, "startLine": null,
                "originalLine": 10, "diffSide": "RIGHT", "subjectType": "LINE",
                "comments": { "nodes": [{ "id": "C1", "databaseId": 4242, "body": "why?", "createdAt": "2026-09-30T10:00:00Z", "url": "u", "author": { "login": "octocat" } }] }
            }]
        } } } } })))
        .mount(&server)
        .await;
    let github = GitHubClient::new(server.uri(), "t").unwrap();
    let threads = github.review_threads(&pr()).await.unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!((threads[0].line, threads[0].side, threads[0].file_level), (Some(10), Side::Right, false));
    assert_eq!(threads[0].comments[0].database_id, 4242);
    assert_eq!(threads[0].comments[0].author, "octocat");
}
