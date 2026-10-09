mod support;

use support::{row_text, OriginRepo};
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::model::{file_slice, row_kind, DiffView};
use wispy_core::range::{compute_range, RangeSpec};
use wispy_core::repo_store::RepoStore;
use wispy_core::split::{align, split_rows, FILLER};

const C: u8 = row_kind::CONTEXT;
const A: u8 = row_kind::ADDED;
const D: u8 = row_kind::DELETED;
const F: u8 = FILLER;

/// The unified view of a single commit on top of `base`, computed through the real pipeline.
fn view_of(files_before: &[(&str, &str)], files_after: &[(&str, Option<&str>)]) -> DiffView {
    let origin = OriginRepo::init();
    for (path, content) in files_before {
        origin.write(path, content);
    }
    let base = origin.commit("base");
    for (path, content) in files_after {
        match content {
            Some(content) => {
                origin.write(path, content);
            }
            None => {
                origin.remove(path);
            }
        }
    }
    let head = origin.commit("change");
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    origin.publish_pr(1, "main");
    store.fetch_pr(&git, 1, "main").unwrap();
    let spec = RangeSpec { from: base, heads: vec![head], prs: vec![0], ignore_whitespace: false };
    compute_range(&git, &spec, &Highlighter::new(), &HighlightCache::default()).unwrap()
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn shape(view: &DiffView, old_len: u32, new_len: u32) -> Vec<(Option<u32>, Option<u32>, u8, u8)> {
    align(file_slice(view, 0), old_len, new_len).pairs.iter().map(|p| (p.o, p.n, p.ok, p.nk)).collect()
}

#[test]
fn aligns_a_whole_file_around_a_replacement_and_an_insertion() {
    let before = numbered(12);
    let after = before
        .replace("line 3\n", "line three\n")
        .replace("line 10\n", "line 10\nnew a\nnew b\n");
    let view = view_of(&[("a.txt", &before)], &[("a.txt", Some(&after))]);

    let pairs = shape(&view, 12, 14);
    assert_eq!(pairs.len(), 14);
    assert_eq!(pairs[0], (Some(1), Some(1), C, C));
    assert_eq!(pairs[2], (Some(3), Some(3), D, A), "replacement sits opposite what it replaced");
    assert_eq!(pairs[10], (None, Some(11), F, A));
    assert_eq!(pairs[11], (None, Some(12), F, A));
    assert_eq!(pairs[12], (Some(11), Some(13), C, C));
    assert_eq!(pairs[13], (Some(12), Some(14), C, C), "unchanged tail after the last hunk");

    let file = &view.summary.files[0];
    assert_eq!(file.split_rows, 15, "pairs + header");
    assert_eq!(file.split_blocks, vec![3, 11]);
}

#[test]
fn uneven_replacements_get_fillers_on_the_short_side() {
    let before = numbered(6);
    let after = before.replace("line 2\nline 3\nline 4\n", "only one\n");
    let view = view_of(&[("a.txt", &before)], &[("a.txt", Some(&after))]);
    let pairs = shape(&view, 6, 4);
    assert_eq!(
        pairs,
        vec![
            (Some(1), Some(1), C, C),
            (Some(2), Some(2), D, A),
            (Some(3), None, D, F),
            (Some(4), None, D, F),
            (Some(5), Some(3), C, C),
            (Some(6), Some(4), C, C),
        ]
    );
}

#[test]
fn added_and_deleted_files_have_one_empty_side() {
    let view = view_of(&[("keep.txt", "k\n"), ("gone.txt", "x\ny\n")], &[("gone.txt", None), ("new.txt", Some("a\nb\nc\n"))]);
    let gone = view.summary.files.iter().position(|f| f.path == "gone.txt").unwrap();
    let new = view.summary.files.iter().position(|f| f.path == "new.txt").unwrap();

    let gone_pairs = align(file_slice(&view, gone), 2, 0).pairs;
    assert!(gone_pairs.iter().all(|p| p.ok == D && p.nk == F && p.n.is_none()));
    assert_eq!(view.summary.files[gone].split_rows, 3);

    let new_pairs = align(file_slice(&view, new), 0, 3).pairs;
    assert!(new_pairs.iter().all(|p| p.ok == F && p.nk == A && p.o.is_none()));
    assert_eq!(view.summary.files[new].split_rows, 4);
}

#[test]
fn whitespace_only_changes_disappear_with_ignore_whitespace() {
    let origin = OriginRepo::init();
    origin.write("a.php", "<?php\nif ($a) {\n    return 1;\n}\n");
    origin.write("b.php", "<?php\n$x = 1;\n");
    let base = origin.commit("base");
    origin.write("a.php", "<?php\nif ($a) {\n        return 1;\n}\n");
    origin.write("b.php", "<?php\n$x = 2;\n");
    let head = origin.commit("reindent + change");
    origin.publish_pr(1, "main");
    let root = tempfile::tempdir().unwrap();
    let store = RepoStore::new(root.path(), None);
    let git = store.ensure_repo("acme", "shop", &origin.url()).unwrap();
    store.fetch_pr(&git, 1, "main").unwrap();

    let spec = RangeSpec { from: base, heads: vec![head], prs: vec![0], ignore_whitespace: false };
    let highlighter = Highlighter::new();
    let cache = HighlightCache::default();
    let all = compute_range(&git, &spec, &highlighter, &cache).unwrap();
    let no_ws = compute_range(&git, &spec.clone().with_ignore_whitespace(true), &highlighter, &cache).unwrap();

    assert_eq!(all.summary.files.len(), 2);
    let paths: Vec<_> = no_ws.summary.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["b.php"]);
    assert_ne!(spec.cache_key("acme/shop"), spec.clone().with_ignore_whitespace(true).cache_key("acme/shop"));
    assert!(no_ws.rows.iter().any(|r| row_text(r) == "$x = 2;"));
}

#[test]
fn replaced_lines_mark_the_words_that_changed_in_both_views() {
    let before = "a\nlet price = 1;\nlet name = 'x';\nz\n";
    let after = "a\nlet total = 1;\nlet name = 'y';\nz\n";
    let view = view_of(&[("a.txt", before)], &[("a.txt", Some(after))]);
    let rows = file_slice(&view, 0);
    let words = |kind: u8, text: &str| rows.iter().find(|r| r.k == kind && row_text(r) == text).unwrap().w.clone();
    assert_eq!(words(D, "let price = 1;"), vec![(4, 9)]);
    assert_eq!(words(A, "let total = 1;"), vec![(4, 9)]);
    assert_eq!(words(D, "let name = 'x';"), vec![(12, 13)], "paired in order within the block");
    assert_eq!(words(A, "let name = 'y';"), vec![(12, 13)]);
    assert!(words(C, "a").is_empty());

    let pairs = align(rows, 4, 4).pairs;
    let split = split_rows(rows, &pairs, &[], &[]);
    assert_eq!((split[1].ow.clone(), split[1].nw.clone()), (vec![(4, 9)], vec![(4, 9)]));
    assert!(split[0].ow.is_empty() && split[0].nw.is_empty());
}

#[test]
fn blocks_of_unequal_length_get_no_word_highlights() {
    let before = "a\nlet price = 1;\nz\n";
    let after = "a\nlet total = 1;\nlet extra = 2;\nz\n";
    let view = view_of(&[("a.txt", before)], &[("a.txt", Some(after))]);
    assert!(file_slice(&view, 0).iter().all(|r| r.w.is_empty()));
}

#[test]
fn a_restructured_block_marks_only_the_lines_that_were_edited() {
    let before = "x\n  resolveBody: (ctx) => ({\n    orderId: resolveOrderId(ctx),\n    warehouseId: options.warehouseId,\n    sendMail: false,\n  }),\ny\n";
    let after = "x\n  resolveBody: (ctx) => {\n    const { orderId, warehouseId } = resolveShipmentTarget(ctx);\n\n    return { orderId, warehouseId, sendMail: false };\n  },\ny\n";
    let view = view_of(&[("a.ts", before)], &[("a.ts", Some(after))]);
    let marked: Vec<(u8, String, Vec<(u32, u32)>)> =
        file_slice(&view, 0).iter().filter(|r| !r.w.is_empty()).map(|r| (r.k, row_text(r), r.w.clone())).collect();
    assert_eq!(
        marked,
        vec![(D, "  resolveBody: (ctx) => ({".into(), vec![(24, 25)]), (D, "  }),".into(), vec![(3, 4)])],
        "the brackets are what changed line for line; the rest was restructured"
    );
}
