mod support;

use support::{row_text, OriginRepo};
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::model::{file_slice, DiffView};
use wispy_core::range::{compute_range, RangeSpec};
use wispy_core::repo_store::RepoStore;

/// The view of one commit changing `before` into `after`, through the real pipeline.
fn view_of(before: &[(&str, &str)], after: &[(&str, &str)]) -> DiffView {
    let origin = OriginRepo::init();
    for (path, content) in before {
        origin.write(path, content);
    }
    let base = origin.commit("base");
    for (path, content) in after {
        origin.write(path, content);
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

fn file<'a>(view: &'a DiffView, path: &str) -> (usize, &'a wispy_core::model::FileSummary) {
    view.summary.files.iter().enumerate().find(|(_, f)| f.path == path).expect("file in the diff")
}

/// Each fold's reason and the changed lines it covers (`-old` / `+new`).
fn folds(view: &DiffView, path: &str) -> Vec<(String, Vec<String>)> {
    let (index, summary) = file(view, path);
    let rows = file_slice(view, index);
    summary
        .folds
        .iter()
        .map(|fold| {
            let lines = rows[fold.offset as usize..(fold.offset + fold.len) as usize]
                .iter()
                .map(|r| format!("{}{}", if r.o.is_some() && r.n.is_none() { "-" } else if r.n.is_some() && r.o.is_none() { "+" } else { " " }, row_text(r).trim()))
                .collect();
            (fold.reason.clone(), lines)
        })
        .collect()
}

#[test]
fn reflowed_arguments_and_a_trailing_comma_are_reformatting() {
    let before = "<?php\nfunction a() {\n    return foo($first, $second, 'x');\n}\n";
    let after = "<?php\nfunction a() {\n    return foo(\n        $first,\n        $second,\n        \"x\",\n    );\n}\n";
    let view = view_of(&[("src/a.php", before)], &[("src/a.php", after)]);

    let found = folds(&view, "src/a.php");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "reformatted");
    assert!(file(&view, "src/a.php").1.mechanical);
}

#[test]
fn a_changed_value_inside_reformatted_code_stays_visible() {
    let before = "<?php\nfunction a() {\n    return foo($first, $second, 'x');\n}\n";
    let after = "<?php\nfunction a() {\n    return foo(\n        $first,\n        $second,\n        'y',\n    );\n}\n";
    let view = view_of(&[("src/a.php", before)], &[("src/a.php", after)]);

    assert!(folds(&view, "src/a.php").is_empty());
    assert!(!file(&view, "src/a.php").1.mechanical);
}

#[test]
fn a_local_variable_renamed_at_every_use_is_mechanical() {
    let before = "<?php\nfunction total(array $items) {\n    $sum = 0;\n    foreach ($items as $item) {\n        $sum += $item;\n    }\n    return $sum;\n}\n";
    let after = before.replace("$sum", "$total");
    let view = view_of(&[("src/total.php", before)], &[("src/total.php", &after)]);

    let found = folds(&view, "src/total.php");
    assert!(!found.is_empty());
    assert!(found.iter().all(|(reason, _)| reason == "renamed `$sum` → `$total`"), "{found:?}");
    assert!(file(&view, "src/total.php").1.mechanical);
}

#[test]
fn renaming_a_variable_onto_a_name_already_in_use_is_not_mechanical() {
    let before = "<?php\nfunction f($a, $b) {\n    $x = $a;\n    return $x + $b;\n}\n";
    // `$x` → `$b` changes what the function computes.
    let after = before.replace("$x", "$b");
    let view = view_of(&[("src/f.php", before)], &[("src/f.php", &after)]);

    assert!(folds(&view, "src/f.php").is_empty());
}

#[test]
fn renaming_only_some_uses_of_a_variable_is_not_mechanical() {
    let before = "function f(items) {\n  const count = items.length;\n  log(count);\n\n\n\n\n\n\n  return count;\n}\n";
    let after = before.replacen("count", "size", 2);
    let view = view_of(&[("src/f.ts", before)], &[("src/f.ts", &after)]);

    assert!(folds(&view, "src/f.ts").is_empty(), "{:?}", folds(&view, "src/f.ts"));
}

#[test]
fn an_object_shorthand_rename_changes_a_key_and_stays_visible() {
    let before = "function f() {\n  const id = 1;\n  return { id };\n}\n";
    let after = before.replace("id", "key");
    let view = view_of(&[("src/f.ts", before)], &[("src/f.ts", &after)]);

    let found = folds(&view, "src/f.ts");
    assert!(found.iter().all(|(_, lines)| !lines.iter().any(|l| l.contains("return"))), "{found:?}");
}

#[test]
fn a_method_renamed_across_every_touched_file_is_mechanical() {
    let class_before = "<?php\nclass Cart {\n    public function sumUp(): int {\n        return 1;\n    }\n}\n";
    let caller_before = "<?php\nfunction checkout(Cart $cart) {\n    return $cart->sumUp();\n}\n";
    let view = view_of(
        &[("src/Cart.php", class_before), ("src/checkout.php", caller_before)],
        &[("src/Cart.php", &class_before.replace("sumUp", "total")), ("src/checkout.php", &caller_before.replace("sumUp", "total"))],
    );

    for path in ["src/Cart.php", "src/checkout.php"] {
        let found = folds(&view, path);
        assert_eq!(found.len(), 1, "{path}: {found:?}");
        assert_eq!(found[0].0, "renamed `sumUp` → `total`");
    }
}

#[test]
fn a_method_rename_that_leaves_a_use_behind_is_not_mechanical() {
    let class_before = "<?php\nclass Cart {\n    public function sumUp(): int {\n        return 1;\n    }\n}\n";
    let caller_before = "<?php\nfunction checkout(Cart $cart) {\n    return $cart->sumUp();\n}\n\nfunction other(Cart $cart) {\n    $x = 1;\n    return $cart->sumUp();\n}\n";
    let caller_after = caller_before.replacen("sumUp", "total", 1).replace("$x = 1", "$x = 2");
    let view = view_of(
        &[("src/Cart.php", class_before), ("src/checkout.php", caller_before)],
        &[("src/Cart.php", &class_before.replace("sumUp", "total")), ("src/checkout.php", &caller_after)],
    );

    assert!(folds(&view, "src/Cart.php").is_empty());
    assert!(folds(&view, "src/checkout.php").is_empty());
}

#[test]
fn comments_may_follow_a_rename_but_not_change_otherwise() {
    let before = "/** Adds up the items. */\nfunction f(items: number[]) {\n  // the running total\n  let acc = 0;\n  for (const i of items) acc += i;\n  return acc;\n}\n";
    let renamed = before.replace("acc", "sum");
    let view = view_of(&[("src/f.ts", before)], &[("src/f.ts", &renamed)]);
    assert!(file(&view, "src/f.ts").1.mechanical, "{:?}", folds(&view, "src/f.ts"));

    let reworded = before.replace("running total", "grand total");
    let view = view_of(&[("src/f.ts", before)], &[("src/f.ts", &reworded)]);
    assert!(folds(&view, "src/f.ts").is_empty());
}

#[test]
fn whitespace_inside_strings_is_not_formatting() {
    let before = "const a = 'x y';\n";
    let after = "const a = 'x  y';\n";
    let view = view_of(&[("src/a.ts", before)], &[("src/a.ts", after)]);
    assert!(folds(&view, "src/a.ts").is_empty());
}

#[test]
fn yaml_indentation_is_never_formatting() {
    let before = "a:\n  b: 1\n";
    let after = "a:\nb: 1\n";
    let view = view_of(&[("conf.yaml", before)], &[("conf.yaml", after)]);
    assert!(folds(&view, "conf.yaml").is_empty());
}
