//! Which PR in a stack range last touched each changed line.
//!
//! The combined diff `base..top` is decomposed into one step per PR (`base..h1`, `h1..h2`, …).
//! An added line is traced backwards through the steps until the step that added it; a deleted
//! line is traced forwards until the step that deleted it. Following the lines through each
//! step's hunks (and renames) is what makes "the last PR that touched it" exact rather than a
//! per-file guess.

use std::collections::HashMap;

use crate::diff::{FileDiff, LineKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineAttr {
    /// Stack index of the PR that last touched the line; `None` for context lines.
    pub pr: Option<u8>,
    /// Every PR that shaped this change, oldest first, when more than one did
    /// (e.g. "added in #2, modified in #3" → `[1, 2]`).
    pub history: Vec<u8>,
    /// The line's number in that PR's own diff (new side for added lines, old side for
    /// deleted ones) — where a review comment on that PR has to point.
    pub line: Option<u32>,
    /// The file's path in that PR, when it differs from the path shown (renamed later).
    pub path: Option<String>,
}

/// One PR's contribution: the diff from the previous head to this PR's head.
pub struct Step<'a> {
    pub pr: u8,
    pub files: &'a [FileDiff],
}

/// Attribution for every line of every file in `combined`, flattened over each file's hunks
/// in order. With a single step every change belongs to that PR.
pub fn attribute(combined: &[FileDiff], steps: &[Step]) -> Vec<Vec<LineAttr>> {
    if let [only] = steps {
        return combined
            .iter()
            .map(|file| {
                file.hunks
                    .iter()
                    .flat_map(|h| &h.lines)
                    .map(|line| match line.kind {
                        LineKind::Context => LineAttr::default(),
                        LineKind::Added => LineAttr { pr: Some(only.pr), line: line.new_no, ..LineAttr::default() },
                        LineKind::Deleted => LineAttr { pr: Some(only.pr), line: line.old_no, ..LineAttr::default() },
                    })
                    .collect()
            })
            .collect();
    }

    let indexed: Vec<StepIndex> = steps.iter().map(StepIndex::new).collect();
    combined
        .iter()
        .map(|file| {
            let fallback = indexed.iter().rev().find(|s| s.touches(file)).map(|s| s.pr);
            file.hunks
                .iter()
                .flat_map(|h| &h.lines)
                .map(|line| match line.kind {
                    LineKind::Context => LineAttr::default(),
                    LineKind::Added => {
                        let path = file.new_path.as_deref().unwrap_or_default();
                        match origin_of_new(&indexed, indexed.len(), path, line.new_no.unwrap_or(0)) {
                            Some(origin) => {
                                let at = &indexed[origin.step].files[origin.file].file;
                                let origin_path = at.new_path.clone().filter(|p| p != path);
                                LineAttr {
                                    pr: Some(indexed[origin.step].pr),
                                    history: history(&indexed, &origin),
                                    line: at.hunks[origin.hunk].lines[origin.line].new_no,
                                    path: origin_path,
                                }
                            }
                            None => LineAttr { pr: fallback, ..LineAttr::default() },
                        }
                    }
                    LineKind::Deleted => {
                        let path = file.old_path.as_deref().unwrap_or_default();
                        match fate_of_old(&indexed, path, line.old_no.unwrap_or(0)) {
                            Some(fate) => LineAttr {
                                pr: Some(indexed[fate.step].pr),
                                line: Some(fate.line),
                                path: Some(fate.path).filter(|p| p != path),
                                ..LineAttr::default()
                            },
                            None => LineAttr { pr: fallback, ..LineAttr::default() },
                        }
                    }
                })
                .collect()
        })
        .collect()
}

/// Where a line was added: step, file within that step, hunk and line within that file.
struct Origin {
    step: usize,
    file: usize,
    hunk: usize,
    line: usize,
}

/// Traces line `n` of `path` at version `version` (after `version` steps) back to the step that
/// added it. `None` if it predates every step (the combined diff disagreed with the steps).
fn origin_of_new(steps: &[StepIndex], version: usize, path: &str, n: u32) -> Option<Origin> {
    let (mut path, mut n) = (path.to_string(), n);
    for step in (0..version).rev() {
        let Some(file) = steps[step].by_new_path(&path) else { continue };
        match steps[step].files[file].map_back(n) {
            Back::Added { hunk, line } => return Some(Origin { step, file, hunk, line }),
            Back::Old(old) => {
                n = old;
                if let Some(old_path) = &steps[step].files[file].file.old_path {
                    path = old_path.clone();
                }
            }
        }
    }
    None
}

/// Where a base line was deleted: the step, and the line's number and path in that step's diff.
struct Fate {
    step: usize,
    line: u32,
    path: String,
}

/// Traces base line `o` of `path` forwards to the step that deleted it.
fn fate_of_old(steps: &[StepIndex], path: &str, o: u32) -> Option<Fate> {
    let (mut path, mut o) = (path.to_string(), o);
    for (index, step) in steps.iter().enumerate() {
        let Some(file) = step.by_old_path(&path) else { continue };
        match step.files[file].map_forward(o) {
            Forward::Deleted => return Some(Fate { step: index, line: o, path }),
            Forward::New(new) => {
                o = new;
                if let Some(new_path) = &step.files[file].file.new_path {
                    path = new_path.clone();
                }
            }
        }
    }
    None
}

/// PRs whose lines the change block containing `origin` replaced, plus the origin PR itself.
fn history(steps: &[StepIndex], origin: &Origin) -> Vec<u8> {
    let file = &steps[origin.step].files[origin.file];
    let lines = &file.file.hunks[origin.hunk].lines;
    let is_change = |i: &usize| lines[*i].kind != LineKind::Context;
    let start = (0..origin.line).rev().take_while(is_change).last().unwrap_or(origin.line);
    let end = (origin.line..lines.len()).take_while(is_change).last().unwrap_or(origin.line);
    let old_path = file.file.old_path.as_deref().unwrap_or_default();

    let mut prs: Vec<u8> = lines[start..=end]
        .iter()
        .filter(|l| l.kind == LineKind::Deleted)
        .filter_map(|l| origin_of_new(steps, origin.step, old_path, l.old_no?))
        .map(|o| steps[o.step].pr)
        .collect();
    if prs.is_empty() {
        return Vec::new();
    }
    prs.push(steps[origin.step].pr);
    prs.sort_unstable();
    prs.dedup();
    if prs.len() > 1 {
        prs
    } else {
        Vec::new()
    }
}

struct StepIndex<'a> {
    pr: u8,
    files: Vec<FileIndex<'a>>,
    new_paths: HashMap<&'a str, usize>,
    old_paths: HashMap<&'a str, usize>,
}

impl<'a> StepIndex<'a> {
    fn new(step: &Step<'a>) -> Self {
        let files: Vec<FileIndex> = step.files.iter().map(FileIndex::new).collect();
        let mut new_paths = HashMap::new();
        let mut old_paths = HashMap::new();
        for (i, f) in step.files.iter().enumerate() {
            if let Some(p) = &f.new_path {
                new_paths.insert(p.as_str(), i);
            }
            if let Some(p) = &f.old_path {
                old_paths.insert(p.as_str(), i);
            }
        }
        StepIndex { pr: step.pr, files, new_paths, old_paths }
    }

    fn by_new_path(&self, path: &str) -> Option<usize> {
        self.new_paths.get(path).copied()
    }

    fn by_old_path(&self, path: &str) -> Option<usize> {
        self.old_paths.get(path).copied()
    }

    fn touches(&self, file: &FileDiff) -> bool {
        file.new_path.as_deref().is_some_and(|p| self.new_paths.contains_key(p))
            || file.old_path.as_deref().is_some_and(|p| self.old_paths.contains_key(p))
    }
}

enum Back {
    Added { hunk: usize, line: usize },
    Old(u32),
}

enum Forward {
    Deleted,
    New(u32),
}

/// A step's file diff with O(1) lookup from a line number inside a hunk to the hunk line.
struct FileIndex<'a> {
    file: &'a FileDiff,
    /// Per hunk: hunk line index for each new-side (resp. old-side) line, by offset from start.
    by_new: Vec<Vec<usize>>,
    by_old: Vec<Vec<usize>>,
}

impl<'a> FileIndex<'a> {
    fn new(file: &'a FileDiff) -> Self {
        let mut by_new = Vec::with_capacity(file.hunks.len());
        let mut by_old = Vec::with_capacity(file.hunks.len());
        for hunk in &file.hunks {
            let mut new_lines = vec![usize::MAX; hunk.new_count as usize];
            let mut old_lines = vec![usize::MAX; hunk.old_count as usize];
            for (i, line) in hunk.lines.iter().enumerate() {
                if let Some(slot) = line.new_no.and_then(|n| new_lines.get_mut((n - hunk.new_start) as usize)) {
                    *slot = i;
                }
                if let Some(slot) = line.old_no.and_then(|o| old_lines.get_mut((o - hunk.old_start) as usize)) {
                    *slot = i;
                }
            }
            by_new.push(new_lines);
            by_old.push(old_lines);
        }
        FileIndex { file, by_new, by_old }
    }

    fn map_back(&self, n: u32) -> Back {
        if self.file.old_path.is_none() {
            // Whole file added in this step.
            return self.locate_new(n).unwrap_or(Back::Added { hunk: 0, line: 0 });
        }
        let mut delta: i64 = 0;
        for (index, hunk) in self.file.hunks.iter().enumerate() {
            let before = if hunk.new_count == 0 { n <= hunk.new_start } else { n < hunk.new_start };
            if before {
                break;
            }
            if n < hunk.new_start + hunk.new_count {
                let line = self.by_new[index][(n - hunk.new_start) as usize];
                return match hunk.lines.get(line) {
                    Some(l) if l.kind == LineKind::Added => Back::Added { hunk: index, line },
                    Some(l) => Back::Old(l.old_no.unwrap_or(n)),
                    None => Back::Old(n),
                };
            }
            delta += i64::from(hunk.old_count) - i64::from(hunk.new_count);
        }
        Back::Old((i64::from(n) + delta).max(1) as u32)
    }

    fn locate_new(&self, n: u32) -> Option<Back> {
        self.file.hunks.iter().enumerate().find_map(|(index, hunk)| {
            let offset = n.checked_sub(hunk.new_start)? as usize;
            let line = *self.by_new[index].get(offset)?;
            Some(Back::Added { hunk: index, line })
        })
    }

    fn map_forward(&self, o: u32) -> Forward {
        if self.file.new_path.is_none() {
            return Forward::Deleted;
        }
        let mut delta: i64 = 0;
        for (index, hunk) in self.file.hunks.iter().enumerate() {
            let before = if hunk.old_count == 0 { o <= hunk.old_start } else { o < hunk.old_start };
            if before {
                break;
            }
            if o < hunk.old_start + hunk.old_count {
                let line = self.by_old[index][(o - hunk.old_start) as usize];
                return match hunk.lines.get(line) {
                    Some(l) if l.kind == LineKind::Deleted => Forward::Deleted,
                    Some(l) => Forward::New(l.new_no.unwrap_or(o)),
                    None => Forward::New(o),
                };
            }
            delta += i64::from(hunk.new_count) - i64::from(hunk.old_count);
        }
        Forward::New((i64::from(o) + delta).max(1) as u32)
    }
}

/// Where line `o` (old side) of `file`'s diff is on the new side, if it wasn't deleted or changed.
pub(crate) fn map_line_forward(file: &FileDiff, o: u32) -> Option<u32> {
    match FileIndex::new(file).map_forward(o) {
        Forward::New(n) => Some(n),
        Forward::Deleted => None,
    }
}
