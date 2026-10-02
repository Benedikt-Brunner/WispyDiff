//! Display widths of code rows, so the frontend can lay out wrapped lines (their heights) for
//! the whole diff without rendering them.

use crate::highlight::Seg;
use crate::model::{row_kind, Row};
use crate::split::SplitRow;

/// Columns a tab advances to (the browser's default `tab-size`).
pub const TAB_WIDTH: u32 = 8;

/// The monospace columns `segs` take: tabs go to the next tab stop, wide (CJK, emoji) characters
/// take two.
pub fn text_width(segs: &[Seg]) -> u32 {
    let mut width = 0;
    for ch in segs.iter().flat_map(|(_, text)| text.chars()) {
        width += match ch {
            '\t' => TAB_WIDTH - width % TAB_WIDTH,
            _ if is_wide(ch) => 2,
            _ => 1,
        };
    }
    width
}

fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD)
}

/// `[offset, width]` of a file's unified code rows wider than `min_width` (offset 0 is the header).
pub fn unified_widths(rows: &[Row], min_width: u32) -> Vec<[u32; 2]> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| matches!(row.k, row_kind::CONTEXT | row_kind::ADDED | row_kind::DELETED))
        .map(|(offset, row)| [offset as u32, text_width(&row.s)])
        .filter(|[_, width]| *width > min_width)
        .collect()
}

/// `[offset, width]` of a file's side-by-side rows wider than `min_width`, where a row's width is
/// its wider side (both halves are equally wide). Offset 0 is the header, so row `i` is `i + 1`.
pub fn split_widths(rows: &[SplitRow], min_width: u32) -> Vec<[u32; 2]> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| [i as u32 + 1, text_width(&row.os).max(text_width(&row.ns))])
        .filter(|[_, width]| *width > min_width)
        .collect()
}
