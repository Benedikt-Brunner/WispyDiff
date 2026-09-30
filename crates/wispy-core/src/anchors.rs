//! Where review comments can go: which lines a PR's diff on GitHub accepts, how a line moves
//! when the PR gets new commits, and the file-comment fallback (permalink + quoted snippet).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::attribution::map_line_forward;
use crate::model::{file_slice, row_kind, DiffView};
use crate::split::align;
use crate::diff::{parse_patch, FileDiff, PATCH_ARGS};
use crate::error::Result;
use crate::git::Git;
use crate::github::Side;

/// Line ranges GitHub accepts comments on for one PR: the hunks of its diff, per side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    right: HashMap<String, Vec<(u32, u32)>>,
    left: HashMap<String, Vec<(u32, u32)>>,
}

impl Coverage {
    /// Coverage of the diff `base..head` (the PR's three-dot diff).
    pub fn compute(git: &Git, base: &str, head: &str) -> Result<Self> {
        let mut args = PATCH_ARGS.to_vec();
        args.extend([base, head, "--"]);
        Ok(Self::from_files(&parse_patch(&git.run(&args)?)?))
    }

    pub fn from_files(files: &[FileDiff]) -> Self {
        let mut coverage = Coverage::default();
        for file in files {
            for hunk in &file.hunks {
                if let (Some(path), true) = (&file.new_path, hunk.new_count > 0) {
                    coverage.right.entry(path.clone()).or_default().push((hunk.new_start, hunk.new_start + hunk.new_count - 1));
                }
                if let (Some(path), true) = (&file.old_path, hunk.old_count > 0) {
                    coverage.left.entry(path.clone()).or_default().push((hunk.old_start, hunk.old_start + hunk.old_count - 1));
                }
            }
        }
        coverage
    }

    /// Whether lines `start..=end` of `path` on `side` all lie in one hunk.
    pub fn accepts(&self, path: &str, side: Side, start: u32, end: u32) -> bool {
        let ranges = match side {
            Side::Right => self.right.get(path),
            Side::Left => self.left.get(path),
        };
        ranges.is_some_and(|ranges| ranges.iter().any(|&(lo, hi)| lo <= start && end <= hi))
    }
}

/// Where line `line` of `path` at `from` is at `to`, if it still exists unchanged.
pub fn remap_line(git: &Git, from: &str, to: &str, path: &str, line: u32) -> Result<Option<(String, u32)>> {
    if from == to {
        return Ok(Some((path.to_string(), line)));
    }
    let mut args = PATCH_ARGS.to_vec();
    args.extend([from, to, "--"]);
    let files = parse_patch(&git.run(&args)?)?;
    Ok(match files.iter().find(|f| f.old_path.as_deref() == Some(path)) {
        None => Some((path.to_string(), line)),
        Some(file) => match (&file.new_path, map_line_forward(file, line)) {
            (Some(new_path), Some(new_line)) => Some((new_path.clone(), new_line)),
            _ => None,
        },
    })
}

/// Lines `start..=end` of `path` at `commit`, as plain text.
pub fn read_lines(git: &Git, commit: &str, path: &str, start: u32, end: u32) -> Result<Vec<String>> {
    let content = git.run(&["show", &format!("{commit}:{path}")])?;
    let text = String::from_utf8_lossy(&content);
    Ok(text.lines().skip(start.saturating_sub(1) as usize).take((end + 1).saturating_sub(start) as usize).map(str::to_string).collect())
}

/// A file-level comment body pointing at lines GitHub wouldn't take a line comment on.
pub fn quoted_body(repo: &str, commit: &str, path: &str, start: u32, end: u32, lines: &[String], body: &str) -> String {
    let range = if start == end { format!("L{start}") } else { format!("L{start}-L{end}") };
    let label = if start == end { format!("L{start}") } else { format!("L{start}–{end}") };
    let fence = if lines.iter().any(|l| l.contains("```")) { "~~~~" } else { "```" };
    let language = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    format!(
        "[{label}](https://github.com/{repo}/blob/{commit}/{path}#{range})\n{fence}{language}\n{}\n{fence}\n\n{body}",
        lines.join("\n")
    )
}

/// A PR-level comment position: line `line` on `side` of `path` in stack PR `pr`'s own diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub pr: u8,
    pub path: String,
    pub side: Side,
    pub line: u32,
}

/// Where an anchor shows up in a view: its file, and its row offset within that file's unified
/// and side-by-side rows (either may be missing, e.g. an unchanged line outside every hunk has
/// only a side-by-side row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub file: u32,
    pub unified: Option<u32>,
    pub split: Option<u32>,
}

/// Finds `anchor` in `view`, a range whose top PR is stack index `top` (unchanged lines belong
/// to the top PR, numbered as on its head — the view's head).
pub fn locate(view: &DiffView, top: u8, anchor: &Anchor) -> Option<Location> {
    let file = view.summary.files.iter().position(|f| {
        f.path == anchor.path
            || f.old_path.as_deref() == Some(anchor.path.as_str())
            || f.pr_paths.iter().any(|(pr, p)| *pr == anchor.pr && *p == anchor.path)
    })?;
    let summary = &view.summary.files[file];
    let rows = file_slice(view, file);
    let unified = rows.iter().position(|r| match anchor.side {
        Side::Left => r.k == row_kind::DELETED && r.a == Some(anchor.pr) && r.l == Some(anchor.line),
        Side::Right => match r.k {
            row_kind::ADDED => r.a == Some(anchor.pr) && r.l == Some(anchor.line),
            row_kind::CONTEXT => anchor.pr == top && r.n == Some(anchor.line),
            _ => false,
        },
    });
    let split = (summary.split_rows > 0)
        .then(|| {
            let pairs = align(rows, summary.old_lines, summary.new_lines).pairs;
            let index = match unified.map(|u| &rows[u]) {
                Some(row) if row.k == row_kind::DELETED => pairs.iter().position(|p| p.o == row.o && p.ok == row_kind::DELETED),
                Some(row) => pairs.iter().position(|p| p.n == row.n && p.nk != crate::split::FILLER),
                None if anchor.side == Side::Right && anchor.pr == top => pairs.iter().position(|p| p.n == Some(anchor.line)),
                None => None,
            };
            index.map(|i| i as u32 + 1)
        })
        .flatten();
    (unified.is_some() || split.is_some()).then_some(Location { file: file as u32, unified: unified.map(|u| u as u32), split })
}