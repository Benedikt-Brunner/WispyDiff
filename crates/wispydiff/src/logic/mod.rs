//! Pure frontend policies, ported from the React client. No window or application state.
pub mod comments;
pub mod file_refs;
pub mod file_tree;
pub mod fuzzy;
pub mod gaps;
pub mod glob;
pub mod history;
pub mod images;
pub mod indent;
pub mod layout;
pub mod palette;

#[cfg(test)]
pub(crate) fn test_file() -> wispy_core::model::FileSummary {
    use wispy_core::{diff::FileStatus, model::FileSummary};
    FileSummary {
        path: "src/new.rs".into(),
        old_path: Some("old.rs".into()),
        status: FileStatus::Modified,
        additions: 1,
        deletions: 1,
        binary: false,
        language: None,
        first_row: 0,
        row_count: 8,
        prs: vec![],
        old_blob: None,
        new_blob: None,
        hunks: vec![1, 5],
        gaps: vec![],
        split_rows: 12,
        split_blocks: vec![3, 8],
        noise: None,
        old_lines: 10,
        new_lines: 10,
        pr_paths: vec![],
        content_key: String::new(),
    }
}

/// ECMAScript whitespace (Rust's `is_whitespace` additionally includes U+0085).
pub(crate) fn js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(js_whitespace)
}
