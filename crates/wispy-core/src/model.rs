//! The prepared, highlighted row model the frontend renders from.

use std::collections::HashMap;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::diff::{parse_patch, FileDiff, FileStatus, LineKind, PATCH_ARGS};
use crate::error::{Error, Result};
use crate::git::Git;
use crate::highlight::{plain_lines, Highlighter, Language, Seg};

/// Bump whenever [`DiffView`] or its computation changes, to invalidate cached views.
pub const MODEL_VERSION: u32 = 1;

pub mod row_kind {
    pub const FILE: u8 = 0;
    pub const HUNK: u8 = 1;
    pub const CONTEXT: u8 = 2;
    pub const ADDED: u8 = 3;
    pub const DELETED: u8 = 4;
    /// Explanatory row, e.g. "Binary file" or "File renamed without changes".
    pub const NOTICE: u8 = 5;
}

/// One rendered line. Field names are short because rows cross the IPC bridge in bulk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// See [`row_kind`].
    pub k: u8,
    /// Index into [`DiffView::files`].
    pub f: u32,
    /// Old / new line numbers (1-based).
    pub o: Option<u32>,
    pub n: Option<u32>,
    /// Highlighted segments.
    pub s: Vec<Seg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSummary {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub language: Option<Language>,
    pub first_row: u32,
    pub row_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub base_sha: String,
    pub head_sha: String,
    pub files: Vec<FileSummary>,
    /// Row indices of every hunk header, for `j`/`k` navigation.
    pub hunk_rows: Vec<u32>,
    pub total_rows: u32,
    /// Longest line in characters, so the frontend can size horizontal scrolling up front.
    pub max_line_chars: u32,
    /// Largest old/new line number shown, so the gutter width is known up front.
    pub max_line_number: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffView {
    pub summary: DiffSummary,
    pub rows: Vec<Row>,
}

impl DiffView {
    pub fn rows(&self, start: u32, end: u32) -> &[Row] {
        let len = self.rows.len();
        let start = (start as usize).min(len);
        let end = (end as usize).clamp(start, len);
        &self.rows[start..end]
    }

    /// Builds rows for `files`, highlighting against the full old/new file contents in `blobs`.
    pub fn build(
        base_sha: &str,
        head_sha: &str,
        files: &[FileDiff],
        blobs: &HashMap<String, Vec<u8>>,
        highlighter: &Highlighter,
    ) -> DiffView {
        let per_file: Vec<Vec<Row>> = files
            .par_iter()
            .enumerate()
            .map(|(index, file)| file_rows(index as u32, file, blobs, highlighter))
            .collect();

        let mut rows = Vec::with_capacity(per_file.iter().map(Vec::len).sum());
        let mut summaries = Vec::with_capacity(files.len());
        let mut hunk_rows = Vec::new();
        let mut max_line_chars = 0u32;
        let mut max_line_number = 0u32;
        for (file, file_rows) in files.iter().zip(per_file) {
            let first_row = rows.len() as u32;
            for (offset, row) in file_rows.iter().enumerate() {
                if row.k == row_kind::HUNK {
                    hunk_rows.push(first_row + offset as u32);
                }
                let chars: usize = row.s.iter().map(|(_, text)| text.chars().count()).sum();
                max_line_chars = max_line_chars.max(chars as u32);
                max_line_number = max_line_number.max(row.o.unwrap_or(0)).max(row.n.unwrap_or(0));
            }
            summaries.push(FileSummary {
                path: file.path().to_string(),
                old_path: file.old_path.clone().filter(|old| Some(old) != file.new_path.as_ref()),
                status: file.status,
                additions: file.additions(),
                deletions: file.deletions(),
                binary: file.binary,
                language: Language::from_path(file.path()),
                first_row,
                row_count: file_rows.len() as u32,
            });
            rows.extend(file_rows);
        }

        DiffView {
            summary: DiffSummary {
                base_sha: base_sha.to_string(),
                head_sha: head_sha.to_string(),
                additions: summaries.iter().map(|f| f.additions).sum(),
                deletions: summaries.iter().map(|f| f.deletions).sum(),
                files: summaries,
                hunk_rows,
                total_rows: rows.len() as u32,
                max_line_chars,
                max_line_number,
            },
            rows,
        }
    }

    /// Diffs `from..to` in `git` (a partial clone: missing blobs are batch-fetched by git).
    pub fn compute(git: &Git, from: &str, to: &str, highlighter: &Highlighter) -> Result<DiffView> {
        let mut args = PATCH_ARGS.to_vec();
        args.extend([from, to, "--"]);
        let patch = git.run(&args)?;
        let files = parse_patch(&patch)?;
        let wanted: Vec<&str> = files
            .iter()
            .filter(|f| !f.binary && !f.hunks.is_empty())
            .flat_map(|f| [f.old_blob.as_deref(), f.new_blob.as_deref()])
            .flatten()
            .collect();
        let blobs = read_blobs(git, &wanted)?;
        Ok(DiffView::build(from, to, &files, &blobs, highlighter))
    }
}

fn file_rows(index: u32, file: &FileDiff, blobs: &HashMap<String, Vec<u8>>, highlighter: &Highlighter) -> Vec<Row> {
    let row = |k: u8, o: Option<u32>, n: Option<u32>, s: Vec<Seg>| Row { k, f: index, o, n, s };
    let mut rows = vec![row(row_kind::FILE, None, None, vec![(0, file.path().to_string())])];

    if file.binary {
        rows.push(row(row_kind::NOTICE, None, None, vec![(0, "Binary file not shown".into())]));
        return rows;
    }
    if file.hunks.is_empty() {
        let notice = match file.status {
            FileStatus::Renamed => "File renamed without changes",
            FileStatus::Copied => "File copied without changes",
            FileStatus::Added => "Empty file added",
            FileStatus::Deleted => "Empty file deleted",
            FileStatus::Modified => "File mode changed",
        };
        rows.push(row(row_kind::NOTICE, None, None, vec![(0, notice.into())]));
        return rows;
    }

    let language = Language::from_path(file.path());
    let has_kind = |kind: LineKind| file.hunks.iter().flat_map(|h| &h.lines).any(|l| l.kind == kind);
    let side = |blob: &Option<String>, needed: bool| -> Option<Vec<Vec<Seg>>> {
        let content = blobs.get(blob.as_deref()?)?;
        needed.then(|| highlighter.highlight(language, &String::from_utf8_lossy(content)))
    };
    let old_lines = side(&file.old_blob, has_kind(LineKind::Deleted));
    let new_lines = side(&file.new_blob, has_kind(LineKind::Added) || has_kind(LineKind::Context));

    for hunk in &file.hunks {
        rows.push(row(row_kind::HUNK, None, None, vec![(0, hunk.header())]));
        for line in &hunk.lines {
            let (kind, source, number) = match line.kind {
                LineKind::Context => (row_kind::CONTEXT, &new_lines, line.new_no),
                LineKind::Added => (row_kind::ADDED, &new_lines, line.new_no),
                LineKind::Deleted => (row_kind::DELETED, &old_lines, line.old_no),
            };
            let text = line.text.strip_suffix('\r').unwrap_or(&line.text);
            let segments = source
                .as_ref()
                .and_then(|lines| lines.get(number? as usize - 1))
                .filter(|segs| segs_match(segs, text))
                .cloned()
                .unwrap_or_else(|| plain_lines(text).pop().unwrap_or_default());
            rows.push(row(kind, line.old_no, line.new_no, segments));
        }
    }
    rows
}

fn segs_match(segs: &[Seg], text: &str) -> bool {
    let mut rest = text;
    for (_, piece) in segs {
        match rest.strip_prefix(piece.as_str()) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    rest.is_empty()
}

/// Reads blobs with one `git cat-file --batch` call.
pub fn read_blobs(git: &Git, shas: &[&str]) -> Result<HashMap<String, Vec<u8>>> {
    let mut blobs = HashMap::with_capacity(shas.len());
    if shas.is_empty() {
        return Ok(blobs);
    }
    let mut input = shas.join("\n");
    input.push('\n');
    let out = git.run_with_stdin(&["cat-file", "--batch"], input.as_bytes())?;

    let mut pos = 0;
    while pos < out.len() {
        let header_end = out[pos..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| pos + i)
            .ok_or_else(|| Error::DiffParse("truncated cat-file output".into()))?;
        let header = String::from_utf8_lossy(&out[pos..header_end]).to_string();
        pos = header_end + 1;
        let mut parts = header.split(' ');
        let sha = parts.next().unwrap_or_default().to_string();
        if parts.next() == Some("missing") {
            continue;
        }
        let size: usize = parts
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::DiffParse(format!("bad cat-file header {header:?}")))?;
        let end = pos + size;
        if end > out.len() {
            return Err(Error::DiffParse("truncated cat-file object".into()));
        }
        blobs.insert(sha, out[pos..end].to_vec());
        pos = end + 1;
    }
    Ok(blobs)
}
