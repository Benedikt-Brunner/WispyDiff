//! UI-independent stack loading, prepared views, and background precomputation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::prefetch::PrefetchTrigger;
use crate::prefs::{Lasting, Prefs};
use tokio::runtime::Runtime;
use wispy_core::anchors::{locate, Anchor, Location};
use wispy_core::assistant::{AssistantEvent, Provider, Thread};
use wispy_core::cache::Cache;
use wispy_core::drafts::Draft;
use wispy_core::github::{GitHubClient, PullRequest, ReviewThread, DEFAULT_API_BASE};
use wispy_core::github::{Side, Verdict};
use wispy_core::highlight::Highlighter;
use wispy_core::model::DiffView;
use wispy_core::pr_ref::PrRef;
use wispy_core::progress::Checkpoint;
use wispy_core::repo_store::RepoStore;
use wispy_core::review::{Outcome, Planned};
use wispy_core::service::PrService;
use wispy_core::service::{NewDraft, ShownDraft};
use wispy_core::split::SplitRow;
use wispy_core::stack::StackSnapshot;
use wispy_core::symbols::SymbolIndex;
use wispy_core::token::resolve_github_token;

/// Diff views kept in memory; others are re-read from SQLite (~25 ms for the worst case).
const MAX_OPEN_VIEWS: usize = 4;
/// Side-by-side files kept in memory.
const MAX_SPLIT_FILES: usize = 32;

#[cfg(test)]
pub(crate) static TEST_ENV: Mutex<()> = Mutex::new(());

type RangeReadyCallback = Arc<dyn Fn(RangeReady) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub lo: usize,
    pub hi: usize,
}

#[derive(Clone)]
pub struct OpenedStack {
    pub stack_id: String,
    pub snapshot: Arc<StackSnapshot>,
    /// Bottom to top.
    pub prs: Vec<PullRequest>,
    pub needs_rebase: Vec<bool>,
    pub focus: usize,
    pub range: Range,
    pub view_id: String,
    pub view: Arc<DiffView>,
    pub from_cache: bool,
    pub ignore_whitespace: bool,
}

#[derive(Clone)]
pub struct Since {
    pub checkpoint_id: String,
    pub created_at: i64,
    /// Replaying the old version onto the new base conflicted: a raw diff is shown.
    pub conflicts: bool,
    /// Each file's content key in the range's full diff, by path: marking a file viewed
    /// here marks it viewed there too.
    pub full_keys: HashMap<String, String>,
}

#[derive(Clone)]
pub struct PrThreads {
    pub pr_index: usize,
    pub threads: Vec<ReviewThread>,
}

pub struct SubmitPlan {
    /// The stack as it is on GitHub right now (drafts are planned against it).
    pub stack: OpenedStack,
    pub prs: Vec<PrPlan>,
    /// The signed-in user: GitHub only lets them comment on their own PRs.
    pub viewer: String,
}

pub struct PrPlan {
    pub pr_index: usize,
    pub planned: Vec<Planned>,
}

#[derive(Clone)]
pub enum GrepEvent {
    Hits {
        hits: Vec<wispy_core::service::GrepHit>,
    },
    Done {
        total: usize,
        unsearched: usize,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone)]
pub struct OpenedRange {
    pub view_id: String,
    pub range: Range,
    pub ignore_whitespace: bool,
    /// Set when this is "changes since checkpoint" rather than the range's full diff.
    pub since: Option<Since>,
    pub view: Arc<DiffView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeReady {
    pub stack_id: String,
    pub lo: usize,
    pub hi: usize,
}

/// `WISPY_DATA_DIR` overrides the location (tests); default `~/Library/Application Support/WispyDiff`
/// on macOS, `$XDG_DATA_HOME/WispyDiff` (usually `~/.local/share/WispyDiff`) on Linux.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WISPY_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()));
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/WispyDiff")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share"))
            .join("WispyDiff")
    }
}

/// Identifies a stack at specific SHAs; any push produces a new id.
pub fn stack_id(snapshot: &StackSnapshot) -> String {
    let prs: Vec<String> = snapshot
        .stack
        .prs
        .iter()
        .zip(&snapshot.heads)
        .map(|(pr, head)| format!("{}@{}", pr.number, &head[..12.min(head.len())]))
        .collect();
    format!("{}:{}", snapshot.repo_slug(), prs.join(","))
}

pub fn view_id(stack_id: &str, lo: usize, hi: usize, ignore_whitespace: bool) -> String {
    format!(
        "{stack_id}:{lo}-{hi}{}",
        if ignore_whitespace { ":w" } else { "" }
    )
}

/// A review thread of stack PR `pr`, and where it shows up in a view (none: outdated, or on
/// lines the view doesn't show).
#[derive(Clone)]
pub struct PlacedThread {
    pub pr: usize,
    pub thread: ReviewThread,
    pub location: Option<Location>,
}

struct OpenView {
    id: String,
    stack_id: String,
    /// Stack index of the range's top PR.
    top: u8,
    view: Arc<DiffView>,
}

pub struct Backend {
    /// GitHub API base and token, when given explicitly (else from the environment / `gh`).
    github: Option<(String, String)>,
    // Taken on drop so the last Arc may safely be released on a runtime worker.
    runtime: Option<Runtime>,
    data_dir: PathBuf,
    service: OnceLock<Result<Arc<PrService>, String>>,
    stacks: Mutex<HashMap<String, Arc<StackSnapshot>>>,
    views: Mutex<VecDeque<OpenView>>,
    split_files: Mutex<VecDeque<(String, Arc<Vec<SplitRow>>)>>,
    /// The stack whose ranges the background worker should be precomputing.
    current_stack: Mutex<Option<String>>,
    /// Stacks with a precompute worker running.
    precomputing: Mutex<HashSet<String>>,
    /// Symbol index per view, built once (concurrent callers wait for the same build).
    indexes: Mutex<HashMap<String, Arc<OnceLock<Arc<SymbolIndex>>>>>,
    on_range_ready: Mutex<Option<RangeReadyCallback>>,
    prefs: Prefs,
    lasting: Lasting,
    prefetch: OnceLock<PrefetchTrigger>,
    on_inbox: Mutex<Option<Arc<dyn Fn(Vec<wispy_core::service::InboxEntry>) + Send + Sync>>>,
    on_inbox_status: Mutex<Option<Arc<dyn Fn(bool) + Send + Sync>>>,
}

impl Backend {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        Self::build(data_dir, None)
    }

    /// A backend talking to the GitHub API at `api_base` with `token` (tests: a fake server per
    /// test, without the process-wide `WISPY_GITHUB_*` variables).
    pub fn with_github(data_dir: PathBuf, api_base: String, token: String) -> Arc<Self> {
        Self::build(data_dir, Some((api_base, token)))
    }

    fn build(data_dir: PathBuf, github: Option<(String, String)>) -> Arc<Self> {
        Arc::new(Self {
            github,
            runtime: Some(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("create backend runtime"),
            ),
            prefs: Prefs::load(&data_dir),
            lasting: Lasting::default(),
            prefetch: OnceLock::new(),
            on_inbox: Mutex::new(None),
            on_inbox_status: Mutex::new(None),
            data_dir,
            service: OnceLock::new(),
            stacks: Mutex::new(HashMap::new()),
            views: Mutex::new(VecDeque::new()),
            split_files: Mutex::new(VecDeque::new()),
            current_stack: Mutex::new(None),
            precomputing: Mutex::new(HashSet::new()),
            indexes: Mutex::new(HashMap::new()),
            on_range_ready: Mutex::new(None),
        })
    }

    /// The runtime's handle (the UI's HTTP client runs its requests on it).
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime().handle().clone()
    }

    fn runtime(&self) -> &Runtime {
        self.runtime.as_ref().expect("backend runtime")
    }

    /// Created lazily so a missing `gh` login is reported on first use instead of at launch.
    pub fn service(&self) -> Result<Arc<PrService>, String> {
        self.service
            .get_or_init(|| self.create_service().map(Arc::new))
            .clone()
    }

    fn create_service(&self) -> Result<PrService, String> {
        let (api_base, token) = match &self.github {
            Some((api, token)) => (api.clone(), token.clone()),
            None => (
                std::env::var("WISPY_GITHUB_API").unwrap_or_else(|_| DEFAULT_API_BASE.to_string()),
                resolve_github_token().map_err(|e| e.to_string())?,
            ),
        };
        let github = GitHubClient::new(api_base, token.clone()).map_err(|e| e.to_string())?;
        let cache = Cache::open(&self.data_dir.join("cache.sqlite")).map_err(|e| e.to_string())?;
        let store = RepoStore::new(self.data_dir.join("repos"), Some(token));
        Ok(PrService::new(github, store, cache, Highlighter::new()))
    }

    /// Resolve the token and compile highlight queries now, not on the first open.
    pub fn warm_up(self: &Arc<Self>) {
        let backend = self.clone();
        std::thread::spawn(move || {
            let _ = backend.service();
        });
    }

    /// Opens the stack containing a PR (by URL or `owner/repo#N`), showing that PR alone.
    /// Returns the cached stack immediately when there is one (call [`Self::refresh_pr`] afterwards).
    pub fn open_pr(
        self: &Arc<Self>,
        input: String,
    ) -> impl Future<Output = Result<OpenedStack, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let t = std::time::Instant::now();
            let pr = PrRef::parse(&input).map_err(|e| e.to_string())?;
            let service = backend.service()?;
            let cached = backend
                .blocking({
                    let (service, pr) = (service.clone(), pr.clone());
                    move || {
                        let Some(snapshot) = service.cached_stack(&pr)? else {
                            return Ok(None);
                        };
                        let focus = snapshot.stack.focus;
                        Ok(service
                            .cached_range(&snapshot, focus, focus)?
                            .map(|view| (snapshot, view)))
                    }
                })
                .await?;
            if std::env::var_os("WISPY_DEBUG_TIMING").is_some() {
                eprintln!("[timing] backend cached read {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            }
            if let Some((snapshot, view)) = cached {
                return Ok(backend.open_stack(Arc::new(snapshot), Arc::new(view), true));
            }
            let (snapshot, view) = backend.discover_and_load(&service, &pr).await?;
            Ok(backend.open_stack(snapshot, view, false))
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Re-discovers the stack; returns it only if anything (structure or heads) changed.
    pub fn refresh_pr(
        self: &Arc<Self>,
        input: String,
        known_stack_id: String,
    ) -> impl Future<Output = Result<Option<OpenedStack>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let pr = PrRef::parse(&input).map_err(|e| e.to_string())?;
            let service = backend.service()?;
            let stack = service.discover(&pr).await.map_err(|e| e.to_string())?;
            let snapshot = backend
                .blocking({
                    let service = service.clone();
                    move || service.fetch(stack)
                })
                .await?;
            if stack_id(&snapshot) == known_stack_id {
                return Ok(None);
            }
            let (snapshot, view) = backend.load_focus(&service, Arc::new(snapshot)).await?;
            Ok(Some(backend.open_stack(snapshot, view, false)))
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Switches the shown range to stack PRs `lo..=hi` (optionally hiding whitespace-only changes).
    pub fn select_range(
        self: &Arc<Self>,
        stack_id: String,
        lo: usize,
        hi: usize,
        ignore_whitespace: bool,
    ) -> impl Future<Output = Result<OpenedRange, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let snapshot = backend
                .stack(&stack_id)
                .ok_or_else(|| format!("stack {stack_id} is no longer open"))?;
            if let Some(range) = backend.cached_range(&stack_id, lo, hi, ignore_whitespace) {
                backend.prewarm_index(&range.view_id);
                return Ok(range);
            }
            let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
            let id = view_id(&stack_id, lo, hi, ignore_whitespace);
            let service = backend.service()?;
            let view = backend
                .blocking(move || service.range_with(&snapshot, lo, hi, ignore_whitespace))
                .await?;
            backend.register_view_with_top(&id, &stack_id, hi, view.clone());
            backend.prewarm_index(&id);
            Ok(OpenedRange {
                view_id: id,
                range: Range { lo, hi },
                ignore_whitespace,
                since: None,
                view,
            })
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// An already-open range, without disk access, service initialization, or a thread hop.
    pub fn cached_range(
        self: &Arc<Self>,
        stack_id: &str,
        lo: usize,
        hi: usize,
        ignore_whitespace: bool,
    ) -> Option<OpenedRange> {
        let snapshot = self.stack(stack_id)?;
        let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
        let id = view_id(stack_id, lo, hi, ignore_whitespace);
        let view = self.view(&id)?;
        // As select_range does: the first usages lookup finds the index built.
        self.prewarm_index(&id);
        Some(OpenedRange {
            view_id: id,
            range: Range { lo, hi },
            ignore_whitespace,
            since: None,
            view,
        })
    }

    /// Full-file side-by-side rows (the UI draws the file header itself).
    pub fn split_file(
        self: &Arc<Self>,
        view_id: String,
        file: usize,
    ) -> impl Future<Output = Result<Arc<Vec<SplitRow>>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            if let Some(rows) = backend.split_file_cached(&view_id, file) {
                return Ok(rows);
            }
            let (view, snapshot) = backend
                .view_with_stack(&view_id)
                .ok_or_else(|| format!("diff {view_id} is no longer open"))?;
            let service = backend.service()?;
            let rows = Arc::new(
                backend
                    .blocking(move || service.split_rows(&snapshot, &view, file))
                    .await?,
            );
            backend.remember_split_file(&format!("{view_id}#{file}"), rows.clone());
            Ok(rows)
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Every stack PR's review threads (from the cache; with `refresh`, fetched from GitHub
    /// first), each with where it shows up in view `view_id`, whose range's top PR is `top`.
    pub fn threads(
        self: &Arc<Self>,
        view_id: String,
        top: usize,
        refresh: bool,
    ) -> impl Future<Output = Result<Vec<PlacedThread>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (view, snapshot) = backend
                .view_with_stack(&view_id)
                .ok_or_else(|| format!("diff {view_id} is no longer open"))?;
            let service = backend.service()?;
            if refresh {
                service
                    .refresh_threads(&snapshot)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            backend
                .blocking(move || {
                    let mut placed = Vec::new();
                    for pr in 0..snapshot.len() {
                        for thread in service.cached_threads(&snapshot, pr)? {
                            let location = thread.line.and_then(|line| {
                                let anchor = Anchor {
                                    pr: pr as u8,
                                    path: thread.path.clone(),
                                    side: thread.side,
                                    line,
                                };
                                locate(&view, top as u8, &anchor)
                            });
                            placed.push(PlacedThread {
                                pr,
                                thread,
                                location,
                            });
                        }
                    }
                    Ok(placed)
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Side-by-side rows already in the in-memory cache; never reads SQLite or builds rows.
    pub fn split_file_cached(&self, view_id: &str, file: usize) -> Option<Arc<Vec<SplitRow>>> {
        let key = format!("{view_id}#{file}");
        lock(&self.split_files)
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, rows)| rows.clone())
    }

    pub fn view(&self, id: &str) -> Option<Arc<DiffView>> {
        lock(&self.views)
            .iter()
            .find(|open| open.id == id)
            .map(|open| open.view.clone())
    }

    /// The symbol index of an open view, building it on first use (blocking).
    pub fn symbol_index(&self, view_id: &str) -> Result<Arc<SymbolIndex>, String> {
        let (view, snapshot) = self
            .view_with_stack(view_id)
            .ok_or_else(|| format!("diff {view_id} is no longer open"))?;
        let cell = {
            let mut indexes = lock(&self.indexes);
            if indexes.len() > 8 && !indexes.contains_key(view_id) {
                indexes.clear();
            }
            indexes.entry(view_id.to_string()).or_default().clone()
        };
        if let Some(index) = cell.get() {
            return Ok(index.clone());
        }
        let service = self.service()?;
        let mut failure = None;
        let index = cell.get_or_init(|| match service.symbol_index(&snapshot, &view) {
            Ok(index) => Arc::new(index),
            Err(err) => {
                failure = Some(err.to_string());
                Arc::new(SymbolIndex::build(&view, |_| None))
            }
        });
        match failure {
            Some(err) => Err(err),
            None => Ok(index.clone()),
        }
    }

    /// Registers the UI's notification callback, invoked on the precompute thread.
    pub fn set_on_range_ready(&self, f: impl Fn(RangeReady) + Send + Sync + 'static) {
        *lock(&self.on_range_ready) = Some(Arc::new(f));
    }

    /// Each file's content key in range `lo..=hi`, by path — without opening the range (used to
    /// tell which files a newer version of the stack changed).
    pub fn range_keys(
        self: &Arc<Self>,
        stack_id: String,
        lo: usize,
        hi: usize,
        ignore_whitespace: bool,
    ) -> impl Future<Output = Result<HashMap<String, String>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
            let view = match backend.view(&view_id(&stack_id, lo, hi, ignore_whitespace)) {
                Some(view) => view,
                None => {
                    backend
                        .blocking(move || service.range_with(&snapshot, lo, hi, ignore_whitespace))
                        .await?
                }
            };
            Ok(view
                .summary
                .files
                .iter()
                .map(|f| (f.path.clone(), f.content_key.clone()))
                .collect())
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Shows only what changed in `lo..=hi` since a checkpoint (rebase-aware).
    pub fn select_since(
        self: &Arc<Self>,
        stack_id: String,
        lo: usize,
        hi: usize,
        checkpoint_id: String,
    ) -> impl Future<Output = Result<OpenedRange, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
            let (view, checkpoint, conflicts, full_keys) = backend
                .blocking({
                    let (service, snapshot) = (service.clone(), snapshot.clone());
                    move || {
                        let checkpoint = service
                            .checkpoints(&snapshot, lo, hi)?
                            .into_iter()
                            .find(|c| c.id == checkpoint_id)
                            .ok_or_else(|| {
                                wispy_core::Error::GitHub(
                                    "checkpoint not found for this range".into(),
                                )
                            })?;
                        let (view, conflicts) =
                            service.interdiff(&snapshot, lo, hi, &checkpoint)?;
                        let full = service.range(&snapshot, lo, hi)?;
                        let full_keys = full
                            .summary
                            .files
                            .iter()
                            .map(|f| (f.path.clone(), f.content_key.clone()))
                            .collect();
                        Ok((view, checkpoint, conflicts, full_keys))
                    }
                })
                .await?;
            let id = format!(
                "{}:since:{}",
                view_id(&stack_id, lo, hi, false),
                checkpoint.id
            );
            backend.register_view_with_top(&id, &stack_id, hi, view.clone());
            backend.prewarm_index(&id);
            Ok(OpenedRange {
                view_id: id,
                range: Range { lo, hi },
                ignore_whitespace: false,
                view,
                since: Some(Since {
                    checkpoint_id: checkpoint.id,
                    created_at: checkpoint.created_at,
                    conflicts,
                    full_keys,
                }),
            })
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn list_checkpoints(
        self: &Arc<Self>,
        stack_id: String,
        lo: usize,
        hi: usize,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.checkpoints(&snapshot, lo, hi.min(snapshot.len() - 1)))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Records the current heads of `lo..=hi` as reviewed (local only).
    pub fn mark_reviewed(
        self: &Arc<Self>,
        stack_id: String,
        lo: usize,
        hi: usize,
    ) -> impl Future<Output = Result<Checkpoint, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.mark_reviewed(&snapshot, lo, hi, "manual"))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn get_viewed(
        self: &Arc<Self>,
        repo: String,
    ) -> impl Future<Output = Result<Vec<String>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend.blocking(move || service.viewed(&repo)).await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn set_viewed(
        self: &Arc<Self>,
        repo: String,
        key: String,
        viewed: bool,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.set_viewed(&repo, &key, viewed))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Highlighted head lines `start..=end` (1-based) of file `file`, for expanding a gap of
    /// unchanged lines in the unified diff.
    pub fn head_lines(
        self: &Arc<Self>,
        view_id: String,
        file: usize,
        start: u32,
        end: u32,
    ) -> impl Future<Output = Result<Vec<Vec<wispy_core::highlight::Seg>>, String>> + Send + 'static
    {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (view, snapshot) = backend
                .view_with_stack(&view_id)
                .ok_or_else(|| format!("diff {view_id} is no longer open"))?;
            if file >= view.summary.files.len() {
                return Err("no such file".into());
            }
            let service = backend.service()?;
            backend
                .blocking(move || service.head_lines(&snapshot, &view, file, start, end))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn get_ignore_patterns(
        self: &Arc<Self>,
        repo: String,
    ) -> impl Future<Output = Result<Vec<String>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.ignore_patterns(&repo))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn set_ignore_patterns(
        self: &Arc<Self>,
        repo: String,
        patterns: Vec<String>,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.set_ignore_patterns(&repo, &patterns))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Pending drafts of the stack, anchored to each PR's current head.
    pub fn list_drafts(
        self: &Arc<Self>,
        stack_id: String,
    ) -> impl Future<Output = Result<Vec<ShownDraft>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.shown_drafts(&snapshot))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn create_draft(
        self: &Arc<Self>,
        stack_id: String,
        draft: NewDraft,
    ) -> impl Future<Output = Result<Draft, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.create_draft(&snapshot, draft))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn update_draft(
        self: &Arc<Self>,
        id: String,
        body: Option<String>,
        as_file: Option<bool>,
    ) -> impl Future<Output = Result<Option<Draft>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.update_draft(&id, body, as_file))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn delete_draft(
        self: &Arc<Self>,
        id: String,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend.blocking(move || service.delete_draft(&id)).await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Keeps a pasted image (base64) for a comment; returns the Markdown to insert.
    pub fn save_image(
        self: &Arc<Self>,
        content_type: String,
        data: String,
    ) -> impl Future<Output = Result<String, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.save_image(&content_type, &data))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn image_data_url(
        self: &Arc<Self>,
        id: String,
    ) -> impl Future<Output = Result<Option<String>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend.blocking(move || service.image_data_url(&id)).await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Whether a line comment on these lines would be accepted as such (else: file comment).
    pub fn accepts_line_comment(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        path: String,
        side: Side,
        start: u32,
        end: u32,
    ) -> impl Future<Output = Result<bool, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || {
                    service.accepts_line_comment(&snapshot, pr_index, &path, side, start, end)
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Review threads of every stack PR from the cache; with `refresh`, fetched from GitHub first.
    pub fn list_threads(
        self: &Arc<Self>,
        stack_id: String,
        refresh: bool,
    ) -> impl Future<Output = Result<Vec<PrThreads>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            if refresh {
                service
                    .refresh_threads(&snapshot)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            backend
                .blocking(move || {
                    (0..snapshot.len())
                        .map(|pr_index| {
                            Ok(PrThreads {
                                pr_index,
                                threads: service.cached_threads(&snapshot, pr_index)?,
                            })
                        })
                        .collect()
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Re-fetches the stack, settles interrupted submits, and plans every PR's pending drafts.
    pub fn prepare_submit(
        self: &Arc<Self>,
        stack_id: String,
    ) -> impl Future<Output = Result<SubmitPlan, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            let pr = snapshot.pr_ref(snapshot.stack.focus);
            let stack = service.discover(&pr).await.map_err(|e| e.to_string())?;
            let viewer = service.viewer().await.map_err(|e| e.to_string())?;
            let fresh = Arc::new(
                backend
                    .blocking({
                        let service = service.clone();
                        move || service.fetch(stack)
                    })
                    .await?,
            );
            for index in 0..fresh.len() {
                service
                    .reconcile(&fresh, index)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let (plans, fresh) = backend
                .blocking({
                    let (service, fresh) = (service.clone(), fresh.clone());
                    move || {
                        let plans = (0..fresh.len())
                            .map(|pr_index| {
                                Ok(PrPlan {
                                    pr_index,
                                    planned: service.plan(&fresh, pr_index)?,
                                })
                            })
                            .collect::<wispy_core::Result<Vec<_>>>()?;
                        Ok((plans, fresh))
                    }
                })
                .await?;
            let (fresh, view) = backend.load_focus(&service, fresh).await?;
            let opened = backend.open_stack(fresh, view, false);
            Ok(SubmitPlan {
                stack: opened,
                prs: plans
                    .into_iter()
                    .filter(|p| !p.planned.is_empty())
                    .collect(),
                viewer,
            })
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Posts one review for a stack PR (after [`Self::prepare_submit`]).
    pub fn submit_review(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        verdict: Verdict,
        summary: Option<String>,
    ) -> impl Future<Output = Result<Outcome, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            let outcome = service
                .submit(&snapshot, pr_index, verdict, summary)
                .await
                .map_err(|e| e.to_string())?;
            if outcome.failed.is_empty() {
                // Submitting counts as having reviewed this PR at its current head.
                let _ = service.mark_reviewed(&snapshot, pr_index, pr_index, "submit");
            }
            // So the inbox shows the new review.
            backend.refresh_inbox();
            Ok(outcome)
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// What merging a stack PR would do: target branch, allowed methods, checks, prefilled message.
    pub fn merge_info(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
    ) -> impl Future<Output = Result<wispy_core::merge::MergeInfo, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            service
                .merge_info(&snapshot, pr_index)
                .await
                .map_err(|e| e.to_string())
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Merges a stack PR at its reviewed head: at once when green, otherwise by enabling auto-merge.
    pub fn merge_pr(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        request: wispy_core::merge::MergeRequest,
    ) -> impl Future<Output = Result<wispy_core::merge::MergeOutcome, String>> + Send + 'static
    {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            let outcome = service
                .merge(&snapshot, pr_index, &request)
                .await
                .map_err(|e| e.to_string())?;
            backend.refresh_inbox();
            Ok(outcome)
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Open PRs on GitHub matching `text` in `repos` (the palette's suggestions).
    pub fn search_prs(
        self: &Arc<Self>,
        text: String,
        repos: Vec<String>,
    ) -> impl Future<Output = Result<Vec<wispy_core::inbox::InboxPr>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move { backend.service()?.search_prs(&text, &repos).await.map_err(|e| e.to_string()) });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn usages(
        self: &Arc<Self>,
        view_id: String,
        name: String,
    ) -> impl Future<Output = Result<wispy_core::symbols::Usages, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let worker = backend.clone();
            backend
                .runtime()
                .spawn_blocking(move || {
                    let index = worker.symbol_index(&view_id)?;
                    Ok(index.usages(&name))
                })
                .await
                .map_err(|e| e.to_string())?
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Streams whole-repo `git grep` hits on the head of a stack PR.
    pub fn grep(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        query: String,
        whole_word: bool,
        mut on_event: impl FnMut(GrepEvent) + Send + 'static,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .runtime()
                .spawn_blocking(move || {
                    let result = service.grep(&snapshot, pr_index, &query, whole_word, |hits| {
                        on_event(GrepEvent::Hits { hits });
                        true
                    });
                    on_event(match result {
                        Ok(summary) => GrepEvent::Done {
                            total: summary.total,
                            unsearched: summary.unsearched,
                        },
                        Err(err) => GrepEvent::Failed {
                            message: err.to_string(),
                        },
                    });
                })
                .await
                .map_err(|e| e.to_string())
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// A whole file on a stack PR's head, highlighted (for files outside the diff).
    pub fn read_file(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        path: String,
    ) -> impl Future<Output = Result<Vec<Vec<wispy_core::highlight::Seg>>, String>> + Send + 'static
    {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || {
                    service
                        .read_file(&snapshot, pr_index, &path)
                        .map(|lines| lines.as_ref().clone())
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// One level of indentation in a file on a stack PR's head (for indenting suggested changes).
    pub fn indent_unit(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        path: String,
    ) -> impl Future<Output = Result<String, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.indent_unit(&snapshot, pr_index, &path))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Which of `paths` are files on a stack PR's head (for file references in assistant answers).
    pub fn existing_files(
        self: &Arc<Self>,
        stack_id: String,
        pr_index: usize,
        paths: Vec<String>,
    ) -> impl Future<Output = Result<Vec<String>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.existing_files(&snapshot, pr_index, &paths))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Asks the assistant (a follow-up in `thread_id`, or a new thread), streaming the answer.
    #[allow(clippy::too_many_arguments)]
    pub fn ask_assistant(
        self: &Arc<Self>,
        stack_id: String,
        lo: usize,
        hi: usize,
        thread_id: Option<String>,
        new_thread: Option<wispy_core::service::NewThread>,
        // A follow-up's model and effort (the conversation keeps them from then on).
        retune: Option<(Option<String>, Option<String>)>,
        question: String,
        mut on_event: impl FnMut(AssistantEvent) + Send + 'static,
    ) -> impl Future<Output = Result<Thread, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || {
                    if let (Some(id), Some((model, effort))) = (thread_id.as_deref(), retune) {
                        service.retune_assistant_thread(id, model, effort)?;
                    }
                    service.ask(
                        &snapshot,
                        (lo, hi),
                        thread_id.as_deref(),
                        new_thread,
                        &question,
                        |event| {
                            on_event(event.clone());
                        },
                    )
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Signs the assistant CLI in again: runs its sign-in in the background, opens the sign-in page
    /// in the browser (and sends it to `on_url`), and returns once the user has signed in.
    pub fn sign_in_assistant(
        self: &Arc<Self>,
        provider: Provider,
        mut on_url: impl FnMut(String) + Send + 'static,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            backend
                .blocking(move || {
                    wispy_core::assistant::sign_in(provider, |url| {
                        open_in_browser(url);
                        on_url(url.to_string());
                    })
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn list_assistant_threads(
        self: &Arc<Self>,
        stack_id: String,
    ) -> impl Future<Output = Result<Vec<Thread>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let (snapshot, service) = (backend.snapshot_of(&stack_id)?, backend.service()?);
            backend
                .blocking(move || service.assistant_threads(&snapshot))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    pub fn delete_assistant_thread(
        self: &Arc<Self>,
        id: String,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.delete_assistant_thread(&id))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// The last known inbox (instant, offline-friendly); fresh entries arrive as `inbox-updated`.
    pub fn get_inbox(
        self: &Arc<Self>,
    ) -> impl Future<Output = Result<Vec<wispy_core::service::InboxEntry>, String>> + Send + 'static
    {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || {
                    let groups = service.cached_inbox()?;
                    service.inbox_entries(groups)
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    fn snapshot_of(&self, stack_id: &str) -> Result<Arc<StackSnapshot>, String> {
        self.stack(stack_id)
            .ok_or_else(|| format!("stack {stack_id} is no longer open"))
    }

    /// Where each anchor shows up in a view (see [`locate`]).
    pub fn locate_anchors(
        &self,
        view_id: String,
        anchors: Vec<Anchor>,
    ) -> Result<Vec<Option<Location>>, String> {
        let (view, top) = lock(&self.views)
            .iter()
            .find(|open| open.id == view_id)
            .map(|open| (open.view.clone(), open.top))
            .ok_or_else(|| format!("diff {view_id} is no longer open"))?;
        Ok(anchors
            .iter()
            .map(|anchor| locate(&view, top, anchor))
            .collect())
    }

    pub fn locate_line(
        &self,
        view_id: String,
        file: usize,
        line: u32,
    ) -> Result<Option<Location>, String> {
        let view = self
            .view(&view_id)
            .ok_or_else(|| format!("diff {view_id} is no longer open"))?;
        Ok(wispy_core::anchors::locate_new_line(&view, file, line))
    }

    /// Keeps a pasted image locally; returns the Markdown that shows it.
    pub fn save_image_data(
        self: &Arc<Self>,
        content_type: String,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<String, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || service.save_image_data(&content_type, bytes))
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// A pasted image's encoded bytes and content type, for native image display.
    pub fn image_bytes(
        self: &Arc<Self>,
        id: String,
    ) -> impl Future<Output = Result<Option<(String, Vec<u8>)>, String>> + Send + 'static {
        let backend = self.clone();
        let task = self.runtime().spawn(async move {
            let service = backend.service()?;
            backend
                .blocking(move || {
                    use base64::Engine;
                    // PrService exposes images as data URLs; keep its cache private.
                    service
                        .image_data_url(&id)?
                        .map(|url| {
                            let (header, data) =
                                url.split_once(";base64,").expect("image data URL");
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(data)
                                .map_err(|e| wispy_core::Error::Image(e.to_string()))?;
                            Ok((header.trim_start_matches("data:").to_string(), bytes))
                        })
                        .transpose()
                })
                .await
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }

    /// Preferences loaded once from the data directory, independent of GitHub authentication.
    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    /// Session state shared across view remounts; never written to disk.
    pub fn lasting(&self) -> &Lasting {
        &self.lasting
    }

    /// Registers the UI's inbox callback, invoked on the prefetch worker.
    pub fn set_on_inbox(
        &self,
        f: impl Fn(Vec<wispy_core::service::InboxEntry>) + Send + Sync + 'static,
    ) {
        *lock(&self.on_inbox) = Some(Arc::new(f));
    }

    /// Registers the UI's connectivity callback, invoked on the prefetch worker.
    pub fn set_on_inbox_status(&self, f: impl Fn(bool) + Send + Sync + 'static) {
        *lock(&self.on_inbox_status) = Some(Arc::new(f));
    }

    pub(crate) fn emit_inbox(&self, entries: Vec<wispy_core::service::InboxEntry>) {
        let callback = lock(&self.on_inbox).clone();
        if let Some(callback) = callback {
            callback(entries);
        }
    }

    pub(crate) fn emit_inbox_status(&self, online: bool) {
        let callback = lock(&self.on_inbox_status).clone();
        if let Some(callback) = callback {
            callback(online);
        }
    }

    /// Starts inbox refresh and prefetch at launch, periodically, and whenever triggered.
    /// Repeated calls leave the existing worker running.
    pub fn start_prefetch(self: &Arc<Self>) {
        self.prefetch
            .get_or_init(|| crate::prefetch::start(Arc::downgrade(self)));
    }

    pub(crate) fn block_on_prefetch(&self) {
        self.runtime().block_on(crate::prefetch::cycle(self));
    }

    pub fn trigger_prefetch(&self) {
        if let Some(trigger) = self.prefetch.get() {
            trigger.fire();
        }
    }

    /// Asks the background worker to refresh the inbox and prefetch now.
    pub fn refresh_inbox(&self) {
        self.trigger_prefetch();
    }

    /// Opens a web link (from a rendered comment) in the default browser.
    pub fn open_url(&self, url: String) -> Result<(), String> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("not a web link: {url}"));
        }
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(&url)
            .spawn()
            .map(drop)
            .map_err(|e| format!("{opener}: {e}"))
    }

    fn register_stack(&self, snapshot: Arc<StackSnapshot>) -> String {
        let id = stack_id(&snapshot);
        lock(&self.stacks).insert(id.clone(), snapshot);
        *lock(&self.current_stack) = Some(id.clone());
        id
    }

    fn stack(&self, id: &str) -> Option<Arc<StackSnapshot>> {
        lock(&self.stacks).get(id).cloned()
    }

    fn is_current_stack(&self, id: &str) -> bool {
        lock(&self.current_stack).as_deref() == Some(id)
    }

    /// Claims the precompute worker slot for a stack; false if one is already running.
    fn start_precompute(&self, id: &str) -> bool {
        lock(&self.precomputing).insert(id.to_string())
    }

    fn finish_precompute(&self, id: &str) {
        lock(&self.precomputing).remove(id);
    }

    #[cfg(test)]
    fn register_view(&self, id: &str, stack_id: &str, view: Arc<DiffView>) {
        self.register_view_with_top(id, stack_id, 0, view);
    }

    fn register_view_with_top(&self, id: &str, stack_id: &str, top: usize, view: Arc<DiffView>) {
        let mut views = lock(&self.views);
        views.retain(|open| open.id != id);
        views.push_front(OpenView {
            id: id.to_string(),
            stack_id: stack_id.to_string(),
            top: top as u8,
            view,
        });
        views.truncate(MAX_OPEN_VIEWS);
    }

    /// The view and the stack it belongs to.
    fn view_with_stack(&self, id: &str) -> Option<(Arc<DiffView>, Arc<StackSnapshot>)> {
        let (view, stack_id) = lock(&self.views)
            .iter()
            .find(|o| o.id == id)
            .map(|o| (o.view.clone(), o.stack_id.clone()))?;
        Some((view, self.stack(&stack_id)?))
    }

    fn remember_split_file(&self, key: &str, rows: Arc<Vec<SplitRow>>) {
        let mut files = lock(&self.split_files);
        files.retain(|(k, _)| k != key);
        files.push_front((key.to_string(), rows));
        files.truncate(MAX_SPLIT_FILES);
    }

    /// Builds a view's symbol index in the background so the first usages lookup is instant.
    fn prewarm_index(self: &Arc<Self>, view_id: &str) {
        // Weak: a backend being shut down (window closed) isn't kept alive for this.
        let (backend, view_id) = (Arc::downgrade(self), view_id.to_string());
        std::thread::spawn(move || {
            // Let the view's first paint have the CPU.
            std::thread::sleep(std::time::Duration::from_millis(400));
            if let Some(backend) = backend.upgrade() {
                let _ = backend.symbol_index(&view_id);
            }
        });
    }

    async fn discover_and_load(
        &self,
        service: &Arc<PrService>,
        pr: &PrRef,
    ) -> Result<(Arc<StackSnapshot>, Arc<DiffView>), String> {
        let stack = service.discover(pr).await.map_err(|e| e.to_string())?;
        let snapshot = self
            .blocking({
                let service = service.clone();
                move || service.fetch(stack)
            })
            .await?;
        self.load_focus(service, Arc::new(snapshot)).await
    }

    async fn load_focus(
        &self,
        service: &Arc<PrService>,
        snapshot: Arc<StackSnapshot>,
    ) -> Result<(Arc<StackSnapshot>, Arc<DiffView>), String> {
        let service = service.clone();
        let for_view = snapshot.clone();
        let view = self
            .blocking(move || {
                let focus = for_view.stack.focus;
                service.range(&for_view, focus, focus)
            })
            .await?;
        Ok((snapshot, view))
    }

    fn open_stack(
        self: &Arc<Self>,
        snapshot: Arc<StackSnapshot>,
        view: Arc<DiffView>,
        from_cache: bool,
    ) -> OpenedStack {
        let stack_id = self.register_stack(snapshot.clone());
        let focus = snapshot.stack.focus;
        let id = view_id(&stack_id, focus, focus, false);
        self.register_view_with_top(&id, &stack_id, focus, view.clone());
        self.prewarm_index(&id);
        self.spawn_precompute(stack_id.clone(), snapshot.clone());
        OpenedStack {
            stack_id,
            prs: snapshot.stack.prs.clone(),
            needs_rebase: snapshot.needs_rebase(),
            focus,
            range: Range {
                lo: focus,
                hi: focus,
            },
            view_id: id,
            snapshot,
            view,
            from_cache,
            ignore_whitespace: false,
        }
    }

    /// Computes every range of the stack into the cache (most useful first) so switching ranges
    /// is a cache read. Stops as soon as another stack is opened.
    fn spawn_precompute(self: &Arc<Self>, stack_id: String, snapshot: Arc<StackSnapshot>) {
        if !self.start_precompute(&stack_id) {
            return;
        }
        let backend = self.clone();
        std::thread::spawn(move || {
            if let Ok(service) = backend.service() {
                for (lo, hi) in snapshot.ranges_by_priority() {
                    if !backend.is_current_stack(&stack_id) {
                        break;
                    }
                    match service.precompute(&snapshot, lo, hi) {
                        Ok(_) => {
                            let callback = lock(&backend.on_range_ready).clone();
                            if let Some(callback) = callback {
                                callback(RangeReady {
                                    stack_id: stack_id.clone(),
                                    lo,
                                    hi,
                                });
                            }
                        }
                        Err(err) => eprintln!("precompute {stack_id} {lo}-{hi}: {err}"),
                    }
                }
            }
            backend.finish_precompute(&stack_id);
        });
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> wispy_core::Result<T> + Send + 'static,
    ) -> Result<T, String> {
        self.runtime()
            .spawn_blocking(f)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            // Runtime::drop blocks (and panics inside async code). Tasks can own the last Arc.
            runtime.shutdown_background();
        }
    }
}

/// Opens `url` in the default browser (`WISPY_OPENER` overrides the opener; tests use a fake).
fn open_in_browser(url: &str) {
    let default = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let opener = std::env::var("WISPY_OPENER").unwrap_or_else(|_| default.to_string());
    let _ = std::process::Command::new(opener).arg(url).status();
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wispy_core::stack::Stack;

    fn snapshot() -> Arc<StackSnapshot> {
        let pr = PullRequest {
            number: 7,
            title: "Test".into(),
            state: "open".into(),
            draft: false,
            html_url: String::new(),
            author: "test".into(),
            base_ref: "main".into(),
            base_sha: "base".into(),
            head_ref: "feature".into(),
            head_sha: "0123456789abcdef".into(),
            clone_url: String::new(),
            base_repo: "owner/repo".into(),
            head_repo: Some("owner/repo".into()),
            default_branch: "main".into(),
        };
        let mut second = pr.clone();
        second.number = 9;
        Arc::new(StackSnapshot {
            stack: Stack {
                prs: vec![pr, second],
                focus: 0,
            },
            heads: vec!["0123456789abcdef".into(), "short".into()],
            bases: vec!["base".into(), "0123456789abcdef".into()],
        })
    }

    fn empty_view() -> Arc<DiffView> {
        Arc::new(DiffView::build(
            "base",
            "head",
            &[],
            &[],
            &HashMap::new(),
            &HashSet::new(),
        ))
    }

    #[test]
    fn stack_and_view_ids() {
        let id = stack_id(&snapshot());
        assert_eq!(id, "owner/repo:7@0123456789ab,9@short");
        assert_eq!(
            view_id(&id, 0, 1, false),
            "owner/repo:7@0123456789ab,9@short:0-1"
        );
        assert_eq!(
            view_id(&id, 1, 1, true),
            "owner/repo:7@0123456789ab,9@short:1-1:w"
        );
        let mut changed = (*snapshot()).clone();
        changed.heads[1] = "another-head".into();
        assert_ne!(stack_id(&changed), id);
    }

    #[test]
    fn data_dir_override() {
        let _env = lock(&TEST_ENV);
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("WISPY_DATA_DIR", value),
                    None => std::env::remove_var("WISPY_DATA_DIR"),
                }
            }
        }
        let _restore = Restore(std::env::var_os("WISPY_DATA_DIR"));
        std::env::set_var("WISPY_DATA_DIR", "/tmp/wispy-backend-override");
        assert_eq!(
            default_data_dir(),
            PathBuf::from("/tmp/wispy-backend-override")
        );
    }

    #[test]
    fn view_lru_keeps_four_and_replaces_existing_entries() {
        let backend = Backend::new(PathBuf::new());
        let view = empty_view();
        for i in 0..4 {
            backend.register_view(&i.to_string(), "stack", view.clone());
        }
        backend.register_view("0", "stack", view.clone());
        backend.register_view("4", "stack", view.clone());
        assert_eq!(lock(&backend.views).len(), 4);
        assert!(backend.view("1").is_none());
        for id in ["0", "2", "3", "4"] {
            assert!(Arc::ptr_eq(&backend.view(id).unwrap(), &view));
        }
        backend.register_view("5", "stack", view);
        assert!(backend.view("2").is_none());
    }

    #[test]
    fn split_lru_keeps_thirty_two_and_replaces_existing_entries() {
        let backend = Backend::new(PathBuf::new());
        let rows = Arc::new(Vec::new());
        for i in 0..32 {
            backend.remember_split_file(&format!("view#{i}"), rows.clone());
        }
        backend.remember_split_file("view#0", rows.clone());
        backend.remember_split_file("view#32", rows.clone());
        assert_eq!(lock(&backend.split_files).len(), 32);
        assert!(backend.split_file_cached("view", 1).is_none());
        assert!(backend.split_file_cached("other", 0).is_none());
        for i in [0, 2, 31, 32] {
            assert!(Arc::ptr_eq(
                &backend.split_file_cached("view", i).unwrap(),
                &rows
            ));
        }
    }

    #[test]
    fn cached_range_is_synchronous_and_never_initializes_service() {
        let backend = Backend::new(PathBuf::new());
        let id = backend.register_stack(snapshot());
        let view = empty_view();
        backend.register_view(&view_id(&id, 0, 1, false), &id, view.clone());
        let cached = backend.cached_range(&id, 0, 99, false).unwrap();
        assert_eq!(cached.range, Range { lo: 0, hi: 1 });
        assert!(Arc::ptr_eq(&cached.view, &view));
        assert!(backend.cached_range(&id, 0, 1, true).is_none());
        assert!(backend.cached_range("missing", 0, 0, false).is_none());
        assert!(backend.service.get().is_none());
    }

    #[test]
    fn async_api_runs_on_a_non_tokio_executor() {
        let backend = Backend::new(PathBuf::new());
        let error = futures::executor::block_on(backend.open_pr("invalid".into()))
            .err()
            .unwrap();
        assert!(!error.is_empty());
        let error =
            futures::executor::block_on(backend.select_range("missing".into(), 0, 0, false))
                .err()
                .unwrap();
        assert_eq!(error, "stack missing is no longer open");
        let error =
            futures::executor::block_on(backend.split_file("missing".into(), 0)).unwrap_err();
        assert_eq!(error, "diff missing is no longer open");
        let pending = backend.select_range("missing".into(), 0, 0, false);
        drop(backend);
        assert!(futures::executor::block_on(pending).is_err());
    }

    #[test]
    fn precompute_claims_are_unique_and_reusable() {
        let backend = Backend::new(PathBuf::new());
        let id = backend.register_stack(snapshot());
        assert!(backend.is_current_stack(&id));
        assert!(backend.start_precompute(&id));
        assert!(!backend.start_precompute(&id));
        backend.finish_precompute(&id);
        assert!(backend.start_precompute(&id));
    }
}
