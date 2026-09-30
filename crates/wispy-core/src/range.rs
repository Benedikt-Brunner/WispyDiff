//! Computing the diff view for a contiguous range of PRs in a stack.

use std::collections::HashMap;
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::attribution::{attribute, Step};
use crate::diff::{parse_patch, FileDiff, LineKind, PATCH_ARGS};
use crate::error::Result;
use crate::git::Git;
use crate::highlight::{Highlighter, Language};
use crate::highlight_cache::{HighlightCache, Lines};
use crate::model::{read_blobs, DiffView, MODEL_VERSION};
use crate::noise::generated_paths;

/// A range `from..heads.last()`, decomposed into one step per PR: step `i` goes from the
/// previous head (or `from`) to `heads[i]` and belongs to stack index `prs[i]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeSpec {
    pub from: String,
    pub heads: Vec<String>,
    pub prs: Vec<u8>,
    /// Diff with `-w`: whitespace-only changes disappear.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

impl RangeSpec {
    pub fn to(&self) -> &str {
        self.heads.last().expect("a range has at least one PR")
    }

    /// Identifies the computed view; any SHA change produces a new key.
    pub fn cache_key(&self, repo_slug: &str) -> String {
        let steps: Vec<String> = self.prs.iter().zip(&self.heads).map(|(pr, head)| format!("{pr}={head}")).collect();
        let whitespace = if self.ignore_whitespace { ":w" } else { "" };
        format!("v{MODEL_VERSION}:{repo_slug}:{}..{}{whitespace}", self.from, steps.join(","))
    }

    pub fn with_ignore_whitespace(mut self, ignore: bool) -> Self {
        self.ignore_whitespace = ignore;
        self
    }
}

pub fn compute_range(git: &Git, spec: &RangeSpec, highlighter: &Highlighter, cache: &HighlightCache) -> Result<DiffView> {
    let diff = |from: &str, to: &str| -> Result<Vec<FileDiff>> {
        let mut args = PATCH_ARGS.to_vec();
        if spec.ignore_whitespace {
            args.push("-w");
        }
        args.extend([from, to, "--"]);
        parse_patch(&git.run(&args)?)
    };

    // The combined diff plus, for multi-PR ranges, one diff per step — all independent.
    let mut endpoints = vec![(spec.from.as_str(), spec.to())];
    if spec.heads.len() > 1 {
        let starts = std::iter::once(spec.from.as_str()).chain(spec.heads.iter().map(String::as_str));
        endpoints.extend(starts.zip(&spec.heads).map(|(from, to)| (from, to.as_str())));
    }
    let mut diffs = endpoints.par_iter().map(|(from, to)| diff(from, to)).collect::<Result<Vec<_>>>()?.into_iter();
    let combined = diffs.next().expect("combined diff");
    let step_files: Vec<Vec<FileDiff>> = diffs.collect();

    let steps: Vec<Step> = if step_files.is_empty() {
        vec![Step { pr: spec.prs[0], files: &combined }]
    } else {
        spec.prs.iter().zip(&step_files).map(|(&pr, files)| Step { pr, files }).collect()
    };
    let attributions = attribute(&combined, &steps);
    let highlights = highlight_blobs(git, &combined, highlighter, cache)?;
    let paths: Vec<&str> = combined.iter().filter(|f| f.new_path.is_some()).map(FileDiff::path).collect();
    let generated = generated_paths(git, spec.to(), &paths)?;
    Ok(DiffView::build(&spec.from, spec.to(), &combined, &attributions, &highlights, &generated))
}

/// Highlighted full contents of one blob (e.g. the other side of a file for side-by-side),
/// from the cache or by reading and highlighting it.
pub fn blob_lines(git: &Git, blob: &str, language: Option<Language>, highlighter: &Highlighter, cache: &HighlightCache) -> Result<Lines> {
    if let Some(lines) = cache.get(blob, language) {
        return Ok(lines);
    }
    let content = read_blobs(git, &[blob])?.remove(blob).unwrap_or_default();
    let lines: Lines = Arc::new(highlighter.highlight(language, &String::from_utf8_lossy(&content)));
    cache.insert_many(vec![(blob.to_string(), language, lines.clone())]);
    Ok(lines)
}

/// Highlighted full contents for every blob the rows need, reusing and filling `cache`.
fn highlight_blobs(
    git: &Git,
    files: &[FileDiff],
    highlighter: &Highlighter,
    cache: &HighlightCache,
) -> Result<HashMap<String, Lines>> {
    let has = |f: &FileDiff, kind: LineKind| f.hunks.iter().flat_map(|h| &h.lines).any(|l| l.kind == kind);
    let mut wanted: HashMap<String, Option<Language>> = HashMap::new();
    for file in files.iter().filter(|f| !f.binary && !f.hunks.is_empty()) {
        let language = Language::from_path(file.path());
        if let Some(old) = file.old_blob.as_ref().filter(|_| has(file, LineKind::Deleted)) {
            wanted.insert(old.clone(), language);
        }
        if let Some(new) = file.new_blob.as_ref().filter(|_| has(file, LineKind::Added) || has(file, LineKind::Context)) {
            wanted.insert(new.clone(), language);
        }
    }

    let mut highlights = HashMap::with_capacity(wanted.len());
    let mut missing = Vec::new();
    for (blob, language) in &wanted {
        match cache.get(blob, *language) {
            Some(lines) => {
                highlights.insert(blob.clone(), lines);
            }
            None => missing.push(blob.as_str()),
        }
    }

    let contents = read_blobs(git, &missing)?;
    let fresh: Vec<(String, Lines)> = contents
        .par_iter()
        .map(|(blob, content)| {
            let lines = highlighter.highlight(wanted[blob], &String::from_utf8_lossy(content));
            (blob.clone(), Arc::new(lines))
        })
        .collect();
    cache.insert_many(fresh.iter().map(|(blob, lines)| (blob.clone(), wanted[blob], lines.clone())).collect());
    highlights.extend(fresh);
    Ok(highlights)
}
