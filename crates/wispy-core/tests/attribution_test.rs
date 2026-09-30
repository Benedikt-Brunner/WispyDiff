mod support;

use support::{pull_request, row_text, OriginRepo};
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::model::{row_kind, DiffView, Row};
use wispy_core::range::compute_range;
use wispy_core::repo_store::RepoStore;
use wispy_core::stack::{Stack, StackSnapshot};

fn base_lines() -> Vec<String> {
    (1..=20).map(|i| format!("line {i}")).collect()
}

fn write_lines(origin: &OriginRepo, path: &str, lines: &[String]) {
    origin.write(path, &(lines.join("\n") + "\n"));
}

/// main: lib.php (20 lines)
/// #1 (stack/1): line 2 → "two by pr1", adds "added by pr1" after line 20
/// #2 (stack/2): "two by pr1" → "two by pr2", deletes line 4, adds "added by pr2" at the end
/// #3 (stack/3): renames lib.php → src/lib.php, line 1 → "one by pr3", adds new.ts
fn three_pr_stack() -> (OriginRepo, StackSnapshot, tempfile::TempDir) {
    let origin = OriginRepo::init();
    let mut lines = base_lines();
    write_lines(&origin, "lib.php", &lines);
    origin.commit("base");

    origin.checkout_new("stack/1");
    lines[1] = "two by pr1".into();
    lines.push("added by pr1".into());
    write_lines(&origin, "lib.php", &lines);
    origin.commit("pr1");
    origin.publish_pr(1, "stack/1");

    origin.checkout_new("stack/2");
    lines[1] = "two by pr2".into();
    lines.remove(3);
    lines.push("added by pr2".into());
    write_lines(&origin, "lib.php", &lines);
    origin.commit("pr2");
    origin.publish_pr(2, "stack/2");

    origin.checkout_new("stack/3");
    origin.remove("lib.php");
    lines[0] = "one by pr3".into();
    write_lines(&origin, "src/lib.php", &lines);
    origin.write("new.ts", "export const a = 1;\nexport const b = 2;\n");
    origin.commit("pr3");
    origin.publish_pr(3, "stack/3");

    let url = origin.url();
    let stack = Stack {
        prs: vec![
            pull_request(1, "main", "stack/1", &url),
            pull_request(2, "stack/1", "stack/2", &url),
            pull_request(3, "stack/2", "stack/3", &url),
        ],
        focus: 0,
    };
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &url).unwrap();
    let points = store.fetch_stack(&git, &stack.prs).unwrap();
    (origin, StackSnapshot::new(stack, points), root)
}

fn view(snapshot: &StackSnapshot, root: &tempfile::TempDir, lo: usize, hi: usize) -> DiffView {
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &snapshot.stack.prs[0].clone_url).unwrap();
    compute_range(&git, &snapshot.range(lo, hi), &Highlighter::new(), &HighlightCache::default()).unwrap()
}

fn find<'a>(view: &'a DiffView, kind: u8, text: &str) -> &'a Row {
    view.rows
        .iter()
        .find(|r| r.k == kind && row_text(r) == text)
        .unwrap_or_else(|| panic!("no row {kind} {text:?}"))
}

#[test]
fn attributes_each_line_to_the_last_pr_that_touched_it() {
    let (_origin, snapshot, root) = three_pr_stack();
    let view = view(&snapshot, &root, 0, 2);

    let paths: Vec<_> = view.summary.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["new.ts", "src/lib.php"]);
    assert_eq!(view.summary.files[1].old_path.as_deref(), Some("lib.php"), "rename detected");

    let attr = |kind, text| {
        let row = find(&view, kind, text);
        (row.a, row.h.clone())
    };
    assert_eq!(attr(row_kind::ADDED, "added by pr1"), (Some(0), vec![]));
    assert_eq!(attr(row_kind::ADDED, "two by pr2"), (Some(1), vec![0, 1]), "pr2 modified pr1's line");
    assert_eq!(attr(row_kind::ADDED, "added by pr2"), (Some(1), vec![]));
    assert_eq!(attr(row_kind::ADDED, "one by pr3"), (Some(2), vec![]));
    assert_eq!(attr(row_kind::ADDED, "export const a = 1;"), (Some(2), vec![]));
    assert_eq!(attr(row_kind::DELETED, "line 1"), (Some(2), vec![]));
    assert_eq!(attr(row_kind::DELETED, "line 2"), (Some(0), vec![]), "pr1 replaced base line 2");
    assert_eq!(attr(row_kind::DELETED, "line 4"), (Some(1), vec![]));
    assert!(view.rows.iter().filter(|r| r.k == row_kind::CONTEXT).all(|r| r.a.is_none()));

    assert_eq!(view.summary.files[0].prs, vec![2]);
    assert_eq!(view.summary.files[1].prs, vec![0, 1, 2]);
}

#[test]
fn a_range_in_the_middle_starts_from_the_pr_below() {
    let (_origin, snapshot, root) = three_pr_stack();
    let view = view(&snapshot, &root, 1, 1);

    assert_eq!(view.summary.base_sha, snapshot.heads[0]);
    assert_eq!(view.summary.head_sha, snapshot.heads[1]);
    assert_eq!((view.summary.additions, view.summary.deletions), (2, 2));
    assert_eq!(find(&view, row_kind::DELETED, "two by pr1").a, Some(1));
    assert!(view.rows.iter().all(|r| r.k != row_kind::ADDED || r.a == Some(1)));
}

#[test]
fn flags_prs_not_based_on_the_latest_head_below() {
    let (origin, snapshot, _root) = three_pr_stack();
    assert_eq!(snapshot.needs_rebase(), vec![false, false, false]);

    // #1 gets a new commit; #2 and #3 are not rebased onto it.
    origin.checkout("stack/1");
    origin.write("late.txt", "late\n");
    origin.commit("late fix in pr1");
    origin.publish_pr(1, "stack/1");

    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    let points = store.fetch_stack(&git, &snapshot.stack.prs).unwrap();
    let refreshed = StackSnapshot::new(snapshot.stack.clone(), points);

    assert_eq!(refreshed.needs_rebase(), vec![false, true, false]);
}

#[test]
fn ranges_are_ordered_focus_first_then_whole_stack() {
    let (_origin, mut snapshot, _root) = three_pr_stack();
    snapshot.stack.focus = 1;
    assert_eq!(snapshot.ranges_by_priority(), vec![(1, 1), (0, 2), (0, 0), (2, 2), (0, 1), (1, 2)]);
}
