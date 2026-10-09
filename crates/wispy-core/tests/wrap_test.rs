use wispy_core::model::{row_kind, Row};
use wispy_core::split::SplitRow;
use wispy_core::wrap::{split_widths, text_width, unified_widths};

fn segs(text: &str) -> Vec<(u8, String)> {
    vec![(1, text[..text.len() / 2].to_string()), (0, text[text.len() / 2..].to_string())]
}

fn row(k: u8, text: &str) -> Row {
    Row { k, f: 0, o: None, n: None, s: segs(text), a: None, h: Vec::new(), l: None, w: Vec::new() }
}

fn split(old: &str, new: &str) -> SplitRow {
    SplitRow { o: None, n: None, ok: row_kind::CONTEXT, nk: row_kind::CONTEXT, os: segs(old), ns: segs(new), oa: None, na: None, ol: None, nl: None, ow: Vec::new(), nw: Vec::new() }
}

#[test]
fn measures_columns_with_tab_stops_and_wide_characters() {
    assert_eq!(text_width(&segs("abcd")), 4);
    assert_eq!(text_width(&segs("\tx")), 9);
    assert_eq!(text_width(&segs("ab\tx")), 9, "a tab goes to the next stop, not 8 further");
    assert_eq!(text_width(&segs("名前")), 4);
    assert_eq!(text_width(&[]), 0);
}

#[test]
fn lists_only_code_rows_wider_than_the_threshold() {
    let long = "x".repeat(30);
    let rows = vec![
        row(row_kind::FILE, &long),
        row(row_kind::HUNK, &long),
        row(row_kind::CONTEXT, "short"),
        row(row_kind::ADDED, &long),
        row(row_kind::DELETED, &"y".repeat(21)),
        row(row_kind::NOTICE, &long),
    ];
    assert_eq!(unified_widths(&rows, 20), vec![[3, 30], [4, 21]]);
    assert_eq!(unified_widths(&rows, 25), vec![[3, 30]]);
}

#[test]
fn a_side_by_side_row_is_as_wide_as_its_wider_half_and_starts_after_the_header() {
    let rows = vec![split("short", "short"), split(&"o".repeat(40), "n"), split("o", &"n".repeat(50))];
    assert_eq!(split_widths(&rows, 20), vec![[2, 40], [3, 50]]);
}
