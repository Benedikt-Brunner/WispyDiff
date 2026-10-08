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

/// Where head line `line` of file `file` is in a view (for jumping to a search hit).
pub fn locate_new_line(view: &DiffView, file: usize, line: u32) -> Option<Location> {
    let summary = view.summary.files.get(file)?;
    let rows = file_slice(view, file);
    let unified = rows.iter().position(|r| matches!(r.k, row_kind::ADDED | row_kind::CONTEXT) && r.n == Some(line));
    let split = (summary.split_rows > 0)
        .then(|| align(rows, summary.old_lines, summary.new_lines).pairs.iter().position(|p| p.n == Some(line)).map(|i| i as u32 + 1))
        .flatten();
    Some(Location { file: file as u32, unified: unified.map(|u| u as u32), split })
}
/// Where a comment on head lines of a file in a view goes (a range whose top PR is `top`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadTarget {
    /// Stack index of the PR, and the file's path there.
    pub pr: u8,
    pub path: String,
    /// The lines in that PR's diff (RIGHT side); `None` for a comment on the whole file.
    pub line: Option<u32>,
    pub start_line: Option<u32>,
}

/// Where a comment on head lines `start..=end` of `path` goes (`None`: the whole file), as the
/// diff's gutter routes it: an added line to the PR that last touched it, at its line there;
/// any other line to the highest PR that touched the file (its numbers there are the head's).
/// A range whose ends belong to different PRs is cut to its end line. `Err` says why it can't go.
pub fn head_target(view: &DiffView, path: &str, lines: Option<(u32, u32)>) -> std::result::Result<HeadTarget, String> {
    let index = view.summary.files.iter().position(|f| f.path == path).ok_or_else(|| format!("{path} isn't changed in this range"))?;
    let file = &view.summary.files[index];
    let owner = *file.prs.iter().max().ok_or_else(|| format!("{path} isn't changed in this range"))?;
    let path_in = |pr: u8| file.pr_paths.iter().find(|(p, _)| *p == pr).map_or_else(|| file.path.clone(), |(_, p)| p.clone());
    let Some((start, end)) = lines else {
        return Ok(HeadTarget { pr: owner, path: path_in(owner), line: None, start_line: None });
    };
    let rows = file_slice(view, index);
    let at = |line: u32| -> Option<(u8, u32)> {
        match rows.iter().find(|r| r.n == Some(line) && matches!(r.k, row_kind::ADDED | row_kind::CONTEXT)) {
            Some(r) if r.k == row_kind::ADDED => Some((r.a?, r.l?)),
            _ => (line >= 1 && line <= file.new_lines).then_some((owner, line)),
        }
    };
    let (pr, line) = at(end).ok_or_else(|| format!("{path} has no line {end} at the head"))?;
    let start_line = (start < end).then(|| at(start)).flatten().filter(|(p, l)| *p == pr && *l < line).map(|(_, l)| l);
    Ok(HeadTarget { pr, path: path_in(pr), line: Some(line), start_line })
}
