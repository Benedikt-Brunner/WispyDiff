use std::sync::Arc;

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::anchors::{remap_line, Coverage};
use crate::cache::Cache;
use crate::drafts::{now, Draft, DraftKind, DraftStatus};
use crate::github::{ReviewThread, Side, Verdict};
use crate::inbox::{group, InboxGroup};
use crate::progress::{Checkpoint, CheckpointEntry};
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

/// One `git grep` hit on the head of a range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// Whole-repo search stops after this many hits.
pub const MAX_GREP_HITS: usize = 1000;

/// How a whole-repo search went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrepSummary {
    pub total: usize,
    /// Files that couldn't be searched because their contents couldn't be downloaded (offline).
    pub unsearched: usize,
}

/// Starting a new assistant thread.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewThread {
    pub provider: crate::assistant::Provider,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub selection: Option<crate::assistant::Selection>,
    pub anchor: Option<crate::assistant::ThreadAnchor>,
}

/// How much of the range's diff goes into the first prompt.
const PROMPT_DIFF_LIMIT: usize = 120_000;

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
    /// member alone and the whole stack, downloads every head's files (for whole-repo search),
    /// and caches review threads.
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
        let git = self.git(&snapshot)?;
        for head in &snapshot.heads {
            self.store.hydrate(&git, head)?;
        }
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

    // ---------- code intelligence ----------

    /// The symbol index of a view (the head side of every file it touches).
    pub fn symbol_index(&self, snapshot: &StackSnapshot, view: &DiffView) -> Result<crate::symbols::SymbolIndex> {
        let git = self.git(snapshot)?;
        Ok(crate::symbols::SymbolIndex::build(view, |file| {
            let blob = file.new_blob.as_ref()?;
            blob_lines(&git, blob, Language::from_path(&file.path), &self.highlighter, &self.highlights).ok()
        }))
    }

    /// Streams `git grep` hits for `query` (fixed string) on the head of stack PR `index`,
    /// in batches to `on_hits` (return `false` to stop). The first search of a head downloads
    /// its missing blobs in one batch; if that fails (offline), the files already downloaded
    /// are searched and the rest are counted as unsearched.
    pub fn grep(&self, snapshot: &StackSnapshot, index: usize, query: &str, whole_word: bool, mut on_hits: impl FnMut(Vec<GrepHit>) -> bool) -> Result<GrepSummary> {
        use std::io::{BufRead, BufReader};
        let head = &snapshot.heads[index];
        let git = self.git(snapshot)?;
        let unsearched = match self.store.hydrate(&git, head) {
            Ok(_) => 0,
            Err(_) => self.store.missing_blobs(&git, head)?.len(),
        };
        let mut args = vec!["grep", "-z", "-n", "-I", "--fixed-strings", "--max-count=50"];
        if whole_word {
            args.push("-w");
        }
        args.extend(["-e", query, head.as_str(), "--"]);
        let mut child = git.spawn_local(&args)?;
        let reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let prefix = format!("{head}:");
        let (mut batch, mut total, mut last_flush) = (Vec::new(), 0usize, std::time::Instant::now());
        for line in reader.split(b'\n') {
            let line = line?;
            let mut fields = line.splitn(3, |b| *b == 0);
            let (Some(path), Some(number), Some(text)) = (fields.next(), fields.next(), fields.next()) else { continue };
            let path = String::from_utf8_lossy(path);
            batch.push(GrepHit {
                path: path.strip_prefix(&prefix).unwrap_or(&path).to_string(),
                line: String::from_utf8_lossy(number).parse().unwrap_or(0),
                text: String::from_utf8_lossy(text).trim().chars().take(200).collect(),
            });
            total += 1;
            // First hits go out immediately; later ones in batches.
            if total == 1 || batch.len() >= 50 || last_flush.elapsed().as_millis() > 40 {
                if !on_hits(std::mem::take(&mut batch)) {
                    break;
                }
                last_flush = std::time::Instant::now();
            }
            if total >= MAX_GREP_HITS {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        if !batch.is_empty() {
            on_hits(batch);
        }
        Ok(GrepSummary { total, unsearched })
    }

    /// The highlighted contents of `path` on the head of stack PR `index` (read-only file view).
    pub fn read_file(&self, snapshot: &StackSnapshot, index: usize, path: &str) -> Result<crate::highlight_cache::Lines> {
        let git = self.git(snapshot)?;
        let blob = git.run_string(&["rev-parse", &format!("{}:{path}", snapshot.heads[index])])?;
        blob_lines(&git, &blob, Language::from_path(path), &self.highlighter, &self.highlights)
    }

    // ---------- assistant ----------

    /// Assistant threads started on PRs of this stack.
    pub fn assistant_threads(&self, snapshot: &StackSnapshot) -> Result<Vec<crate::assistant::Thread>> {
        let numbers: Vec<u64> = snapshot.stack.prs.iter().map(|p| p.number).collect();
        Ok(self
            .cache
            .assistant_threads(snapshot.repo_slug())?
            .into_iter()
            .filter(|t| t.prs.iter().any(|n| numbers.contains(n)))
            .collect())
    }

    pub fn delete_assistant_thread(&self, id: &str) -> Result<()> {
        self.cache.delete_assistant_thread(id)
    }

    /// Asks a question: in `thread` (a follow-up, resuming its CLI session) or in a new thread
    /// about `lo..=hi` (and `new.selection`, if any). The CLI runs read-only in a checkout of
    /// the range head. `on_event` receives the streamed answer; the thread (with both messages)
    /// is stored either way. Blocking.
    pub fn ask(
        &self,
        snapshot: &StackSnapshot,
        (lo, hi): (usize, usize),
        thread: Option<&str>,
        new: Option<NewThread>,
        question: &str,
        on_event: impl FnMut(&crate::assistant::AssistantEvent),
    ) -> Result<crate::assistant::Thread> {
        use crate::assistant::{diff_text, first_prompt, run, worktree, Ask, Context, Message, Thread};
        let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
        let existing = match thread {
            Some(id) => Some(self.cache.assistant_thread(id)?.ok_or_else(|| crate::Error::Assistant("thread not found".into()))?),
            None => None,
        };
        let git = self.git(snapshot)?;
        let head = &snapshot.heads[hi];
        self.store.hydrate(&git, head)?;
        let (owner, repo) = snapshot.repo_slug().split_once('/').unwrap_or((snapshot.repo_slug(), ""));
        let cwd = worktree(&git, &self.store.worktrees_dir(owner, repo), head)?;

        let (mut thread, context) = match existing {
            Some(thread) => (thread, None),
            None => {
                let new = new.ok_or_else(|| crate::Error::Assistant("no thread to continue".into()))?;
                let context = Context {
                    range_label: range_label(snapshot, lo, hi),
                    titles: snapshot.stack.prs[lo..=hi].iter().map(|p| format!("#{} {}", p.number, p.title)).collect(),
                    diff: diff_text(&git, &snapshot.bases[lo], head, PROMPT_DIFF_LIMIT)?,
                    selection: new.selection,
                };
                let thread = Thread {
                    id: crate::drafts::new_id(),
                    repo: snapshot.repo_slug().to_string(),
                    prs: snapshot.stack.prs[lo..=hi].iter().map(|p| p.number).collect(),
                    provider: new.provider,
                    model: new.model,
                    effort: new.effort,
                    session: None,
                    selection: new.anchor,
                    messages: Vec::new(),
                    created_at: now(),
                };
                (thread, Some(context))
            }
        };
        // Follow-ups resume the CLI session, which already has the context.
        let prompt = match (&thread.session, context) {
            (None, Some(context)) => first_prompt(&context, question),
            _ => question.to_string(),
        };
        thread.messages.push(Message { role: "user".into(), text: question.to_string(), at: now(), error: false });

        let ask = Ask { provider: thread.provider, model: thread.model.clone(), effort: thread.effort.clone(), prompt, resume: thread.session.clone() };
        let mut session = None;
        let mut forward = on_event;
        let result = run(&ask, &cwd, |event| {
            if let crate::assistant::AssistantEvent::Session { id } = event {
                session = Some(id.clone());
            }
            forward(event);
        });
        if thread.session.is_none() {
            thread.session = session;
        }
        let (text, error) = match result {
            Ok(text) => (text, false),
            Err(err) => (err.to_string(), true),
        };
        thread.messages.push(Message { role: "assistant".into(), text, at: now(), error });
        self.cache.put_assistant_thread(&thread)?;
        Ok(thread)
    }

    // ---------- progress ----------

    pub fn viewed(&self, repo: &str) -> Result<Vec<String>> {
        self.cache.viewed_keys(repo)
    }

    pub fn set_viewed(&self, repo: &str, key: &str, viewed: bool) -> Result<()> {
        self.cache.set_viewed(repo, key, viewed)
    }

    /// Records the current heads of stack PRs `lo..=hi` as reviewed (local only).
    pub fn mark_reviewed(&self, snapshot: &StackSnapshot, lo: usize, hi: usize, source: &str) -> Result<Checkpoint> {
        let checkpoint = Checkpoint {
            id: crate::drafts::new_id(),
            repo: snapshot.repo_slug().to_string(),
            created_at: now(),
            source: source.to_string(),
            entries: (lo..=hi.min(snapshot.len() - 1))
                .map(|i| CheckpointEntry {
                    pr: snapshot.stack.prs[i].number,
                    head: snapshot.heads[i].clone(),
                    base: snapshot.bases[i].clone(),
                })
                .collect(),
        };
        self.cache.put_checkpoint(&checkpoint)?;
        Ok(checkpoint)
    }

    /// Checkpoints that cover the bottom and top PR of `lo..=hi`, newest first.
    pub fn checkpoints(&self, snapshot: &StackSnapshot, lo: usize, hi: usize) -> Result<Vec<Checkpoint>> {
        let (bottom, top) = (snapshot.stack.prs[lo].number, snapshot.stack.prs[hi].number);
        Ok(self
            .cache
            .checkpoints(snapshot.repo_slug())?
            .into_iter()
            .filter(|c| c.entry(bottom).is_some() && c.entry(top).is_some())
            .collect())
    }

    /// What changed in `lo..=hi` since `checkpoint`, ignoring changes a rebase brought in.
    /// Returns the view and whether replaying conflicted (see [`crate::progress::Interdiff`]).
    pub fn interdiff(&self, snapshot: &StackSnapshot, lo: usize, hi: usize, checkpoint: &Checkpoint) -> Result<(Arc<DiffView>, bool)> {
        let (bottom, top) = (snapshot.stack.prs[lo].number, snapshot.stack.prs[hi].number);
        let (Some(old_bottom), Some(old_top)) = (checkpoint.entry(bottom), checkpoint.entry(top)) else {
            return Err(crate::Error::GitHub("checkpoint doesn't cover this range".into()));
        };
        let git = self.git(snapshot)?;
        let (from, conflicts) = crate::progress::replayed_base(&git, &old_bottom.base, &old_top.head, &snapshot.bases[lo])?;
        let spec = crate::range::RangeSpec { from, heads: vec![snapshot.heads[hi].clone()], prs: vec![hi as u8], ignore_whitespace: false };
        if let Some(view) = self.cache.diff_view(snapshot.repo_slug(), &spec)? {
            return Ok((Arc::new(view), conflicts));
        }
        let view = compute_range(&git, &spec, &self.highlighter, &self.highlights)?;
        self.cache.put_diff_view(snapshot.repo_slug(), &spec, &view)?;
        Ok((Arc::new(view), conflicts))
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

fn range_label(snapshot: &StackSnapshot, lo: usize, hi: usize) -> String {
    let prs = &snapshot.stack.prs;
    if lo == hi {
        format!("{} #{}", snapshot.repo_slug(), prs[lo].number)
    } else {
        format!("{} #{}–#{} (a stack of {} PRs)", snapshot.repo_slug(), prs[lo].number, prs[hi].number, hi - lo + 1)
    }
}
