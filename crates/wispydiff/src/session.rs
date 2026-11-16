//! What the user is reviewing (was `App.tsx`'s state): the open stack and range, its drafts and
//! threads, viewed marks, ignore patterns, the inbox, and the newer version waiting to be loaded.
//! The title bar, the inbox and the diff viewer all read it; it notifies on every change.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use futures::channel::mpsc;
use futures::StreamExt;
use gpui_kit::{App, Context, EventEmitter, Task, Window};
use wispy_core::progress::Checkpoint;
use wispy_core::service::{InboxEntry, ShownDraft};

use crate::backend::{Backend, OpenedRange, OpenedStack, PrThreads};
use crate::logic::comments::pr_key;
use crate::logic::glob::GlobMatcher;
use crate::settings;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BaseMode {
    Unified,
    Split,
}

/// Something to do later, once the user answered (the newer-changes warning).
pub type Later = Rc<dyn Fn(&mut Window, &mut App)>;

pub enum SessionEvent {
    Toast(String),
    /// A PR was opened (the palette closes, the diff takes the focus).
    Opened,
    /// Marking `path` viewed waits for the newer-changes warning; `mark` marks it anyway.
    ViewedWarning { path: String, mark: Later },
}

pub struct Session {
    pub backend: Arc<Backend>,
    pub stack: Option<OpenedStack>,
    pub shown: Option<OpenedRange>,
    /// A newer version of the open stack, waiting for "load latest".
    pub newer: Option<OpenedStack>,
    pub home: bool,
    pub inbox: Vec<InboxEntry>,
    pub online: Option<bool>,
    /// The PR being opened, and why the last open failed.
    pub busy: Option<String>,
    pub error: Option<String>,
    pub show_files: bool,
    pub default_mode: BaseMode,
    pub wrap: bool,
    /// PRs (`pr_key`) whose threads are all hidden (`h`, remembered).
    /// Oldest first (the saved list keeps the latest 500).
    pub hidden_comment_prs: indexmap::IndexSet<String>,
    /// The assistant's panels (per repo), alive while the window is.
    pub assistant: crate::diff::assistant::Panels,
    pub ignore_patterns: Vec<String>,
    ignore: GlobMatcher,
    pub drafts: Vec<ShownDraft>,
    pub threads: Vec<PrThreads>,
    pub viewed: HashSet<String>,
    /// Marks set or cleared while the repo's marks were loading (they win over what loads).
    viewed_changes: HashMap<String, bool>,
    pub checkpoints: Vec<Checkpoint>,
    /// Ranges the backend has precomputed, per stack.
    ready: HashMap<String, HashSet<(usize, usize)>>,
    range_request: u64,
    /// The newer version's content keys for the shown range: (which range, keys or unknown).
    newer_keys: Option<(String, Option<HashMap<String, String>>)>,
    /// The newer version the user chose to ignore (no more warnings for it).
    dismissed_version: Option<String>,
    checking: bool,
    /// The stack bar's anchor for shift-click (a single PR picked another way becomes it).
    chip_anchor: usize,
    _listener: Task<()>,
}

enum Incoming {
    RangeReady(String, usize, usize),
    Inbox(Vec<InboxEntry>),
    Status(bool),
}

impl EventEmitter<SessionEvent> for Session {}

impl Session {
    pub fn new(backend: Arc<Backend>, cx: &mut Context<Self>) -> Self {
        // Backend callbacks run on its threads: forward them to the UI thread.
        let (tx, mut rx) = mpsc::unbounded::<Incoming>();
        {
            let tx = tx.clone();
            backend.set_on_range_ready(move |r| {
                let _ = tx.unbounded_send(Incoming::RangeReady(r.stack_id, r.lo, r.hi));
            });
        }
        {
            let tx = tx.clone();
            backend.set_on_inbox(move |entries| {
                let _ = tx.unbounded_send(Incoming::Inbox(entries));
            });
        }
        backend.set_on_inbox_status(move |online| {
            let _ = tx.unbounded_send(Incoming::Status(online));
        });
        let listener = cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    match message {
                        Incoming::RangeReady(stack, lo, hi) => {
                            this.ready.entry(stack).or_default().insert((lo, hi));
                        }
                        Incoming::Inbox(entries) => this.inbox = entries,
                        Incoming::Status(online) => {
                            this.online = Some(online);
                            // Each background refresh (launch, every 5 minutes, focus) also checks the open PR.
                            if online {
                                this.check_for_new_commits(cx);
                            }
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let mode = settings::pref(cx, "defaultMode", &["unified", "split"], "unified");
        let wrap = settings::pref(cx, "wrap", &["on", "off"], "off") == "on";
        let mut session = Session {
            backend: backend.clone(),
            stack: None,
            shown: None,
            newer: None,
            home: true,
            inbox: Vec::new(),
            online: None,
            busy: None,
            error: None,
            show_files: true,
            default_mode: if mode == "split" { BaseMode::Split } else { BaseMode::Unified },
            wrap,
            hidden_comment_prs: settings::list(cx, "hiddenCommentPrs").into_iter().collect(),
            assistant: Default::default(),
            ignore_patterns: Vec::new(),
            ignore: GlobMatcher::new(&[]),
            drafts: Vec::new(),
            threads: Vec::new(),
            viewed: HashSet::new(),
            viewed_changes: HashMap::new(),
            checkpoints: Vec::new(),
            ready: HashMap::new(),
            range_request: 0,
            newer_keys: None,
            dismissed_version: None,
            checking: false,
            chip_anchor: 0,
            _listener: listener,
        };
        session.load_inbox(cx);
        backend.start_prefetch();
        session
    }

    pub fn repo(&self) -> Option<String> {
        self.stack.as_ref().map(|s| s.prs[0].base_repo.clone())
    }

    pub fn is_ignored(&self, path: &str) -> bool {
        self.ignore.matches(path)
    }

    pub fn is_viewed(&self, key: &str) -> bool {
        self.viewed.contains(key)
    }

    pub fn ready(&self, lo: usize, hi: usize) -> bool {
        let Some(stack) = &self.stack else { return false };
        self.ready.get(&stack.stack_id).is_some_and(|r| r.contains(&(lo, hi)))
    }

    pub fn showing_diff(&self) -> bool {
        !self.home && self.stack.is_some() && self.shown.is_some()
    }

    // ---------- inbox ----------

    /// The last known inbox (re-read on every return home: marks made since show up).
    pub fn load_inbox(&mut self, cx: &mut Context<Self>) {
        let task = self.backend.get_inbox();
        cx.spawn(async move |this, cx| {
            if let Ok(entries) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.inbox = entries;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn refresh_inbox(&mut self) {
        self.backend.refresh_inbox();
    }

    pub fn go_home(&mut self, cx: &mut Context<Self>) {
        self.home = true;
        self.load_inbox(cx);
        cx.notify();
    }

    // ---------- opening ----------

    pub fn open(&mut self, input: String, cx: &mut Context<Self>) {
        crate::perf_marks::mark("open:start");
        self.busy = Some(input.clone());
        self.error = None;
        cx.notify();
        let task = self.backend.open_pr(input);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(opened) => {
                        crate::perf_marks::mark("open:backend");
                        let label = pr_label(&opened.prs[opened.focus]);
                        settings::remember_pr(cx, &label);
                        let (from_cache, stack_id) = (opened.from_cache, opened.stack_id.clone());
                        this.show(opened, cx);
                        this.home = false;
                        cx.emit(SessionEvent::Opened);
                        if from_cache {
                            // Stale-while-revalidate: show the cached stack now, offer the new one if anything moved.
                            this.refresh(label, stack_id, cx);
                        }
                    }
                    Err(err) => {
                        eprintln!("opening the PR failed: {err}");
                        this.error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks GitHub whether the stack moved; a newer version waits for "load latest".
    fn refresh(&mut self, label: String, stack_id: String, cx: &mut Context<Self>) {
        let task = self.backend.refresh_pr(label, stack_id.clone());
        cx.spawn(async move |this, cx| {
            let fresh = task.await;
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                if let Ok(Some(fresh)) = fresh {
                    if this.stack.as_ref().is_some_and(|s| s.stack_id == stack_id) {
                        this.newer = Some(fresh);
                        this.update_newer_keys(cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn check_for_new_commits(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        let Some(stack) = &self.stack else { return };
        let (label, id) = (pr_label(&stack.prs[stack.focus]), stack.stack_id.clone());
        self.checking = true;
        self.refresh(label, id, cx);
    }

    /// Shows a stack (just opened, or the newer version).
    pub fn show(&mut self, opened: OpenedStack, cx: &mut Context<Self>) {
        let previous_repo = self.repo();
        let previous_stack = self.stack.as_ref().map(|s| s.stack_id.clone());
        self.shown = Some(OpenedRange {
            view_id: opened.view_id.clone(),
            range: opened.range,
            ignore_whitespace: opened.ignore_whitespace,
            since: None,
            view: opened.view.clone(),
        });
        self.chip_anchor = opened.range.lo;
        self.stack = Some(opened);
        self.newer = None;
        self.newer_keys = None;
        if previous_stack.as_deref() != self.stack.as_ref().map(|s| s.stack_id.as_str()) {
            self.drafts.clear();
            self.threads.clear();
            self.refresh_drafts(cx);
            // Cached threads first (offline-friendly), then GitHub's current state.
            self.load_threads(false, cx);
            self.load_threads(true, cx);
        }
        if previous_repo != self.repo() {
            self.load_repo_state(cx);
        }
        cx.notify();
    }

    pub fn load_newer(&mut self, cx: &mut Context<Self>) {
        if let Some(newer) = self.newer.take() {
            self.show(newer, cx);
        }
    }

    fn load_repo_state(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo() else { return };
        self.viewed.clear();
        self.viewed_changes.clear();
        let patterns = self.backend.get_ignore_patterns(repo.clone());
        let viewed = self.backend.get_viewed(repo);
        cx.spawn(async move |this, cx| {
            let patterns = patterns.await.unwrap_or_default();
            let viewed = viewed.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.ignore = GlobMatcher::new(&patterns);
                this.ignore_patterns = patterns;
                this.viewed = viewed.into_iter().collect();
                for (key, value) in this.viewed_changes.drain() {
                    if value {
                        this.viewed.insert(key);
                    } else {
                        this.viewed.remove(&key);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn refresh_drafts(&mut self, cx: &mut Context<Self>) {
        let Some(stack) = &self.stack else { return };
        let task = self.backend.list_drafts(stack.stack_id.clone());
        cx.spawn(async move |this, cx| match task.await {
            Ok(drafts) => {
                let _ = this.update(cx, |this, cx| {
                    this.drafts = drafts;
                    cx.notify();
                });
            }
            Err(err) => eprintln!("drafts: {err}"),
        })
        .detach();
    }

    pub fn load_threads(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let Some(stack) = &self.stack else { return };
        let id = stack.stack_id.clone();
        let task = self.backend.list_threads(id.clone(), refresh);
        cx.spawn(async move |this, cx| {
            if let Ok(threads) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.stack.as_ref().is_some_and(|s| s.stack_id == id) {
                        this.threads = threads;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    // ---------- ranges ----------

    pub fn choose_range(&mut self, lo: usize, hi: usize, ignore_whitespace: Option<bool>, cx: &mut Context<Self>) {
        let Some(stack) = &self.stack else { return };
        let ignore_whitespace = ignore_whitespace.unwrap_or_else(|| self.shown.as_ref().is_some_and(|s| s.ignore_whitespace));
        crate::perf_marks::mark("range:start");
        self.range_request += 1;
        let request = self.range_request;
        if let Some(opened) = self.backend.cached_range(&stack.stack_id, lo, hi, ignore_whitespace) {
            self.set_shown(opened, cx);
            return;
        }
        let task = self.backend.select_range(stack.stack_id.clone(), lo, hi, ignore_whitespace);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                // A later selection wins even if its response arrives first.
                Ok(opened) if request == this.range_request => this.set_shown(opened, cx),
                Ok(_) => {}
                Err(err) => eprintln!("range switch failed: {err}"),
            });
        })
        .detach();
    }

    fn set_shown(&mut self, shown: OpenedRange, cx: &mut Context<Self>) {
        if shown.range.lo == shown.range.hi {
            self.chip_anchor = shown.range.lo;
        }
        self.shown = Some(shown);
        self.update_newer_keys(cx);
        cx.notify();
    }

    /// The stack bar: click selects one PR; shift-click extends to a contiguous range.
    pub fn click_chip(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        if extend {
            let (lo, hi) = (self.chip_anchor.min(index), self.chip_anchor.max(index));
            self.choose_range(lo, hi, None, cx);
        } else {
            self.chip_anchor = index;
            self.choose_range(index, index, None, cx);
        }
    }

    /// Tab / ⇧Tab: the next / previous PR of the stack on its own, wrapping around.
    pub fn cycle_pr(&mut self, back: bool, cx: &mut Context<Self>) {
        let (Some(stack), Some(shown)) = (&self.stack, &self.shown) else { return };
        let size = stack.prs.len();
        if size <= 1 {
            return;
        }
        let index = if back { (shown.range.lo + size - 1) % size } else { (shown.range.hi + 1) % size };
        self.choose_range(index, index, None, cx);
    }

    /// `[`/`]` move the selection down/up the stack; `{`/`}` extend it.
    pub fn step_range(&mut self, key: char, cx: &mut Context<Self>) {
        let (Some(stack), Some(shown)) = (&self.stack, &self.shown) else { return };
        let (lo, hi, size) = (shown.range.lo, shown.range.hi, stack.prs.len());
        let next = match key {
            '[' if lo > 0 => Some((lo - 1, hi - 1)),
            ']' if hi < size - 1 => Some((lo + 1, hi + 1)),
            '{' if lo > 0 => Some((lo - 1, hi)),
            '}' if hi < size - 1 => Some((lo, hi + 1)),
            _ => None,
        };
        if let Some((lo, hi)) = next {
            self.choose_range(lo, hi, None, cx);
        }
    }

    pub fn toggle_whitespace(&mut self, cx: &mut Context<Self>) {
        if let Some(shown) = &self.shown {
            let (range, ws) = (shown.range, shown.ignore_whitespace);
            self.choose_range(range.lo, range.hi, Some(!ws), cx);
        }
    }

    pub fn mark_reviewed(&mut self, cx: &mut Context<Self>) {
        let (Some(stack), Some(shown)) = (&self.stack, &self.shown) else { return };
        let (lo, hi) = (shown.range.lo, shown.range.hi);
        let which = if lo == hi {
            format!("#{}", stack.prs[lo].number)
        } else {
            format!("#{}–#{}", stack.prs[lo].number, stack.prs[hi].number)
        };
        let head = stack.prs[hi].head_sha.chars().take(7).collect::<String>();
        let task = self.backend.mark_reviewed(stack.stack_id.clone(), lo, hi);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| match result {
                Ok(_) => cx.emit(SessionEvent::Toast(format!(
                    "Marked {which} as reviewed at {head} — press d later to see what changed"
                ))),
                Err(err) => cx.emit(SessionEvent::Toast(err)),
            });
        })
        .detach();
    }

    /// Shows the changes since a checkpoint (the latest one when none is given).
    pub fn show_since(&mut self, checkpoint: Option<String>, cx: &mut Context<Self>) {
        let (Some(stack), Some(shown)) = (&self.stack, &self.shown) else { return };
        let (id, lo, hi) = (stack.stack_id.clone(), shown.range.lo, shown.range.hi);
        let backend = self.backend.clone();
        let list = backend.list_checkpoints(id.clone(), lo, hi);
        cx.spawn(async move |this, cx| {
            let available = match list.await {
                Ok(available) => available,
                Err(err) => {
                    let _ = this.update(cx, |_, cx| cx.emit(SessionEvent::Toast(err)));
                    return;
                }
            };
            let chosen = checkpoint.or_else(|| available.first().map(|c| c.id.clone()));
            let request = this
                .update(cx, |this, cx| {
                    this.checkpoints = available;
                    cx.notify();
                    if chosen.is_none() {
                        cx.emit(SessionEvent::Toast("No checkpoint for this range yet — press M to mark it as reviewed".into()));
                        return None;
                    }
                    crate::perf_marks::mark("range:start");
                    this.range_request += 1;
                    Some(this.range_request)
                })
                .ok()
                .flatten();
            let (Some(request), Some(chosen)) = (request, chosen) else { return };
            let result = backend.select_since(id, lo, hi, chosen).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(opened) if request == this.range_request => this.set_shown(opened, cx),
                Ok(_) => {}
                Err(err) => cx.emit(SessionEvent::Toast(err)),
            });
        })
        .detach();
    }

    /// `d`: changes since the last checkpoint, or back to the full diff.
    pub fn toggle_since(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = &self.shown else { return };
        if shown.since.is_some() {
            let range = shown.range;
            self.choose_range(range.lo, range.hi, Some(false), cx);
        } else {
            self.show_since(None, cx);
        }
    }

    // ---------- viewed marks ----------

    pub fn set_viewed(&mut self, key: &str, value: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.repo() else { return };
        // In "changes since checkpoint" the mark also applies to the file in the full diff.
        let mut keys = vec![key.to_string()];
        if let Some(shown) = &self.shown {
            if let Some(since) = &shown.since {
                let path = shown.view.summary.files.iter().find(|f| f.content_key == key).map(|f| f.path.clone());
                if let Some(full) = path.and_then(|p| since.full_keys.get(&p)) {
                    if full != key {
                        keys.push(full.clone());
                    }
                }
            }
        }
        for k in keys {
            if value {
                self.viewed.insert(k.clone());
            } else {
                self.viewed.remove(&k);
            }
            self.viewed_changes.insert(k.clone(), value);
            self.backend.set_viewed(repo.clone(), k, value).detach_unwrap(cx);
        }
        cx.notify();
    }

    /// The newer version's range matching the one shown: (cache key, lo, hi, ignore whitespace, its stack).
    fn newer_range(&self) -> Option<(String, Option<(usize, usize)>, bool, String)> {
        let (newer, stack, shown) = (self.newer.as_ref()?, self.stack.as_ref()?, self.shown.as_ref()?);
        let find = |i: usize| newer.prs.iter().position(|p| p.number == stack.prs[i].number);
        let (lo, hi) = (find(shown.range.lo), find(shown.range.hi));
        let ws = if shown.since.is_some() { false } else { shown.ignore_whitespace };
        let range = match (lo, hi) {
            (Some(lo), Some(hi)) if hi >= lo => Some((lo, hi)),
            _ => None,
        };
        let key = format!("{}:{:?}-{:?}:{ws}", newer.stack_id, lo, hi);
        Some((key, range, ws, newer.stack_id.clone()))
    }

    fn update_newer_keys(&mut self, cx: &mut Context<Self>) {
        let Some((key, range, ws, stack_id)) = self.newer_range() else { return };
        if self.newer_keys.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let Some((lo, hi)) = range else {
            self.newer_keys = Some((key, None));
            return;
        };
        let task = self.backend.range_keys(stack_id, lo, hi, ws);
        cx.spawn(async move |this, cx| {
            let keys = task.await.ok();
            let _ = this.update(cx, |this, _| this.newer_keys = Some((key, keys)));
        })
        .detach();
    }

    /// Marking a file viewed while a newer version is waiting warns once per version, and only
    /// when the file changed in it (its content key at the same range differs; unknown counts as
    /// changed). False: hold the mark back (`mark` does it if the user says so).
    pub fn confirm_viewed(&mut self, path: &str, key: &str, mark: Later, cx: &mut Context<Self>) -> bool {
        let Some(newer) = &self.newer else { return true };
        if self.dismissed_version.as_deref() == Some(&newer.stack_id) {
            return true;
        }
        let Some((range_key, ..)) = self.newer_range() else { return true };
        let shown = self.shown.as_ref();
        let current = shown.and_then(|s| s.since.as_ref()).and_then(|s| s.full_keys.get(path)).map(String::as_str).unwrap_or(key);
        let newer_keys = self.newer_keys.as_ref().filter(|(k, _)| *k == range_key).and_then(|(_, keys)| keys.as_ref());
        if newer_keys.and_then(|keys| keys.get(path)).is_some_and(|k| k == current) {
            return true;
        }
        cx.emit(SessionEvent::ViewedWarning { path: path.to_string(), mark });
        false
    }

    /// "Mark viewed anyway": no more warnings for this newer version.
    pub fn dismiss_newer_warnings(&mut self) {
        self.dismissed_version = self.newer.as_ref().map(|n| n.stack_id.clone());
    }

    // ---------- preferences ----------

    pub fn update_ignore(&mut self, patterns: Vec<String>, cx: &mut Context<Self>) {
        let Some(repo) = self.repo() else { return };
        self.ignore = GlobMatcher::new(&patterns);
        self.ignore_patterns = patterns.clone();
        self.backend.set_ignore_patterns(repo, patterns).detach_unwrap(cx);
        cx.notify();
    }

    pub fn set_default_mode(&mut self, mode: BaseMode, cx: &mut Context<Self>) {
        self.default_mode = mode;
        settings::set_pref(cx, "defaultMode", if mode == BaseMode::Split { "split" } else { "unified" });
        cx.notify();
    }

    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap = wrap;
        settings::set_pref(cx, "wrap", if wrap { "on" } else { "off" });
        cx.notify();
    }

    pub fn set_comments_hidden(&mut self, keys: &[String], hide: bool, cx: &mut Context<Self>) {
        for key in keys {
            self.hidden_comment_prs.shift_remove(key);
            if hide {
                self.hidden_comment_prs.insert(key.clone());
            }
        }
        let list: Vec<String> = self.hidden_comment_prs.iter().cloned().collect();
        let keep = list.len().saturating_sub(500);
        settings::set_list(cx, "hiddenCommentPrs", list[keep..].to_vec());
        cx.notify();
    }

    /// The range's PR keys, and whether all their threads are hidden.
    pub fn range_keys(&self) -> (Vec<String>, bool) {
        let (Some(stack), Some(shown)) = (&self.stack, &self.shown) else { return (Vec::new(), false) };
        let keys: Vec<String> = stack.prs[shown.range.lo..=shown.range.hi].iter().map(pr_key).collect();
        let hidden = keys.iter().all(|k| self.hidden_comment_prs.contains(k));
        (keys, hidden)
    }

    pub fn toggle_files(&mut self, cx: &mut Context<Self>) {
        self.show_files = !self.show_files;
        cx.notify();
    }

    /// After the submit sheet sent something: drafts and threads changed; a newer stack waits.
    pub fn after_submit(&mut self, fresh: OpenedStack, done: bool, message: String, cx: &mut Context<Self>) {
        if done {
            cx.emit(SessionEvent::Toast(message));
        }
        self.refresh_drafts(cx);
        self.load_threads(false, cx);
        if self.stack.as_ref().is_some_and(|s| s.stack_id != fresh.stack_id) {
            self.newer = Some(fresh);
            self.update_newer_keys(cx);
        }
        cx.notify();
    }
}

pub fn pr_label(pr: &wispy_core::github::PullRequest) -> String {
    format!("{}#{}", pr.base_repo, pr.number)
}

/// Runs a backend future for its side effect, logging a failure.
pub trait DetachUnwrap {
    fn detach_unwrap<T: 'static>(self, cx: &mut Context<T>);
}

impl<F, R> DetachUnwrap for F
where
    F: std::future::Future<Output = Result<R, String>> + Send + 'static,
    R: Send + 'static,
{
    fn detach_unwrap<T: 'static>(self, cx: &mut Context<T>) {
        cx.background_executor().spawn(async move {
            if let Err(err) = self.await {
                eprintln!("{err}");
            }
        })
        .detach();
    }
}
