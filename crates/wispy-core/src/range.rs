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

/// A range `from..heads.last()`, decomposed into one step per PR: step `i` goes from the
/// previous head (or `from`) to `heads[i]` and belongs to stack index `prs[i]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeSpec {
    pub from: String,
    pub heads: Vec<String>,
    pub prs: Vec<u8>,
}

impl RangeSpec {
    pub fn to(&self) -> &str {
        self.heads.last().expect("a range has at least one PR")
    }

    /// Identifies the computed view; any SHA change produces a new key.
    pub fn cache_key(&self, repo_slug: &str) -> String {
        let steps: Vec<String> = self.prs.iter().zip(&self.heads).map(|(pr, head)| format!("{pr}={head}")).collect();
        format!("v{MODEL_VERSION}:{repo_slug}:{}..{}", self.from, steps.join(","))
    }
}

pub fn compute_range(git: &Git, spec: &RangeSpec, highlighter: &Highlighter, cache: &HighlightCache) -> Result<DiffView> {
    let diff = |from: &str, to: &str| -> Result<Vec<FileDiff>> {
        let mut args = PATCH_ARGS.to_vec();
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
    Ok(DiffView::build(&spec.from, spec.to(), &combined, &attributions, &highlights))
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
    for (blob, lines) in fresh {
        cache.insert(blob.clone(), wanted[&blob], lines.clone());
        highlights.insert(blob, lines);
    }
    Ok(highlights)
}
