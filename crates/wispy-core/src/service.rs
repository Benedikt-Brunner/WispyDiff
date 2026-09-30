use std::sync::Arc;

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::anchors::{remap_line, Coverage};
use crate::cache::Cache;
use crate::drafts::{now, Draft, DraftKind, DraftStatus};
use crate::github::{ReviewThread, Side, Verdict};
use crate::inbox::{group, InboxGroup};
use crate::review::{self, Outcome, Planned, Target};
use crate::error::Result;
use crate::github::GitHubClient;
use crate::highlight::Highlighter;
use crate::highlight_cache::HighlightCache;
use crate::highlight::Language;
use crate::model::{file_slice, DiffView};
use crate::pr_ref::PrRef;
use crate::range::{blob_lines, compute_range};
use crate::repo_store::RepoStore;
use crate::split::{align, split_rows, SplitRow};
use crate::stack::{discover_stack, Stack, StackSnapshot};

/// Opens stacks: structure from GitHub, code from the local blobless clone, and precomputed
/// range views from the cache whenever the SHAs are unchanged.
pub struct PrService {
    github: GitHubClient,
    store: RepoStore,
    cache: Arc<Cache>,
    highlighter: Highlighter,
    highlights: HighlightCache,
    coverage: Mutex<HashMap<String, Arc<Coverage>>>,
}

/// A pending draft of a stack PR, with its anchor moved to the PR's current head
/// (`None` when the lines changed since).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownDraft {
    pub draft: Draft,
    /// Stack index of the draft's PR.
    pub pr_index: u8,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub start_line: Option<u32>,
}

/// An inbox group as the home screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    pub group: InboxGroup,
    /// Its stack is prefetched at the listed heads: opening it needs no network.
    pub ready: bool,
    /// Unposted drafts on the group's PRs.
    pub drafts: usize,
}

const INBOX_KEY: &str = "inbox";

/// What a new draft is attached to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDraft {
    pub pr_index: usize,
    pub kind: DraftKind,
    pub path: Option<String>,
    pub side: Option<Side>,
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub body: String,
    pub thread_id: Option<String>,
    pub reply_to: Option<u64>,
}

impl PrService {
    pub fn new(github: GitHubClient, store: RepoStore, cache: Cache, highlighter: Highlighter) -> Self {
        let cache = Arc::new(cache);
        let highlights = HighlightCache::new(3_000_000, Some(cache.clone()));
        PrService { github, store, cache, highlighter, highlights, coverage: Mutex::new(HashMap::new()) }
    }

    /// The stack `pr` was last seen in, without touching the network.
    pub fn cached_stack(&self, pr: &PrRef) -> Result<Option<StackSnapshot>> {
        self.cache.stack(pr)
    }

    pub async fn discover(&self, pr: &PrRef) -> Result<Stack> {
        discover_stack(&self.github, pr).await
    }

    /// Fetches every PR of `stack` into the local clone and records the snapshot.
    /// Blocking: call from a worker thread.
    pub fn fetch(&self, stack: Stack) -> Result<StackSnapshot> {
        let bottom = &stack.prs[0];
        let (owner, repo) = bottom.base_repo.split_once('/').unwrap_or((&bottom.base_repo, ""));
        let git = self.store.ensure_repo(owner, repo, &bottom.clone_url)?;
        let points = self.store.fetch_stack(&git, &stack.prs)?;
        let snapshot = StackSnapshot::new(stack, points);
        self.cache.put_stack(&snapshot)?;
        Ok(snapshot)
    }

    /// The cached view of `lo..=hi`, if it was computed before (no git involved).
    pub fn cached_range(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<Option<DiffView>> {
        self.cache.diff_view(snapshot.repo_slug(), &snapshot.range(lo, hi))
    }

    /// The view of `lo..=hi`, computing and caching it if needed. Blocking.
    pub fn range(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<Arc<DiffView>> {
        self.range_with(snapshot, lo, hi, false)
    }

    /// Like [`PrService::range`], optionally ignoring whitespace-only changes.
    pub fn range_with(&self, snapshot: &StackSnapshot, lo: usize, hi: usize, ignore_whitespace: bool) -> Result<Arc<DiffView>> {
        let spec = snapshot.range(lo, hi).with_ignore_whitespace(ignore_whitespace);
        if let Some(view) = self.cache.diff_view(snapshot.repo_slug(), &spec)? {
            return Ok(Arc::new(view));
        }
        self.compute(snapshot, spec).map(Arc::new)
    }

    /// Computes `lo..=hi` into the cache unless it's already there. Returns whether it computed.
    pub fn precompute(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<bool> {
        let spec = snapshot.range(lo, hi);
        if self.cache.has_diff_view(snapshot.repo_slug(), &spec)? {
            return Ok(false);
        }
        self.compute(snapshot, spec)?;
        Ok(true)
    }

    /// The full-file side-by-side rows of file `index` in `view` (a view of `snapshot`),
    /// excluding the header. Empty when the file has no side-by-side (binary, rename-only).
    pub fn split_rows(&self, snapshot: &StackSnapshot, view: &DiffView, index: usize) -> Result<Vec<SplitRow>> {
        let file = &view.summary.files[index];
        if file.split_rows == 0 {
            return Ok(Vec::new());
        }
        let git = self.git(snapshot)?;
        let language = Language::from_path(&file.path);
        let lines = |blob: &Option<String>| -> Result<crate::highlight_cache::Lines> {
            match blob {
                Some(blob) => blob_lines(&git, blob, language, &self.highlighter, &self.highlights),
                None => Ok(Arc::new(Vec::new())),
            }
        };
        let (old, new) = (lines(&file.old_blob)?, lines(&file.new_blob)?);
        let rows = file_slice(view, index);
        let alignment = align(rows, old.len() as u32, new.len() as u32);
        Ok(split_rows(rows, &alignment.pairs, &old, &new))
    }

    /// Glob patterns of files to collapse in this repo (set by the user).
    pub fn ignore_patterns(&self, repo_slug: &str) -> Result<Vec<String>> {
        let settings = self.cache.repo_settings(repo_slug)?.unwrap_or_default();
        Ok(settings["ignore"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect())
    }

    pub fn set_ignore_patterns(&self, repo_slug: &str, patterns: &[String]) -> Result<()> {
        let mut settings = self.cache.repo_settings(repo_slug)?.unwrap_or_else(|| serde_json::json!({}));
        settings["ignore"] = serde_json::json!(patterns);
        self.cache.put_repo_settings(repo_slug, &settings)
    }

    // ---------- drafts ----------

    /// Stores a new draft for stack PR `pr_index`, anchored to that PR's current head.
    pub fn create_draft(&self, snapshot: &StackSnapshot, new: NewDraft) -> Result<Draft> {
        let now = now();
        let draft = Draft {
            id: crate::drafts::new_id(),
            repo: snapshot.repo_slug().to_string(),
            pr: snapshot.stack.prs[new.pr_index].number,
            kind: new.kind,
            path: new.path,
            side: new.side,
            line: new.line,
            start_line: new.start_line.filter(|s| Some(*s) != new.line),
            commit: snapshot.heads[new.pr_index].clone(),
            base: snapshot.bases[new.pr_index].clone(),
            body: new.body,
            thread_id: new.thread_id,
            reply_to: new.reply_to,
            as_file: false,
            status: DraftStatus::Draft,
            error: None,
            token: None,
            created_at: now,
            updated_at: now,
        };
        self.cache.put_draft(&draft)?;
        Ok(draft)
    }

    /// Changes a draft's body and/or whether an outdated line draft goes out as a file comment.
    /// A failed draft becomes a plain draft again (it'll be retried).
    pub fn update_draft(&self, id: &str, body: Option<String>, as_file: Option<bool>) -> Result<Option<Draft>> {
        let Some(mut draft) = self.cache.draft(id)? else { return Ok(None) };
        if let Some(body) = body {
            draft.body = body;
        }
        if let Some(as_file) = as_file {
            draft.as_file = as_file;
        }
        if draft.status == DraftStatus::Failed {
            draft.status = DraftStatus::Draft;
            draft.error = None;
        }
        draft.updated_at = now();
        self.cache.put_draft(&draft)?;
        Ok(Some(draft))
    }

    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.cache.delete_draft(id)
    }

    /// Pending drafts of the stack, each moved to its PR's current head.
    pub fn shown_drafts(&self, snapshot: &StackSnapshot) -> Result<Vec<ShownDraft>> {
        let numbers: Vec<u64> = snapshot.stack.prs.iter().map(|p| p.number).collect();
        let drafts = self.cache.drafts(snapshot.repo_slug(), &numbers)?;
        let git = self.git(snapshot)?;
        let mut shown = Vec::new();
        for draft in drafts.into_iter().filter(Draft::is_pending) {
            let Some(index) = numbers.iter().position(|n| *n == draft.pr) else { continue };
            let (mut path, mut line, mut start_line) = (draft.path.clone(), None, None);
            if let (DraftKind::Line, Some(p), Some(side), Some(end)) = (draft.kind, &draft.path, draft.side, draft.line) {
                let (from, to) = match side {
                    Side::Right => (&draft.commit, &snapshot.heads[index]),
                    Side::Left => (&draft.base, &snapshot.bases[index]),
                };
                if let Some((new_path, new_end)) = remap_line(&git, from, to, p, end)? {
                    let start = match draft.start_line {
                        Some(s) => remap_line(&git, from, to, p, s)?.map(|(_, s)| s),
                        None => None,
                    };
                    path = Some(new_path);
                    line = Some(new_end);
                    start_line = start;
                }
            }
            shown.push(ShownDraft { draft, pr_index: index as u8, path, line, start_line });
        }
        Ok(shown)
    }

    /// Whether GitHub would take a line comment on lines `start..=end` of stack PR `index`
    /// (otherwise it goes out as a file comment).
    pub fn accepts_line_comment(&self, snapshot: &StackSnapshot, index: usize, path: &str, side: Side, start: u32, end: u32) -> Result<bool> {
        Ok(self.pr_coverage(snapshot, index)?.accepts(path, side, start, end))
    }

    fn pr_coverage(&self, snapshot: &StackSnapshot, index: usize) -> Result<Arc<Coverage>> {
        let key = format!("{}:{}..{}", snapshot.repo_slug(), snapshot.bases[index], snapshot.heads[index]);
        if let Some(coverage) = self.coverage.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(coverage.clone());
        }
        let coverage = Arc::new(Coverage::compute(&self.git(snapshot)?, &snapshot.bases[index], &snapshot.heads[index])?);
        self.coverage.lock().unwrap_or_else(|p| p.into_inner()).insert(key, coverage.clone());
        Ok(coverage)
    }

    // ---------- threads ----------

    pub fn cached_threads(&self, snapshot: &StackSnapshot, index: usize) -> Result<Vec<ReviewThread>> {
        Ok(self.cache.threads(snapshot.repo_slug(), snapshot.stack.prs[index].number)?.unwrap_or_default())
    }

    /// Fetches every stack PR's review threads into the cache.
    pub async fn refresh_threads(&self, snapshot: &StackSnapshot) -> Result<()> {
        for index in 0..snapshot.len() {
            let pr = snapshot.pr_ref(index);
            let threads = self.github.review_threads(&pr).await?;
            self.cache.put_threads(snapshot.repo_slug(), pr.number, &threads)?;
        }
        Ok(())
    }

    // ---------- submitting ----------

    /// Settles drafts of stack PR `index` whose previous submit had an unknown outcome.
    pub async fn reconcile(&self, snapshot: &StackSnapshot, index: usize) -> Result<()> {
        let pr = snapshot.pr_ref(index);
        let drafts = self.cache.drafts(snapshot.repo_slug(), &[pr.number])?;
        review::reconcile(&self.github, &self.cache, &pr, &drafts).await
    }

    /// How each pending draft of stack PR `index` would be posted. `snapshot` must be fresh.
    pub fn plan(&self, snapshot: &StackSnapshot, index: usize) -> Result<Vec<Planned>> {
        let pr = snapshot.pr_ref(index);
        let drafts = self.cache.drafts(snapshot.repo_slug(), &[pr.number])?;
        let coverage = self.pr_coverage(snapshot, index)?;
        let target = Target { pr: &pr, head: &snapshot.heads[index], base: &snapshot.bases[index], coverage: &coverage };
        review::plan(&self.git(snapshot)?, &target, &drafts)
    }

    /// Posts one review for stack PR `index` from its pending drafts. `snapshot` must be fresh.
    pub async fn submit(&self, snapshot: &StackSnapshot, index: usize, verdict: Verdict, summary: Option<String>) -> Result<Outcome> {
        let pr = snapshot.pr_ref(index);
        let coverage = self.pr_coverage(snapshot, index)?;
        let target = Target { pr: &pr, head: &snapshot.heads[index], base: &snapshot.bases[index], coverage: &coverage };
        let payload = {
            let git = self.git(snapshot)?;
            let drafts = self.cache.drafts(snapshot.repo_slug(), &[pr.number])?;
            let planned = review::plan(&git, &target, &drafts)?;
            review::payload(&git, &target, &planned, summary.as_deref())?
        };
        let outcome = review::send(&self.github, &self.cache, &target, payload, verdict).await?;
        if let Ok(threads) = self.github.review_threads(&pr).await {
            self.cache.put_threads(snapshot.repo_slug(), pr.number, &threads)?;
        }
        Ok(outcome)
    }

    // ---------- inbox & prefetch ----------

    /// The last inbox fetched, for instant (and offline) display.
    pub fn cached_inbox(&self) -> Result<Vec<InboxGroup>> {
        Ok(self.cache.app_value(INBOX_KEY)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
    }

    pub async fn refresh_inbox(&self) -> Result<Vec<InboxGroup>> {
        let groups = group(self.github.inbox().await?);
        self.cache.put_app_value(INBOX_KEY, &serde_json::to_value(&groups).map_err(crate::Error::codec)?)?;
        Ok(groups)
    }

    pub fn inbox_entries(&self, groups: Vec<InboxGroup>) -> Result<Vec<InboxEntry>> {
        groups
            .into_iter()
            .map(|group| {
                let numbers: Vec<u64> = group.prs.iter().map(|p| p.number).collect();
                let drafts = self.cache.drafts(&group.repo, &numbers)?.iter().filter(|d| d.is_pending()).count();
                let ready = group.prs.iter().all(|pr| {
                    let reference = PrRef { owner: owner_of(&group.repo).into(), repo: name_of(&group.repo).into(), number: pr.number };
                    matches!(self.cache.stack(&reference), Ok(Some(s)) if s.heads.contains(&pr.head_sha))
                });
                Ok(InboxEntry { group, ready, drafts })
            })
            .collect()
    }

    /// Makes a group openable offline: discovers and fetches its full stack, precomputes each
    /// member alone and the whole stack, and caches review threads.
    pub async fn prefetch(&self, group: &InboxGroup) -> Result<StackSnapshot> {
        let pr = PrRef { owner: owner_of(&group.repo).into(), repo: name_of(&group.repo).into(), number: group.prs[0].number };
        let stack = self.discover(&pr).await?;
        let snapshot = self.fetch(stack)?;
        let members: Vec<usize> = group
            .prs
            .iter()
            .filter_map(|member| snapshot.stack.prs.iter().position(|p| p.number == member.number))
            .collect();
        for &index in &members {
            self.precompute(&snapshot, index, index)?;
        }
        self.precompute(&snapshot, 0, snapshot.len() - 1)?;
        self.refresh_threads(&snapshot).await?;
        Ok(snapshot)
    }

    /// Drops cached stacks of PRs that left the inbox because they were merged or closed,
    /// unless they still have unposted drafts. Returns how many PRs were evicted.
    pub async fn evict_finished(&self, inbox: &[InboxGroup]) -> Result<usize> {
        let listed: std::collections::HashSet<String> =
            inbox.iter().flat_map(|g| g.prs.iter().map(move |p| format!("{}#{}", g.repo, p.number))).collect();
        let mut evicted = 0;
        for key in self.cache.cached_prs()? {
            if listed.contains(&key) {
                continue;
            }
            let Ok(pr) = PrRef::parse(&key) else { continue };
            if self.cache.has_pending_drafts(&pr.repo_slug(), &[pr.number])? {
                continue;
            }
            let state = match self.github.pull_request(&pr).await {
                Ok(pull) => pull.state,
                Err(_) => continue,
            };
            if state == "open" {
                continue;
            }
            let heads = self.cache.stack(&pr)?.map(|s| s.heads).unwrap_or_default();
            self.cache.evict(&pr, &heads)?;
            evicted += 1;
        }
        Ok(evicted)
    }

    fn git(&self, snapshot: &StackSnapshot) -> Result<crate::git::Git> {
        let (owner, repo) = snapshot.repo_slug().split_once('/').unwrap_or((snapshot.repo_slug(), ""));
        self.store.ensure_repo(owner, repo, &snapshot.stack.prs[0].clone_url)
    }

    fn compute(&self, snapshot: &StackSnapshot, spec: crate::range::RangeSpec) -> Result<DiffView> {
        let git = self.git(snapshot)?;
        let view = compute_range(&git, &spec, &self.highlighter, &self.highlights)?;
        self.cache.put_diff_view(snapshot.repo_slug(), &spec, &view)?;
        Ok(view)
    }
}

fn owner_of(slug: &str) -> &str {
    slug.split_once('/').map(|(o, _)| o).unwrap_or(slug)
}

fn name_of(slug: &str) -> &str {
    slug.split_once('/').map(|(_, r)| r).unwrap_or("")
}
