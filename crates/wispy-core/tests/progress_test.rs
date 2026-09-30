mod support;

use support::{pull_request, OriginRepo};
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::progress::replayed_base;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;
use wispy_core::stack::Stack;

fn lines(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix} {i}\n")).collect()
}

/// main: a.txt, b.txt. PR #1 (feature) changes a.txt line 3.
fn origin() -> OriginRepo {
    let origin = OriginRepo::init();
    origin.write("a.txt", &lines("a", 10));
    origin.write("b.txt", &lines("b", 10));
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("a.txt", &lines("a", 10).replace("a 3\n", "a three\n"));
    origin.commit("pr");
    origin.publish_pr(1, "feature");
    origin
}

/// main gets a change to b.txt, the PR is rebased onto it, and (optionally) the author also
/// changes a.txt line 8.
fn rebase(origin: &OriginRepo, with_real_change: bool) {
    origin.checkout("main");
    origin.write("b.txt", &lines("b", 10).replace("b 5\n", "b five\n"));
    origin.commit("main moves");
    origin.checkout("feature");
    origin.git(&["rebase", "--quiet", "main"]);
    if with_real_change {
        origin.write("a.txt", &lines("a", 10).replace("a 3\n", "a three\n").replace("a 8\n", "a eight\n"));
        origin.commit("review fix");
    }
    origin.publish_pr(1, "feature");
}

fn service(data: &std::path::Path) -> PrService {
    PrService::new(
        GitHubClient::new("http://127.0.0.1:9", "t").unwrap(),
        RepoStore::new(data.join("repos"), None),
        Cache::open(&data.join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    )
}

fn stack(origin: &OriginRepo) -> Stack {
    Stack { prs: vec![pull_request(1, "main", "feature", &origin.url())], focus: 0 }
}

fn paths(view: &wispy_core::model::DiffView) -> Vec<String> {
    view.summary.files.iter().map(|f| f.path.clone()).collect()
}

#[test]
fn viewed_keys_survive_a_rebase_that_does_not_touch_the_file() {
    let origin = origin();
    let data = tempfile::tempdir().unwrap();
    let service = service(data.path());
    let before = service.fetch(stack(&origin)).unwrap();
    let key_before = service.range(&before, 0, 0).unwrap().summary.files[0].content_key.clone();
    service.set_viewed("acme/shop", &key_before, true).unwrap();

    rebase(&origin, false);
    let after = service.fetch(stack(&origin)).unwrap();
    assert_ne!(before.heads, after.heads, "rebased");
    let key_after = service.range(&after, 0, 0).unwrap().summary.files[0].content_key.clone();
    assert_eq!(key_after, key_before);
    assert!(service.viewed("acme/shop").unwrap().contains(&key_after));

    rebase(&origin, true);
    let changed = service.fetch(stack(&origin)).unwrap();
    assert_ne!(service.range(&changed, 0, 0).unwrap().summary.files[0].content_key, key_before, "real change: not viewed");

    service.set_viewed("acme/shop", &key_before, false).unwrap();
    assert!(service.viewed("acme/shop").unwrap().is_empty());
}

#[test]
fn since_checkpoint_shows_only_what_the_author_changed_not_the_rebase() {
    let origin = origin();
    let data = tempfile::tempdir().unwrap();
    let service = service(data.path());
    let reviewed = service.fetch(stack(&origin)).unwrap();
    let checkpoint = service.mark_reviewed(&reviewed, 0, 0, "manual").unwrap();
    assert_eq!(checkpoint.entries[0].head, reviewed.heads[0]);

    rebase(&origin, true);
    let now = service.fetch(stack(&origin)).unwrap();
    assert_eq!(service.checkpoints(&now, 0, 0).unwrap(), vec![checkpoint.clone()]);

    let (interdiff, conflicts) = service.interdiff(&now, 0, 0, &checkpoint).unwrap();
    assert!(!conflicts);
    assert_eq!(paths(&interdiff), vec!["a.txt"], "b.txt came from main via the rebase");
    assert_eq!((interdiff.summary.additions, interdiff.summary.deletions), (1, 1), "only line 8");

    // A plain head-to-head diff would have shown the rebase too.
    let git = wispy_core::git::Git::new(data.path().join("repos/acme/shop.git"), None);
    let raw = git.run_string(&["diff", "--name-only", &reviewed.heads[0], &now.heads[0]]).unwrap();
    assert_eq!(raw.lines().collect::<Vec<_>>(), vec!["a.txt", "b.txt"]);
}

#[test]
fn replaying_onto_a_conflicting_base_falls_back_to_a_raw_diff() {
    let origin = origin();
    let data = tempfile::tempdir().unwrap();
    let service = service(data.path());
    let reviewed = service.fetch(stack(&origin)).unwrap();

    // main rewrites the very line the PR changed; the author resolves the conflict.
    origin.checkout("main");
    origin.write("a.txt", &lines("a", 10).replace("a 3\n", "a III\n"));
    origin.commit("main conflicts");
    origin.checkout("feature");
    origin.git(&["reset", "--quiet", "--hard", "main"]);
    origin.write("a.txt", &lines("a", 10).replace("a 3\n", "a three (resolved)\n"));
    origin.commit("pr, rebased by hand");
    origin.publish_pr(1, "feature");
    let now = service.fetch(stack(&origin)).unwrap();

    let git = wispy_core::git::Git::new(data.path().join("repos/acme/shop.git"), None);
    let (from, conflicts) = replayed_base(&git, &reviewed.bases[0], &reviewed.heads[0], &now.bases[0]).unwrap();
    assert!(conflicts);
    assert_eq!(from, reviewed.heads[0]);
}

#[test]
fn checkpoints_must_cover_both_ends_of_the_range() {
    let origin = OriginRepo::init();
    origin.write("a.txt", "a\n");
    origin.commit("base");
    for (n, branch) in [(1, "s/1"), (2, "s/2")] {
        origin.checkout_new(branch);
        origin.write(&format!("f{n}.txt"), "x\n");
        origin.commit(branch);
        origin.publish_pr(n, branch);
    }
    let url = origin.url();
    let data = tempfile::tempdir().unwrap();
    let service = service(data.path());
    let snapshot = service
        .fetch(Stack { prs: vec![pull_request(1, "main", "s/1", &url), pull_request(2, "s/1", "s/2", &url)], focus: 0 })
        .unwrap();
    service.mark_reviewed(&snapshot, 1, 1, "submit").unwrap();
    let both = service.mark_reviewed(&snapshot, 0, 1, "manual").unwrap();

    assert_eq!(service.checkpoints(&snapshot, 0, 1).unwrap(), vec![both.clone()]);
    assert_eq!(service.checkpoints(&snapshot, 1, 1).unwrap().len(), 2);
    assert_eq!(service.checkpoints(&snapshot, 0, 0).unwrap(), vec![both]);
}
