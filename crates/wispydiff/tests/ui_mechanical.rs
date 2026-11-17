//! Mechanical changes (renames, reformatting) fold away and open again.
mod ui_c_support;
use gpui_kit::TestAppContext;
use ui_c_support::*;
use wispydiff::diff::layout::Mode;
use wispydiff::diff::UnifiedRow;

const RENAMED: &str = "src/Mech/Renamed.php";
const MIXED: &str = "src/Mech/Mixed.php";

fn renamed_before() -> String {
    "<?php\nfunction total(array $items): int {\n    $sum = 0;\n    foreach ($items as $item) {\n        $sum += $item;\n    }\n    return $sum;\n}\n".into()
}

fn mixed_before() -> String {
    let filler: String = (0..20).map(|i| format!("function filler{i}(): int {{ return {i}; }}\n")).collect();
    format!("<?php\nfunction a() {{\n    return foo($first, $second, 'x');\n}}\n{filler}function b(): int {{\n    return 1;\n}}\n")
}

fn mixed_after() -> String {
    mixed_before()
        .replace("    return foo($first, $second, 'x');\n", "    return foo(\n        $first,\n        $second,\n        'x',\n    );\n")
        .replace("    return 1;\n", "    return 2;\n")
}

/// The solo PR (#5) with a file that only renames a variable and one that reformats a call and
/// changes a value.
fn fixture() -> Fixture {
    let f = Fixture::new();
    f.write(RENAMED, &renamed_before());
    f.write(MIXED, &mixed_before());
    let commit = ["-c", "user.name=Test", "-c", "user.email=test@example.test", "commit", "--quiet", "-m"];
    f.git(&["add", "-A"]);
    f.git(&[&commit[..], &["base files"]].concat());
    f.git(&["checkout", "--quiet", "solo"]);
    f.git(&["-c", "user.name=Test", "-c", "user.email=test@example.test", "rebase", "--quiet", "main"]);
    f.git(&["checkout", "--quiet", "main"]);
    f.push(RENAMED, &renamed_before().replace("$sum", "$total"));
    f.push(MIXED, &mixed_after());
    f
}

fn index(ui: &Ui, cx: &TestAppContext, path: &str) -> usize {
    ui.paths(cx).iter().position(|p| p == path).expect("file in the diff")
}

fn fold_rows(ui: &Ui, cx: &TestAppContext, file: usize) -> Vec<u32> {
    ui.diff(cx).read_with(cx, |d, _| {
        if d.segment(file).mode != Mode::Unified {
            return vec![];
        }
        (0..d.segment(file).rows).filter(|&o| matches!(d.unified_at(file, o), UnifiedRow::Fold { .. })).collect()
    })
}

#[gpui_kit::test]
async fn mechanical_changes_fold_open_on_click_and_m_shows_all(cx: &mut TestAppContext) {
    let f = fixture();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    ui.focus_diff(cx);
    let (renamed, mixed) = (index(&ui, cx, RENAMED), index(&ui, cx, MIXED));

    // A file with mechanical changes only starts collapsed; the other folds its reformatted call
    // and shows the changed value.
    ui.diff(cx).read_with(cx, |d, _| {
        assert_eq!(d.segment(renamed).mode, Mode::Collapsed);
        assert_eq!(d.view.summary.files[mixed].folds.len(), 1);
        assert_eq!(d.view.summary.files[mixed].folds[0].reason, "reformatted");
    });
    let folds = fold_rows(&ui, cx, mixed);
    assert_eq!(folds.len(), 1);
    assert_eq!(ui.changed_offsets(cx, mixed).len(), 2, "only `return 1` → `return 2` is shown");

    // A click on the fold shows its rows.
    ui.reveal_row(cx, mixed, folds[0]);
    let bounds = ui.row_bounds(cx, mixed, folds[0]).expect("fold row painted");
    ui.click(cx, bounds.center());
    assert!(fold_rows(&ui, cx, mixed).is_empty());
    assert_eq!(ui.changed_offsets(cx, mixed).len(), 2 + 6);

    // `m` shows every mechanical change, and folds them again (remembered as a preference).
    ui.keys(cx, "m");
    ui.diff(cx).read_with(cx, |d, _| assert_eq!(d.segment(renamed).mode, Mode::Unified));
    assert!(fold_rows(&ui, cx, mixed).is_empty());
    ui.keys(cx, "m");
    ui.diff(cx).read_with(cx, |d, _| assert_eq!(d.segment(renamed).mode, Mode::Collapsed));
    assert_eq!(fold_rows(&ui, cx, mixed).len(), 1);
    ui.close(cx).await;
}
