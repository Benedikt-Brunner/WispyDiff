//! Parser for `git diff --full-index -M` patch output.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Arguments that make `git diff` output match what [`parse_patch`] expects.
pub const PATCH_ARGS: &[&str] =
    &["diff", "--no-color", "--no-ext-diff", "--full-index", "-M", "-U3", "--src-prefix=a/", "--dst-prefix=b/"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub section: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    pub fn header(&self) -> String {
        let mut header =
            format!("@@ -{},{} +{},{} @@", self.old_start, self.old_count, self.new_start, self.new_count);
        if !self.section.is_empty() {
            header.push(' ');
            header.push_str(&self.section);
        }
        header
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    pub fn path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or_default()
    }

    pub fn additions(&self) -> u32 {
        self.count(LineKind::Added)
    }

    pub fn deletions(&self) -> u32 {
        self.count(LineKind::Deleted)
    }

    fn count(&self, kind: LineKind) -> u32 {
        self.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == kind).count() as u32
    }
}

pub fn parse_patch(input: &[u8]) -> Result<Vec<FileDiff>> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut hunk_state: Option<HunkState> = None;

    for raw_line in split_lines(input) {
        let line = String::from_utf8_lossy(raw_line);

        if let Some(state) = hunk_state.as_mut() {
            if state.old_remaining > 0 || state.new_remaining > 0 {
                let file = files.last_mut().expect("hunk belongs to a file");
                let hunk = file.hunks.last_mut().expect("hunk exists");
                state.push(hunk, &line)?;
                continue;
            }
            if line.starts_with('\\') {
                // "\ No newline at end of file"
                continue;
            }
            hunk_state = None;
        }

        if let Some(rest) = line.strip_prefix("diff --git ") {
            let (old_path, new_path) = split_header_paths(rest);
            files.push(FileDiff {
                old_path,
                new_path,
                status: FileStatus::Modified,
                old_blob: None,
                new_blob: None,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }

        let Some(file) = files.last_mut() else {
            continue;
        };

        if line.starts_with("@@ ") {
            let hunk = parse_hunk_header(&line)?;
            hunk_state = Some(HunkState {
                old_no: hunk.old_start,
                new_no: hunk.new_start,
                old_remaining: hunk.old_count,
                new_remaining: hunk.new_count,
            });
            file.hunks.push(hunk);
        } else if line.starts_with("new file mode") {
            file.status = FileStatus::Added;
            file.old_path = None;
        } else if line.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
            file.new_path = None;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(unquote(path));
        } else if let Some(path) = line.strip_prefix("rename to ") {
            file.status = FileStatus::Renamed;
            file.new_path = Some(unquote(path));
        } else if let Some(path) = line.strip_prefix("copy from ") {
            file.status = FileStatus::Copied;
            file.old_path = Some(unquote(path));
        } else if let Some(path) = line.strip_prefix("copy to ") {
            file.status = FileStatus::Copied;
            file.new_path = Some(unquote(path));
        } else if let Some(rest) = line.strip_prefix("index ") {
            let range = rest.split(' ').next().unwrap_or_default();
            if let Some((old, new)) = range.split_once("..") {
                file.old_blob = non_null_blob(old);
                file.new_blob = non_null_blob(new);
            }
        } else if let Some(path) = line.strip_prefix("--- ") {
            if path != "/dev/null" {
                file.old_path = Some(strip_prefix_dir(&unquote(path), "a/"));
            }
        } else if let Some(path) = line.strip_prefix("+++ ") {
            if path != "/dev/null" {
                file.new_path = Some(strip_prefix_dir(&unquote(path), "b/"));
            }
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            file.binary = true;
        }
    }

    if let Some(state) = hunk_state {
        if state.old_remaining > 0 || state.new_remaining > 0 {
            return Err(Error::DiffParse("patch ended in the middle of a hunk".into()));
        }
    }
    Ok(files)
}

struct HunkState {
    old_no: u32,
    new_no: u32,
    old_remaining: u32,
    new_remaining: u32,
}

impl HunkState {
    fn push(&mut self, hunk: &mut Hunk, line: &str) -> Result<()> {
        let (kind, text) = match line.as_bytes().first() {
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Deleted, &line[1..]),
            Some(b'\\') => return Ok(()),
            // Some tools strip the space from empty context lines.
            None => (LineKind::Context, ""),
            Some(_) => return Err(Error::DiffParse(format!("unexpected line inside hunk: {line:?}"))),
        };
        let (old_no, new_no) = match kind {
            LineKind::Context => {
                self.take_old()?;
                self.take_new()?;
                (Some(self.old_no - 1), Some(self.new_no - 1))
            }
            LineKind::Added => {
                self.take_new()?;
                (None, Some(self.new_no - 1))
            }
            LineKind::Deleted => {
                self.take_old()?;
                (Some(self.old_no - 1), None)
            }
        };
        hunk.lines.push(DiffLine { kind, old_no, new_no, text: text.to_string() });
        Ok(())
    }

    fn take_old(&mut self) -> Result<()> {
        self.old_remaining =
            self.old_remaining.checked_sub(1).ok_or_else(|| Error::DiffParse("hunk has too many old lines".into()))?;
        self.old_no += 1;
        Ok(())
    }

    fn take_new(&mut self) -> Result<()> {
        self.new_remaining =
            self.new_remaining.checked_sub(1).ok_or_else(|| Error::DiffParse("hunk has too many new lines".into()))?;
        self.new_no += 1;
        Ok(())
    }
}

fn split_lines(input: &[u8]) -> impl Iterator<Item = &[u8]> {
    let input = input.strip_suffix(b"\n").unwrap_or(input);
    input.split(|b| *b == b'\n').filter(move |_| !input.is_empty())
}

fn parse_hunk_header(line: &str) -> Result<Hunk> {
    let invalid = || Error::DiffParse(format!("bad hunk header: {line:?}"));
    let body = line.strip_prefix("@@ -").ok_or_else(invalid)?;
    let (ranges, section) = body.split_once(" @@").ok_or_else(invalid)?;
    let (old, new) = ranges.split_once(" +").ok_or_else(invalid)?;
    let (old_start, old_count) = parse_range(old).ok_or_else(invalid)?;
    let (new_start, new_count) = parse_range(new).ok_or_else(invalid)?;
    Ok(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.trim_start().to_string(),
        lines: Vec::new(),
    })
}

fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Best-effort paths from `diff --git a/x b/y`; later header lines override these.
fn split_header_paths(rest: &str) -> (Option<String>, Option<String>) {
    if rest.starts_with('"') {
        if let Some(end) = find_closing_quote(rest) {
            let old = unquote(&rest[..=end]);
            let new = unquote(rest[end + 1..].trim_start());
            return (Some(strip_prefix_dir(&old, "a/")), Some(strip_prefix_dir(&new, "b/")));
        }
    }
    // Unquoted and unrenamed: "a/<p> b/<p>", both halves equal.
    let bytes = rest.as_bytes();
    if bytes.len() >= 5 && bytes.len() % 2 == 1 {
        let mid = bytes.len() / 2;
        let (old, new) = (&rest[..mid], &rest[mid + 1..]);
        if bytes[mid] == b' ' && old.strip_prefix("a/") == new.strip_prefix("b/") && old.starts_with("a/") {
            let path = old[2..].to_string();
            return (Some(path.clone()), Some(path));
        }
    }
    (None, None)
}

fn find_closing_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn strip_prefix_dir(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

fn non_null_blob(sha: &str) -> Option<String> {
    (!sha.is_empty() && !sha.bytes().all(|b| b == b'0')).then(|| sha.to_string())
}

/// Undoes git's C-style path quoting (`"a/caf\303\251.txt"`).
fn unquote(path: &str) -> String {
    let path = path.trim_end_matches('\t');
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return path.to_string();
    };
    let mut out = Vec::with_capacity(inner.len());
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let escaped = bytes[i + 1];
        i += 2;
        match escaped {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'v' => out.push(0x0b),
            b'0'..=b'7' => {
                let mut value = u32::from(escaped - b'0');
                for _ in 0..2 {
                    if i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                        value = value * 8 + u32::from(bytes[i] - b'0');
                        i += 1;
                    }
                }
                out.push(value as u8);
            }
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
