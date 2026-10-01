mod support;

use std::time::Duration;

use support::OriginRepo;
use wispy_core::git::Git;
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::model::{row_kind, DiffView};
use wispy_core::range::{compute_range, RangeSpec};
use wispy_core::repo_store::{FetchedPr, RepoStore};

fn origin_with_pr() -> (OriginRepo, String) {
    let origin = OriginRepo::init();
    origin.write("src/Stock.php", "<?php\nclass Stock\n{\n    public int $qty = 0;\n}\n");
    origin.write("README.md", "# Shop\n");
    origin.commit("base");
    origin.checkout_new("feature");
    origin.write("src/Stock.php", "<?php\nclass Stock\n{\n    public int $qty = 5;\n}\n");
    origin.write("src/Bin.ts", "export const bin = 'A-1';\n");
    let head = origin.commit("feature work");
    origin.publish_pr(7, "feature");
    origin.checkout("main");
    // main moves on after the branch point; the PR diff must not include this.
    origin.write("README.md", "# Shop\n\nMore docs.\n");
    origin.commit("docs on main");
    (origin, head)
}

fn single_pr_view(git: &Git, fetched: &FetchedPr) -> DiffView {
    let spec = RangeSpec { from: fetched.merge_base.clone(), heads: vec![fetched.head_sha.clone()], prs: vec![0], ignore_whitespace: false };
    compute_range(git, &spec, &Highlighter::new(), &HighlightCache::default()).unwrap()
}

fn missing_objects(git: &Git) -> usize {
    let out = git.run_string(&["rev-list", "--objects", "--all", "--missing=print"]).unwrap();
    out.lines().filter(|l| l.starts_with('?')).count()
}

#[test]
fn fetches_a_pr_into_a_blobless_clone() {
    let (origin, head) = origin_with_pr();
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);

    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    let fetched = store.fetch_pr(&git, 7, "main").unwrap();

    assert_eq!(git.git_dir(), root.path().join("acme/shop.git"));
    assert_eq!(fetched.head_sha, head);
    assert_eq!(fetched.base_sha, origin.git(&["rev-parse", "main"]));
    assert_eq!(fetched.merge_base, origin.git(&["merge-base", "main", "feature"]));
    assert!(missing_objects(&git) > 0, "blobs should not have been downloaded");
}

#[test]
fn ensure_repo_is_idempotent_and_refetch_picks_up_new_commits() {
    let (origin, _) = origin_with_pr();
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    store.fetch_pr(&git, 7, "main").unwrap();

    origin.checkout("feature");
    origin.write("src/Bin.ts", "export const bin = 'B-2';\n");
    let new_head = origin.commit("force-pushed fixup");
    origin.publish_pr(7, "feature");

    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    assert_eq!(store.fetch_pr(&git, 7, "main").unwrap().head_sha, new_head);
}

#[test]
fn computes_a_highlighted_three_dot_diff() {
    let (origin, _) = origin_with_pr();
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    let fetched = store.fetch_pr(&git, 7, "main").unwrap();

    let view = single_pr_view(&git, &fetched);

    let paths: Vec<_> = view.summary.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/Bin.ts", "src/Stock.php"], "README change on main must not appear");
    assert_eq!((view.summary.additions, view.summary.deletions), (2, 1));
    assert_eq!(view.summary.total_rows as usize, view.rows.len());

    let stock = &view.summary.files[1];
    let stock_rows = view.rows(stock.first_row, stock.first_row + stock.row_count);
    assert_eq!(stock_rows[0].k, row_kind::FILE);
    assert_eq!(stock_rows[1].k, row_kind::HUNK);
    assert_eq!(stock.hunks, vec![1]);
    assert_eq!(view.summary.files[0].hunks, vec![1]);

    let added = stock_rows.iter().find(|r| r.k == row_kind::ADDED).unwrap();
    assert_eq!(added.n, Some(4));
    let text: String = added.s.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(text, "    public int $qty = 5;");
    assert!(added.s.iter().any(|(class, _)| *class != 0), "PHP line should carry highlighting: {:?}", added.s);

    let deleted = stock_rows.iter().find(|r| r.k == row_kind::DELETED).unwrap();
    assert_eq!((deleted.o, deleted.n), (Some(4), None));

    // A single-PR range attributes every change to that PR and nothing else.
    assert!(view.rows.iter().all(|r| matches!(r.k, row_kind::ADDED | row_kind::DELETED) == (r.a == Some(0))));
    assert!(view.summary.files.iter().all(|f| f.prs == vec![0]));
}

#[test]
fn rows_window_is_clamped() {
    let (origin, _) = origin_with_pr();
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    let fetched = store.fetch_pr(&git, 7, "main").unwrap();
    let view = single_pr_view(&git, &fetched);

    let total = view.summary.total_rows;
    assert_eq!(view.rows(0, 10_000).len() as u32, total);
    assert!(view.rows(total + 5, total + 10).is_empty());
    assert!(view.rows(5, 2).is_empty());
}

#[test]
fn concurrent_setup_and_fetches_of_one_repo_do_not_collide() {
    let (origin, head) = origin_with_pr();
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let url = origin.url();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let git = store.ensure_repo("acme", "shop", &url).unwrap();
                    store.fetch_pr(&git, 7, "main").unwrap().head_sha
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), head);
        }
    });
}
#[test]
fn reads_of_a_set_up_repo_do_not_wait_for_a_running_fetch_but_fetches_do() {
    let (origin, head) = origin_with_pr();
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let url = origin.url();
    let git = store.ensure_repo("acme", "shop", &url).unwrap();
    store.fetch_pr(&git, 7, "main").unwrap();

    let lock = store.write_lock(&git);
    let (store, url, git) = (&store, &url, &git);
    std::thread::scope(|scope| {
        // Held inside the scope, so a failing assertion releases it before the threads are joined.
        let writing = lock.hold();
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        scope.spawn(move || read_tx.send(store.ensure_repo("acme", "shop", url).map(|g| g.git_dir().to_path_buf())));
        let (fetch_tx, fetch_rx) = std::sync::mpsc::channel();
        scope.spawn(move || fetch_tx.send(store.fetch_pr(git, 7, "main").map(|f| f.head_sha)));

        let read = read_rx.recv_timeout(Duration::from_secs(5)).expect("ensure_repo waited for the write lock");
        assert_eq!(read.unwrap(), root.path().join("acme/shop.git"));
        assert!(fetch_rx.recv_timeout(Duration::from_millis(300)).is_err(), "a fetch ran while another write held the lock");

        drop(writing);
        assert_eq!(fetch_rx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap(), head);
    });
}
