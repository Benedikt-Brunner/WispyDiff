//! The prepared, highlighted row model the frontend renders from.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::attribution::LineAttr;
use crate::diff::{FileDiff, FileStatus, LineKind};
use crate::error::{Error, Result};
use crate::git::Git;
use crate::highlight::{plain_lines, Language, Seg};
use crate::highlight_cache::Lines;
use crate::noise::path_noise;
use crate::split::align;

/// Bump whenever [`DiffView`] or its computation changes, to invalidate cached views.
pub const MODEL_VERSION: u32 = 5;

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
    /// Stack index of the PR that last touched this line (added/deleted rows only).
    pub a: Option<u8>,
    /// Every PR that shaped this line, oldest first, when more than one did.
    pub h: Vec<u8>,
    /// The line's number in PR `a`'s own diff (where a comment on it must point).
    pub l: Option<u32>,
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
    /// Stack indices of the PRs that touched this file within the range.
    pub prs: Vec<u8>,
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
    /// Offsets (within this file's unified rows, header = 0) of each hunk header.
    pub hunks: Vec<u32>,
    /// Row count of the full-file side-by-side (header included); 0 when unavailable
    /// (binary, rename-only, ...).
    pub split_rows: u32,
    /// Offsets (within the side-by-side rows, header = 0) of each change block.
    pub split_blocks: Vec<u32>,
    /// Why the file is collapsed by default ("lockfile", "generated", ...), if it is.
    pub noise: Option<String>,
    /// Line counts of the base and head versions (0 when not a text file with hunks).
    pub old_lines: u32,
    pub new_lines: u32,
    /// The file's path in a PR of the range, where it differs from `path` (renamed later).
    pub pr_paths: Vec<(u8, String)>,
    /// Identifies this file's change by content (paths and changed/context text, not line
    /// numbers or SHAs), so a "viewed" mark survives rebases that don't touch it.
    pub content_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub base_sha: String,
    pub head_sha: String,
    pub files: Vec<FileSummary>,
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

    /// Builds rows for `files`. `attributions` holds one entry per hunk line of each file (see
    /// [`crate::attribution::attribute`]); `highlights` maps blob SHAs to highlighted full files;
    /// `generated` lists paths marked `linguist-generated`.
    pub fn build(
        base_sha: &str,
        head_sha: &str,
        files: &[FileDiff],
        attributions: &[Vec<LineAttr>],
        highlights: &HashMap<String, Lines>,
        generated: &HashSet<String>,
    ) -> DiffView {
        let per_file: Vec<Vec<Row>> = files
            .par_iter()
            .zip(attributions)
            .enumerate()
            .map(|(index, (file, attrs))| file_rows(index as u32, file, attrs, highlights))
            .collect();
        // Per file: the path each PR knew it under, where that differs from the shown path.
        let mut renamed_in: HashMap<usize, Vec<(u8, String)>> = HashMap::new();
        for (index, attrs) in attributions.iter().enumerate() {
            let mut paths: Vec<(u8, String)> =
                attrs.iter().filter_map(|a| Some((a.pr?, a.path.clone()?))).collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                renamed_in.insert(index, paths);
            }
        }

        let mut rows = Vec::with_capacity(per_file.iter().map(Vec::len).sum());
        let mut summaries = Vec::with_capacity(files.len());
        let mut max_line_chars = 0u32;
        let mut max_line_number = 0u32;
        for (file, file_rows) in files.iter().zip(per_file) {
            let first_row = rows.len() as u32;
            let mut prs: Vec<u8> = file_rows.iter().filter_map(|r| r.a).collect();
            prs.sort_unstable();
            prs.dedup();
            let mut hunks = Vec::new();
            for (offset, row) in file_rows.iter().enumerate() {
                if row.k == row_kind::HUNK {
                    hunks.push(offset as u32);
                }
                let chars: usize = row.s.iter().map(|(_, text)| text.chars().count()).sum();
                max_line_chars = max_line_chars.max(chars as u32);
                max_line_number = max_line_number.max(row.o.unwrap_or(0)).max(row.n.unwrap_or(0));
            }
            let counts = line_counts(file, highlights);
            let (split_rows, split_blocks) = match counts {
                Some((old_len, new_len)) => {
                    let alignment = align(&file_rows, old_len, new_len);
                    (alignment.pairs.len() as u32 + 1, alignment.blocks.iter().map(|b| b + 1).collect())
                }
                None => (0, Vec::new()),
            };
            let (old_lines, new_lines) = counts.unwrap_or((0, 0));
            let noise = if generated.contains(file.path()) { Some("generated") } else { path_noise(file.path()) };
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
                prs,
                old_blob: file.old_blob.clone(),
                new_blob: file.new_blob.clone(),
                hunks,
                split_rows,
                split_blocks,
                noise: noise.map(str::to_string),
                old_lines,
                new_lines,
                pr_paths: renamed_in.remove(&(summaries.len())).unwrap_or_default(),
                content_key: content_key(file),
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
                total_rows: rows.len() as u32,
                max_line_chars,
                max_line_number,
            },
            rows,
        }
    }
}

/// Old and new line counts of a text file with hunks, from whichever side was highlighted
/// (the other follows from the diff's additions and deletions).
fn line_counts(file: &FileDiff, highlights: &HashMap<String, Lines>) -> Option<(u32, u32)> {
    if file.binary || file.hunks.is_empty() {
        return None;
    }
    let len = |blob: &Option<String>| blob.as_ref().and_then(|b| highlights.get(b)).map(|l| l.len() as i64);
    let delta = i64::from(file.additions()) - i64::from(file.deletions());
    let (old, new) = match (len(&file.old_blob), len(&file.new_blob)) {
        (Some(old), Some(new)) => (old, new),
        (Some(old), None) => (old, if file.new_blob.is_some() { old + delta } else { 0 }),
        (None, Some(new)) => (if file.old_blob.is_some() { new - delta } else { 0 }, new),
        (None, None) => return None,
    };
    Some((old.max(0) as u32, new.max(0) as u32))
}

/// FNV-1a over the file's paths and diff text: stable across Rust versions and processes.
fn content_key(file: &FileDiff) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(file.old_path.as_deref().unwrap_or("").as_bytes());
    feed(b"\0");
    feed(file.new_path.as_deref().unwrap_or("").as_bytes());
    feed(if file.binary { b"\0binary" } else { b"\0text" });
    for line in file.hunks.iter().flat_map(|h| &h.lines) {
        feed(match line.kind {
            LineKind::Context => b" ",
            LineKind::Added => b"+",
            LineKind::Deleted => b"-",
        });
        feed(line.text.as_bytes());
        feed(b"\n");
    }
    format!("{hash:016x}")
}

/// The rows of file `index` (header first) as a slice of `rows`.
pub fn file_slice<'a>(view: &'a DiffView, index: usize) -> &'a [Row] {
    let file = &view.summary.files[index];
    view.rows(file.first_row, file.first_row + file.row_count)
}

fn file_rows(index: u32, file: &FileDiff, attrs: &[LineAttr], highlights: &HashMap<String, Lines>) -> Vec<Row> {
    let row = |k: u8, o: Option<u32>, n: Option<u32>, s: Vec<Seg>| Row { k, f: index, o, n, s, a: None, h: Vec::new(), l: None };
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

    let old_lines = file.old_blob.as_ref().and_then(|b| highlights.get(b));
    let new_lines = file.new_blob.as_ref().and_then(|b| highlights.get(b));
    let mut attrs = attrs.iter();

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
                .and_then(|lines| lines.get(number? as usize - 1))
                .filter(|segs| segs_match(segs, text))
                .cloned()
                .unwrap_or_else(|| plain_lines(text).pop().unwrap_or_default());
            let attr = attrs.next().cloned().unwrap_or_default();
            rows.push(Row { a: attr.pr, h: attr.history, l: attr.line, ..row(kind, line.old_no, line.new_no, segments) });
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
