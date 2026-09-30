use wispy_core::diff::{parse_patch, FileStatus, LineKind};

const BLOB_A: &str = "1111111111111111111111111111111111111111";
const BLOB_B: &str = "2222222222222222222222222222222222222222";
const NULL: &str = "0000000000000000000000000000000000000000";

#[test]
fn parses_a_modified_file_with_line_numbers() {
    let patch = format!(
        "diff --git a/src/Stock.php b/src/Stock.php\n\
         index {BLOB_A}..{BLOB_B} 100644\n\
         --- a/src/Stock.php\n\
         +++ b/src/Stock.php\n\
         @@ -10,4 +10,5 @@ class Stock\n \
         keep one\n\
         -old line\n\
         +new line\n\
         +another\n \
         keep two\n \
         keep three\n"
    );
    let files = parse_patch(patch.as_bytes()).unwrap();
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.status, FileStatus::Modified);
    assert_eq!(file.path(), "src/Stock.php");
    assert_eq!(file.old_blob.as_deref(), Some(BLOB_A));
    assert_eq!(file.new_blob.as_deref(), Some(BLOB_B));
    assert_eq!((file.additions(), file.deletions()), (2, 1));

    let hunk = &file.hunks[0];
    assert_eq!(hunk.section, "class Stock");
    assert_eq!(hunk.header(), "@@ -10,4 +10,5 @@ class Stock");
    let numbered: Vec<_> = hunk.lines.iter().map(|l| (l.kind, l.old_no, l.new_no, l.text.as_str())).collect();
    assert_eq!(
        numbered,
        vec![
            (LineKind::Context, Some(10), Some(10), "keep one"),
            (LineKind::Deleted, Some(11), None, "old line"),
            (LineKind::Added, None, Some(11), "new line"),
            (LineKind::Added, None, Some(12), "another"),
            (LineKind::Context, Some(12), Some(13), "keep two"),
            (LineKind::Context, Some(13), Some(14), "keep three"),
        ]
    );
}

#[test]
fn parses_added_deleted_renamed_and_binary_files() {
    let patch = format!(
        "diff --git a/new.ts b/new.ts\n\
         new file mode 100644\n\
         index {NULL}..{BLOB_B}\n\
         --- /dev/null\n\
         +++ b/new.ts\n\
         @@ -0,0 +1,2 @@\n\
         +export const a = 1;\n\
         +export const b = 2;\n\
         diff --git a/gone.php b/gone.php\n\
         deleted file mode 100644\n\
         index {BLOB_A}..{NULL}\n\
         --- a/gone.php\n\
         +++ /dev/null\n\
         @@ -1 +0,0 @@\n\
         -<?php\n\
         diff --git a/old name.md b/new name.md\n\
         similarity index 100%\n\
         rename from old name.md\n\
         rename to new name.md\n\
         diff --git a/logo.png b/logo.png\n\
         index {BLOB_A}..{BLOB_B} 100644\n\
         Binary files a/logo.png and b/logo.png differ\n"
    );
    let files = parse_patch(patch.as_bytes()).unwrap();
    let summary: Vec<_> = files
        .iter()
        .map(|f| (f.status, f.old_path.as_deref(), f.new_path.as_deref(), f.binary, f.hunks.len()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (FileStatus::Added, None, Some("new.ts"), false, 1),
            (FileStatus::Deleted, Some("gone.php"), None, false, 1),
            (FileStatus::Renamed, Some("old name.md"), Some("new name.md"), false, 0),
            (FileStatus::Modified, Some("logo.png"), Some("logo.png"), true, 0),
        ]
    );
    assert_eq!(files[0].old_blob, None);
    assert_eq!(files[1].new_blob, None);
    assert_eq!(files[1].path(), "gone.php");
}

#[test]
fn ignores_no_newline_markers_and_keeps_lines_that_look_like_headers() {
    let patch = format!(
        "diff --git a/a.txt b/a.txt\n\
         index {BLOB_A}..{BLOB_B} 100644\n\
         --- a/a.txt\n\
         +++ b/a.txt\n\
         @@ -1,2 +1,2 @@\n\
         --- not a header\n\
         -last\n\
         \\ No newline at end of file\n\
         +++ not a header either\n\
         +last\n"
    );
    let files = parse_patch(patch.as_bytes()).unwrap();
    let texts: Vec<_> = files[0].hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
    assert_eq!(
        texts,
        vec![
            (LineKind::Deleted, "-- not a header"),
            (LineKind::Deleted, "last"),
            (LineKind::Added, "++ not a header either"),
            (LineKind::Added, "last"),
        ]
    );
}

#[test]
fn unquotes_paths_with_special_characters() {
    let patch = format!(
        "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\n\
         index {BLOB_A}..{BLOB_B} 100644\n\
         --- \"a/caf\\303\\251.txt\"\n\
         +++ \"b/caf\\303\\251.txt\"\n\
         @@ -1 +1 @@\n\
         -a\n\
         +b\n"
    );
    let files = parse_patch(patch.as_bytes()).unwrap();
    assert_eq!(files[0].path(), "café.txt");
}

#[test]
fn rejects_truncated_hunks() {
    let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n context\n";
    assert!(parse_patch(patch.as_bytes()).is_err());
}

#[test]
fn parses_empty_input() {
    assert!(parse_patch(b"").unwrap().is_empty());
}
