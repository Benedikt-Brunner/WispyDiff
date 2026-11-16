//! The diff of the selected range (was `DiffViewer.tsx`): every file in its mode, comments in
//! place, the file list, and the panels (usages / search, assistant, file view, file search).
//!
//! Rows come straight from the core's in-memory `DiffView`; the list (GPUI `list`) measures each
//! row as it comes into view, so wrapped lines and inserted comments need no precomputed heights.
//! The reader's place — (file, row in file, mode, pixels into the row) — survives every layout
//! change.

pub mod assistant;
pub mod code_panel;
pub mod file_list;
pub mod file_search;
pub mod file_view;
pub mod inserts;
pub mod layout;
pub mod rows;
pub mod selection;
pub mod testing_a;
pub mod testing_b;
pub mod testing_c;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    div, list, px, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyBinding, ListAlignment, ListOffset,
    ListState, ModifiersChangedEvent, MouseButton, NavigationDirection, Pixels, Point, Render, Subscription,
    Task, Window,
};
use serde_json::Value;
use wispy_core::anchors::{Anchor, Location};
use wispy_core::github::{PullRequest, ReviewThread, Side};
use wispy_core::highlight::Seg;
use wispy_core::model::{row_kind, DiffView, FileSummary, Row};
use wispy_core::service::ShownDraft;
use wispy_core::split::SplitRow;

use crate::backend::Backend;
use crate::logic::comments::{anchor_key, file_index_for, line_label, path_in_pr, pr_key};
use crate::logic::gaps::{all_shown, hidden_lines, reveal_all, reveal_more, DisplayRow, UnifiedMap, GAP_STEP};
use crate::logic::history::NavHistory;
use crate::session::{BaseMode, Session};
use crate::settings;
use crate::ui::{palette, u};

use self::assistant::AssistantPanel;
use self::code_panel::{CodePanel, CodePanelEvent};
use self::file_search::{FileSearch, FileSearchEvent};
use self::file_view::FileViewState;
use self::inserts::{settled_reason, thread_preview, Composer, ComposerEvent};
use self::layout::{Built, Item, Mode, Placed};
use self::selection::TextSelection;

/// Rows the sticky file header covers at the top (rounded up).
const STICKY_ROWS: u32 = 2;
/// Arrow keys scroll by this many pixels (CSS px at 100%; held: continuously).
const ARROW_STEP: f32 = 60.;
/// Two Shift presses this close together open the file search.
const DOUBLE_SHIFT: Duration = Duration::from_millis(400);
/// Within this many rows counts as the same place for back / forward.
const SAME_PLACE_ROWS: u32 = 10;

gpui_kit::actions!(
    diff,
    [
        ScrollDown,
        ScrollUp,
        NextFileAny,
        PrevFileAny,
        NextFile,
        PrevFile,
        ToggleMode,
        ToggleAllModes,
        ToggleWrap,
        ToggleThreads,
        ToggleListMode,
        OpenFilter,
        ToggleFilter,
        HideExtension,
        ToggleCollapse,
        ToggleGaps,
        Comment,
        Usages_,
        Search,
        Assistant,
        Escape,
        ToggleViewed,
        UndoViewed,
        Back,
        Forward,
        Copy,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Diff && !Input");
    cx.bind_keys([
        KeyBinding::new("down", ScrollDown, ctx),
        KeyBinding::new("up", ScrollUp, ctx),
        KeyBinding::new("shift-down", NextFileAny, ctx),
        KeyBinding::new("shift-up", PrevFileAny, ctx),
        KeyBinding::new("n", NextFile, ctx),
        KeyBinding::new("p", PrevFile, ctx),
        KeyBinding::new("s", ToggleMode, ctx),
        KeyBinding::new("shift-s", ToggleAllModes, ctx),
        KeyBinding::new("z", ToggleWrap, ctx),
        KeyBinding::new("h", ToggleThreads, ctx),
        KeyBinding::new("t", ToggleListMode, ctx),
        KeyBinding::new("f", OpenFilter, ctx),
        KeyBinding::new("shift-f", ToggleFilter, ctx),
        KeyBinding::new("x", HideExtension, ctx),
        KeyBinding::new("e", ToggleCollapse, ctx),
        KeyBinding::new("space", ToggleCollapse, ctx),
        KeyBinding::new("shift-e", ToggleGaps, ctx),
        KeyBinding::new("c", Comment, ctx),
        KeyBinding::new("u", Usages_, ctx),
        KeyBinding::new("/", Search, ctx),
        KeyBinding::new("a", Assistant, ctx),
        KeyBinding::new("v", ToggleViewed, ctx),
        KeyBinding::new("shift-v", UndoViewed, ctx),
        KeyBinding::new("alt-left", Back, ctx),
        KeyBinding::new("alt-right", Forward, ctx),
        KeyBinding::new("secondary-[", Back, ctx),
        KeyBinding::new("secondary-]", Forward, ctx),
        KeyBinding::new("secondary-alt-[", Back, ctx),
        KeyBinding::new("secondary-alt-]", Forward, ctx),
        KeyBinding::new("secondary-c", Copy, Some("Diff")),
        // Escape closes the assistant, then the panels, then an open comment box, from anywhere.
        KeyBinding::new("escape", Escape, Some("Diff")),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("cmd-left", Back, ctx), KeyBinding::new("cmd-right", Forward, ctx)]);
}

/// A line comment's target: a row (side) and where a comment on it must point.
#[derive(Clone, Debug)]
pub struct Target {
    pub file: usize,
    pub mode: Mode,
    pub offset: u32,
    pub anchor: Anchor,
}

/// Rows selected in the gutter (dragging), or flashed after a jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowRange {
    pub file: usize,
    pub mode: Mode,
    pub from: u32,
    pub to: u32,
}

/// The reader's place: a row of a file in a mode, and how far it's scrolled past.
#[derive(Clone, Debug)]
struct Place {
    path: String,
    offset: u32,
    mode: Mode,
    delta: Pixels,
}

/// A place back / forward returns to: a spot in the diff, or a file open read-only.
#[derive(Clone, Debug)]
pub enum HistoryPlace {
    Diff { place_path: String, offset: u32, mode: Mode, delta: Pixels, line: Option<(u32, Pixels)> },
    File { path: String, line: u32, end: u32 },
}

fn same_place(a: &HistoryPlace, b: &HistoryPlace) -> bool {
    match (a, b) {
        (HistoryPlace::Diff { place_path: p1, offset: o1, mode: m1, .. }, HistoryPlace::Diff { place_path: p2, offset: o2, mode: m2, .. }) => {
            p1 == p2 && m1 == m2 && o1.abs_diff(*o2) < SAME_PLACE_ROWS
        }
        (HistoryPlace::File { path: p1, line: l1, .. }, HistoryPlace::File { path: p2, line: l2, .. }) => p1 == p2 && l1 == l2,
        _ => false,
    }
}

/// Files left out of the diff by extension (like GitHub's file filter), remembered per PR / range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFilter {
    /// Hidden extensions ("" = files without one).
    pub exts: Vec<String>,
    /// Switched off with `F`: everything shown, the extensions kept for switching back on.
    pub off: bool,
}

impl FileFilter {
    fn from_value(value: Option<&Value>) -> FileFilter {
        let Some(Value::Object(o)) = value else { return FileFilter::default() };
        let exts = o.get("exts").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        FileFilter { exts, off: o.get("off").and_then(Value::as_bool).unwrap_or(false) }
    }

    fn to_value(&self) -> Value {
        serde_json::json!({ "exts": self.exts, "off": self.off })
    }
}

/// A file's extensions, most specific first: [".unit.php", ".php"] for "src/A.unit.php"; empty
/// without one (dotfiles like ".gitignore" included). Multi-part ones go up to three parts, each
/// starting with a letter (so "v1.2.json" has just ".json").
pub fn extension_suffixes(path: &str) -> Vec<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let dots: Vec<usize> = name.match_indices('.').map(|(i, _)| i).filter(|&i| i > 0).collect();
    let mut out = Vec::new();
    for (n, &dot) in dots.iter().rev().enumerate() {
        let suffix = &name[dot..];
        if n > 0 {
            let part = &suffix[1..suffix[1..].find('.').map_or(suffix.len(), |i| i + 1)];
            if n > 2 || !part.starts_with(|c: char| c.is_ascii_alphabetic()) {
                break;
            }
        }
        out.push(suffix.to_string());
    }
    out.reverse();
    out
}

/// Whether `filter` leaves out `path` (any of its extensions unticked).
pub fn filter_hides(filter: &FileFilter, path: &str) -> bool {
    let suffixes = extension_suffixes(path);
    if suffixes.is_empty() {
        filter.exts.iter().any(|e| e.is_empty())
    } else {
        suffixes.iter().any(|s| filter.exts.contains(s))
    }
}

/// ".php" for "src/a.test.php"; "" without one (dotfiles like ".gitignore" included).
pub fn extension_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[dot..].to_string(),
        _ => String::new(),
    }
}

/// A hidden thread's gutter (or file header) icon: the threads that share it.
#[derive(Clone, Debug)]
pub struct CommentMark {
    pub header: bool,
    pub right: bool,
    /// Every thread here is one GitHub folds away (resolved, outdated, minimized).
    pub settled: bool,
    pub threads: Vec<HiddenThread>,
}

#[derive(Clone, Debug)]
pub struct HiddenThread {
    pub id: String,
    pub preview: String,
    pub settled: bool,
}

/// What a placed comment item shows.
#[derive(Clone)]
pub enum CommentKind {
    Thread { pr_index: usize, thread: ReviewThread },
    Draft(ShownDraft),
    Composer,
}

/// A thread, draft or the open composer, placed at its anchor or under its file's header.
#[derive(Clone)]
pub struct CommentItem {
    pub key: String,
    pub anchor: Option<Anchor>,
    pub path: String,
    pub hidden: Option<HiddenThread>,
    /// What a collapsed file counts this as ("open", a settled reason, "draft"); None: always shown.
    pub tally: Option<String>,
    pub kind: CommentKind,
}

/// The open comment box: where its comment goes.
#[derive(Clone, Debug)]
pub struct ComposerTarget {
    pub key: String,
    pub file_kind: bool,
    pub pr_index: usize,
    pub path: String,
    pub side: Option<Side>,
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    /// It'll go out as a file comment (None while checking).
    pub fallback: Option<bool>,
}

/// Held arrow keys: pixels still to scroll, eased over frames.
struct Glide {
    rest: Pixels,
}

pub enum Panel {
    Code(Entity<CodePanel>),
}

pub struct DiffViewer {
    pub session: Entity<Session>,
    backend: Arc<Backend>,

    // ---- the view (from the session) ----
    pub stack_id: String,
    pub view_id: String,
    pub view: Arc<DiffView>,
    pub prs: Vec<PullRequest>,
    pub lo: usize,
    pub hi: usize,
    since: bool,
    signature: u64,

    // ---- per-file choices, by path (they survive range switches) ----
    overrides: HashMap<String, BaseMode>,
    expanded: HashSet<String>,
    collapsed: HashSet<String>,
    /// A collapsed file opened for a look: it collapses again once another file is shown.
    temporary: Option<String>,
    pub list_mode_tree: bool,
    pub tree_closed: HashSet<String>,
    tree_done: HashSet<String>,
    filters: serde_json::Map<String, Value>,
    pub filter_open: bool,
    pub filter_highlight: usize,
    file_search: Option<Entity<FileSearch>>,

    // ---- per view ----
    reveals: HashMap<usize, Vec<[u32; 2]>>,
    head_lines: HashMap<usize, HashMap<u32, Arc<Vec<Seg>>>>,
    split: HashMap<usize, Arc<Vec<SplitRow>>>,
    split_loading: HashSet<usize>,
    gap_requests: HashMap<usize, u64>,

    // ---- layout ----
    pub built: Built,
    pub items_by_key: HashMap<String, CommentItem>,
    pub marks: HashMap<(u32, bool), CommentMark>,
    pub tallies: HashMap<usize, HashMap<String, usize>>,
    pub list: ListState,
    anchor: Option<Place>,
    pinned: Option<(usize, ListOffset)>,
    pending_file: Option<(usize, usize)>,
    keep_file: Option<usize>,
    reveal_file: Option<usize>,
    pending_jump: Option<(usize, u32, u32)>,
    /// A jump to run once the layout it waited for is drawn.
    pending_jump_now: Option<(usize, u32, u32)>,

    // ---- comments ----
    pub composer: Option<(ComposerTarget, Entity<Composer>)>,
    /// The rows the open composer comments on (and whether on the new side), kept highlighted.
    pub composer_rows: Option<(RowRange, bool)>,
    pub selection: Option<RowRange>,
    dragging: Option<Target>,
    pub hovered: Option<Target>,
    /// Oldest first (the saved list keeps the latest 500).
    hidden_threads: indexmap::IndexSet<String>,
    revealed: HashSet<String>,
    hidden_prs_seen: indexmap::IndexSet<String>,
    pub thread_replies: HashMap<String, Entity<Composer>>,
    pub unfolded: HashSet<String>,
    pub draft_edits: HashMap<String, Entity<Composer>>,

    // ---- code intelligence ----
    panel: Option<Panel>,
    pub file_view: Option<FileViewState>,
    pub flash: Option<RowRange>,
    flash_task: Option<Task<()>>,
    history: NavHistory<HistoryPlace>,
    viewed_history: Vec<String>,

    // ---- assistant ----
    /// The assistant panel while it's shown here.
    pub assistant: Option<Entity<AssistantPanel>>,
    /// The repo's assistant panel (shown or not: its threads give the gutter markers).
    assistant_panel: Option<Entity<AssistantPanel>>,

    // ---- input ----
    glide: Option<Glide>,
    last_shift: Option<Instant>,
    shift_down: bool,
    pub hover_word: Option<String>,
    pub text_selection: Option<TextSelection>,
    pub scroll_x: Pixels,
    pub char_width: Option<Pixels>,
    char_zoom: f32,
    pub pointer: Option<Point<Pixels>>,
    /// The current file as of this frame (headers drawn in the list mark it).
    pub frame_current: Option<usize>,
    /// Scrolls while a text selection is dragged past the list's top or bottom edge.
    drag_scroll: Option<gpui_kit::Task<()>>,
    pub wrap_columns: (usize, usize),
    pub pending_mark: Option<&'static str>,
    pub file_list_width: f32,
    pub filter_focus_handle: Option<FocusHandle>,
    pub file_rows: std::rc::Rc<Vec<file_list::FileRow>>,
    pub file_scroll: gpui_kit::UniformListScrollHandle,
    pub file_list_current: Option<usize>,
    pub code_panel_width: f32,
    pub assistant_width: f32,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl DiffViewer {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = session.read(cx);
        let backend = s.backend.clone();
        let stack = s.stack.as_ref().expect("a stack is open");
        let shown = s.shown.as_ref().expect("a range is shown");
        let (stack_id, prs) = (stack.stack_id.clone(), stack.prs.clone());
        let (view_id, view, lo, hi, since) = (shown.view_id.clone(), shown.view.clone(), shown.range.lo, shown.range.hi, shown.since.is_some());
        let hidden_prs_seen = s.hidden_comment_prs.clone();
        let subscriptions = vec![cx.observe_in(&session, window, |this, _, window, cx| this.sync(window, cx))];
        let width = |key: &str, fallback: f32, cx: &App| settings::get::<f32>(cx, &format!("width.{key}")).filter(|w| *w > 0.).unwrap_or(fallback);
        let mut viewer = DiffViewer {
            backend,
            stack_id,
            view_id,
            view,
            prs,
            lo,
            hi,
            since,
            signature: 0,
            overrides: HashMap::new(),
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            temporary: None,
            list_mode_tree: settings::pref(cx, "fileList", &["list", "tree"], "list") == "tree",
            tree_closed: HashSet::new(),
            tree_done: HashSet::new(),
            filters: settings::record(cx, "fileFilters"),
            filter_open: false,
            filter_highlight: 0,
            file_search: None,
            reveals: HashMap::new(),
            head_lines: HashMap::new(),
            split: HashMap::new(),
            split_loading: HashSet::new(),
            gap_requests: HashMap::new(),
            built: Built::default(),
            items_by_key: HashMap::new(),
            marks: HashMap::new(),
            tallies: HashMap::new(),
            list: ListState::new(0, ListAlignment::Top, px(1200.)),
            anchor: None,
            pinned: None,
            pending_file: None,
            keep_file: None,
            reveal_file: None,
            pending_jump: None,
            pending_jump_now: None,
            composer: None,
            composer_rows: None,
            selection: None,
            dragging: None,
            hovered: None,
            hidden_threads: settings::list(cx, "hiddenThreads").into_iter().collect(),
            revealed: HashSet::new(),
            hidden_prs_seen,
            thread_replies: HashMap::new(),
            unfolded: HashSet::new(),
            draft_edits: HashMap::new(),
            panel: None,
            file_view: None,
            flash: None,
            flash_task: None,
            history: NavHistory::new(same_place),
            viewed_history: Vec::new(),
            assistant: None,
            assistant_panel: None,
            glide: None,
            last_shift: None,
            shift_down: false,
            hover_word: None,
            text_selection: None,
            scroll_x: px(0.),
            char_width: None,
            char_zoom: 1.0,
            pointer: None,
            frame_current: None,
            drag_scroll: None,
            wrap_columns: (0, 0),
            pending_mark: Some("view"),
            file_list_width: width("files", 280., cx),
            filter_focus_handle: None,
            file_rows: Default::default(),
            file_scroll: gpui_kit::UniformListScrollHandle::new(),
            file_list_current: None,
            code_panel_width: width("code", 380., cx),
            assistant_width: width("assistant", 420., cx),
            focus: cx.focus_handle(),
            session,
            _subscriptions: subscriptions,
        };
        crate::perf_marks::mark("open:viewer-new");
        viewer.rebuild(cx);
        crate::perf_marks::mark("open:rebuilt");
        viewer.restore_anchor(true);
        let (panel, subscription, shown) = assistant::attach(&mut viewer, window, cx);
        viewer._subscriptions.push(subscription);
        if shown {
            viewer.assistant = Some(panel.clone());
        }
        viewer.assistant_panel = Some(panel);
        viewer
    }

    // ---------- inputs ----------

    fn s<'a>(&self, cx: &'a App) -> &'a Session {
        self.session.read(cx)
    }

    pub fn is_viewed(&self, key: &str, cx: &App) -> bool {
        self.s(cx).is_viewed(key)
    }

    /// Picks up session changes: a new range (a new view) or anything that moves rows.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.session.read(cx);
        let (Some(stack), Some(shown)) = (&s.stack, &s.shown) else { return };
        if stack.stack_id != self.stack_id {
            return;
        }
        // Hiding or showing a PR's threads (h) starts over: the ones shown from their icons follow it.
        if s.hidden_comment_prs != self.hidden_prs_seen {
            self.hidden_prs_seen = s.hidden_comment_prs.clone();
            self.revealed.clear();
        }
        let new_view = shown.view_id != self.view_id;
        if new_view {
            self.record_anchor(cx);
            self.view_id = shown.view_id.clone();
            self.view = shown.view.clone();
            self.lo = shown.range.lo;
            self.hi = shown.range.hi;
            self.since = shown.since.is_some();
            self.prs = stack.prs.clone();
            self.reveals.clear();
            self.head_lines.clear();
            self.split.clear();
            self.split_loading.clear();
            self.selection = None;
            self.text_selection = None;
            self.pending_mark = Some("view");
            self.pending_file = None;
            self.pinned = None;
            self.history = NavHistory::new(same_place);
            self.viewed_history.clear();
            self.glide = None;
            self.rebuild(cx);
            self.restore_anchor(false);
            self.sync_assistant(cx);
            if let Some(panel) = self.code_panel().cloned() {
                let (view_id, stack_id, hi) = (self.view_id.clone(), self.stack_id.clone(), self.hi);
                panel.update(cx, |p, cx| p.set_view(view_id, stack_id, hi, window, cx));
            }
            if let Some(assistant) = &self.assistant {
                let _ = assistant;
            }
            cx.notify();
            return;
        }
        if self.layout_signature(cx) != self.signature {
            self.relayout(cx);
        } else {
            cx.notify();
        }
    }

    /// Everything outside this view that moves rows.
    fn layout_signature(&self, cx: &App) -> u64 {
        let s = self.s(cx);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (s.default_mode == BaseMode::Split).hash(&mut h);
        s.ignore_patterns.hash(&mut h);
        for f in &self.view.summary.files {
            s.is_viewed(&f.content_key).hash(&mut h);
        }
        for d in &s.drafts {
            (&d.draft.id, &d.draft.body, d.line, d.start_line, &d.path, format!("{:?}", d.draft.status)).hash(&mut h);
        }
        for t in &s.threads {
            t.pr_index.hash(&mut h);
            for th in &t.threads {
                (&th.id, th.resolved, th.outdated, th.line, th.comments.len()).hash(&mut h);
            }
        }
        let mut hidden: Vec<&String> = s.hidden_comment_prs.iter().collect();
        hidden.sort();
        hidden.hash(&mut h);
        h.finish()
    }

    // ---------- modes ----------

    pub fn filter_key(&self) -> String {
        let (lo, hi) = (&self.prs[self.lo], &self.prs[self.hi]);
        if self.lo == self.hi {
            pr_key(lo)
        } else {
            format!("{}..#{}", pr_key(lo), hi.number)
        }
    }

    pub fn filter(&self) -> FileFilter {
        FileFilter::from_value(self.filters.get(&self.filter_key()))
    }

    /// Per file index: left out by the filter.
    pub fn filtered(&self) -> Vec<bool> {
        let filter = self.filter();
        self.view.summary.files.iter().map(|f| !filter.off && filter_hides(&filter, &f.path)).collect()
    }

    fn mode_of_with(&self, index: usize, filtered: &[bool], cx: &App) -> Mode {
        let s = self.s(cx);
        let file = &self.view.summary.files[index];
        if filtered[index] {
            return Mode::Hidden;
        }
        let is_temp = self.temporary.as_deref() == Some(file.path.as_str());
        let hidden = !is_temp
            && (self.collapsed.contains(&file.path)
                || ((file.noise.is_some() || s.is_ignored(&file.path) || s.is_viewed(&file.content_key)) && !self.expanded.contains(&file.path)));
        if hidden {
            return Mode::Collapsed;
        }
        let wanted = self.overrides.get(&file.path).copied().unwrap_or(s.default_mode);
        if wanted == BaseMode::Split && file.split_rows > 0 {
            Mode::Split
        } else {
            Mode::Unified
        }
    }

    pub fn segment(&self, file: usize) -> &layout::Segment {
        &self.built.segments[file]
    }

    // ---------- layout ----------

    /// Rebuilds the list (keeping the reader's place: call `record_anchor` first).
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filtered = self.filtered();
        let modes: Vec<Mode> = (0..self.view.summary.files.len()).map(|i| self.mode_of_with(i, &filtered, cx)).collect();
        let mut built = Built::new(&self.view, |i| modes[i], &self.reveals);
        let items = self.comment_items(cx);
        let (placed, marks, tallies) = self.place_items(&built, &items, cx);
        built.place(placed);
        self.built = built;
        self.items_by_key = items.into_iter().map(|i| (i.key.clone(), i)).collect();
        self.marks = marks;
        self.tallies = tallies;
        self.list.reset_with_uniform_height(self.built.items.len(), u_px(20., cx));
        self.signature = self.layout_signature(cx);
        self.update_tree_done(cx);
    }

    /// A layout change (modes, collapsing, gaps, comments, filter): keeps the reader's place.
    pub fn relayout(&mut self, cx: &mut Context<Self>) {
        self.record_anchor(cx);
        self.rebuild(cx);
        self.restore_anchor(true);
        cx.notify();
    }

    /// Records the anchor, flags the perf mark, then applies `f` and lays out again.
    pub fn change_layout(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Context<Self>)) {
        self.glide = None;
        self.record_anchor(cx);
        self.pending_mark = Some("layout");
        crate::perf_marks::mark("toggle:start");
        let modes_before: Vec<Mode> = self.built.segments.iter().map(|s| s.mode).collect();
        f(self, cx);
        self.rebuild(cx);
        // Sideways scrolling fits one mode's widths (side by side halves are narrower): a file
        // changing mode starts at the left again, or the code can sit off screen.
        if self.built.segments.iter().map(|s| s.mode).ne(modes_before.iter().copied()) {
            self.scroll_x = px(0.);
        }
        self.restore_anchor(true);
        cx.notify();
    }

    /// Remembers the reader's place: the row at the top of the list.
    fn record_anchor(&mut self, _: &App) {
        if self.built.items.is_empty() {
            return;
        }
        let top = self.list.logical_scroll_top();
        let (row, delta) = match self.built.items.get(top.item_ix) {
            Some(Item::Row(row)) => (*row, top.offset_in_item),
            Some(Item::Insert(i)) => (self.built.inserts[*i as usize].after, px(0.)),
            None => (self.built.total_rows.saturating_sub(1), px(0.)),
        };
        let Some(segment) = self.built.segment_at(row) else { return };
        self.anchor = Some(Place { path: self.view.summary.files[segment.file].path.clone(), offset: row - segment.start, mode: segment.mode, delta });
    }

    /// How far the top of the view is past row `row`'s top: positive inside the top row, negative
    /// for a row further down (it keeps its place on screen).
    fn row_delta(&self, row: u32, top_row: u32, top: ListOffset) -> Pixels {
        if row == top_row {
            return top.offset_in_item;
        }
        match self.list.bounds_for_item(self.built.item_of_row(row)) {
            Some(bounds) => self.list.viewport_bounds().top() - bounds.top(),
            None => px(0.),
        }
    }

    /// Scrolls so that the top of the view is `delta` past item `item`'s top (negative: above it).
    fn scroll_to_item_delta(&self, item: usize, delta: Pixels) -> ListOffset {
        self.list.scroll_to(ListOffset { item_ix: item, offset_in_item: delta.max(px(0.)) });
        if delta < px(0.) {
            self.list.scroll_by(delta);
        }
        self.list.logical_scroll_top()
    }

    /// Puts the view back at the recorded place (or where a navigation wants it).
    fn restore_anchor(&mut self, same_view: bool) {
        let mut pin = None;
        let mut target: Option<ListOffset> = None;
        if let Some(reveal) = self.reveal_file.take() {
            if same_view && reveal < self.built.segments.len() {
                self.pending_file = None;
                target = Some(ListOffset { item_ix: self.built.item_of_row(self.built.segments[reveal].start), offset_in_item: px(0.) });
                pin = Some(reveal);
            }
        }
        if target.is_none() {
            if let Some((from, next)) = self.pending_file {
                if same_view && matches!(self.built.segments.get(from).map(|s| s.mode), Some(Mode::Collapsed | Mode::Hidden)) {
                    self.pending_file = None;
                    target = Some(ListOffset { item_ix: self.built.item_of_row(self.built.segments[next].start), offset_in_item: px(0.) });
                    pin = Some(next);
                }
            }
        }
        if target.is_none() {
            if let Some(place) = &self.anchor {
                if let Some(index) = self.view.summary.files.iter().position(|f| f.path == place.path) {
                    let segment = &self.built.segments[index];
                    let keep = same_view && segment.mode == place.mode;
                    let offset = if keep { place.offset.min(segment.rows.saturating_sub(1)) } else { 0 };
                    target = Some(ListOffset {
                        item_ix: self.built.item_of_row(segment.start + offset),
                        offset_in_item: if keep { place.delta } else { px(0.) },
                    });
                }
            }
        }
        if pin.is_none() {
            pin = self.keep_file.take();
        }
        self.keep_file = None;
        if !same_view {
            pin = None;
        }
        let target = target.unwrap_or_default();
        let target = self.scroll_to_item_delta(target.item_ix, target.offset_in_item);
        self.pinned = pin.map(|file| (file, target));
        self.record_anchor_from(target);
        self.after_layout();
    }

    fn record_anchor_from(&mut self, target: ListOffset) {
        let row = self.built.row_of_item(target.item_ix);
        if let Some(segment) = self.built.segment_at(row) {
            self.anchor = Some(Place {
                path: self.view.summary.files[segment.file].path.clone(),
                offset: row - segment.start,
                mode: segment.mode,
                delta: target.offset_in_item,
            });
        }
    }

    /// A jump that waited for its file to change mode.
    fn after_layout(&mut self) {
        if let Some(jump) = self.pending_jump.take() {
            self.pending_jump_now = Some(jump);
        }
    }

    // ---------- current file ----------

    /// The file keys act on: the pinned one while the view hasn't moved, else the one at the top
    /// (one row in, so a header scrolled to the top counts). None in an empty diff.
    pub fn current(&self) -> Option<usize> {
        if self.built.total_rows == 0 {
            return None;
        }
        // Pinned until the view moves (scrolling unpins), even where the list couldn't scroll the
        // file to the top (the last files of the diff).
        if let Some((file, _)) = self.pinned {
            return Some(file);
        }
        // The file at the top: the one whose header is pinned there (see `sticky_header`).
        let top = self.list.logical_scroll_top();
        self.built.segment_at(self.built.row_of_item(top.item_ix)).map(|s| s.file)
    }

    /// The header of the file at the top, pinned there while its own has scrolled away (so it's
    /// clear which file this is, and which one keys act on); the next file's header pushes it
    /// out as it arrives.
    fn sticky_header(&mut self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if self.built.items.is_empty() {
            return None;
        }
        let top = self.list.logical_scroll_top();
        let segment = self.built.segment_at(self.built.row_of_item(top.item_ix))?.clone();
        if segment.mode == Mode::Hidden || segment.rows == 0 {
            return None;
        }
        let gap = if segment.start == 0 { px(0.) } else { u_px(rows::FILE_GAP, cx) };
        // Its own header bar is still in full view.
        if top.item_ix == self.built.item_of_row(segment.start) && top.offset_in_item <= gap {
            return None;
        }
        let height = u_px(rows::HEADER, cx);
        let end = segment.start + segment.rows;
        let mut shift = px(0.);
        if end < self.built.total_rows {
            // How far below the top the next file's header (its band) begins.
            let next = self.built.item_of_row(end);
            let (mut y, mut ix) = (-top.offset_in_item, top.item_ix);
            while ix < next && y < height {
                y += self.list.bounds_for_item(ix).map_or(u_px(20., cx), |b| b.size.height);
                ix += 1;
            }
            if ix == next && y < height {
                shift = y - height;
            }
        }
        let p = palette(cx).clone();
        let bar = self.file_header_bar(segment.file, segment.mode, segment.start, true, cx);
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(shift)
                .occlude()
                .when(p.shadows, |d| d.shadow_sm())
                .child(bar)
                .into_any_element(),
        )
    }

    /// Makes `file` the current one where it is (a click on its header or collapsed line).
    pub fn pin_file(&mut self, file: usize, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        self.pinned = Some((file, top));
        cx.notify();
    }

    /// The global row at the top of the view.
    fn top_row(&self) -> u32 {
        self.built.row_of_item(self.list.logical_scroll_top().item_ix)
    }

    // ---------- scrolling ----------

    /// Puts `row` near the top, with `margin` rows above it; `pin` makes that file the current one.
    pub fn scroll_to_row(&mut self, row: u32, margin: u32, pin: Option<usize>, cx: &mut Context<Self>) {
        self.glide = None;
        let item = self.built.item_of_row(self.top_row_for(row, margin));
        let target = ListOffset { item_ix: item, offset_in_item: px(0.) };
        self.list.scroll_to(target);
        self.record_anchor_from(target);
        self.pinned = pin.map(|f| (f, target));
        cx.notify();
    }

    /// The row to put at the top so `row` shows with `margin` rows above it: a row of a file's
    /// body also clears the sticky header (two rows), but no further up than the file's own
    /// header, which then shows instead.
    fn top_row_for(&self, row: u32, margin: u32) -> u32 {
        match self.built.segment_at(row) {
            Some(segment) if row > segment.start => row.saturating_sub(margin + STICKY_ROWS).max(segment.start),
            _ => row.saturating_sub(margin),
        }
    }

    fn glide_pixels(&mut self, delta: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.glide {
            Some(glide) => glide.rest += delta,
            None => {
                if palette(cx).instant_jumps {
                    self.list.scroll_by(delta);
                    self.pinned = None;
                    cx.notify();
                    return;
                }
                self.glide = Some(Glide { rest: delta });
                self.glide_step(window, cx);
            }
        }
    }

    fn glide_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(glide) = &mut self.glide else { return };
        let viewport = self.list.viewport_bounds();
        let height = viewport.size.height.max(px(100.));
        let distance = glide.rest;
        // At most ~6 screens a second, so the passing code stays readable.
        let cap = height / 10.;
        let magnitude = distance.abs().min(cap).min((distance.abs() * 0.35).max(px(6.)));
        let step = if distance < px(0.) { -magnitude } else { magnitude };
        let before = self.list.logical_scroll_top();
        self.list.scroll_by(step);
        glide.rest -= step;
        let after = self.list.logical_scroll_top();
        let stuck = before.item_ix == after.item_ix && before.offset_in_item == after.offset_in_item;
        self.pinned = None;
        if distance.abs() < px(1.) || stuck {
            self.glide = None;
            self.record_anchor_from(after);
        } else {
            cx.on_next_frame(window, |this, window, cx| this.glide_step(window, cx));
        }
        cx.notify();
    }

    // ---------- loading ----------

    /// A file's side-by-side rows, loading them when missing (None until they arrive).
    pub fn split_rows(&mut self, file: usize, cx: &mut Context<Self>) -> Option<Arc<Vec<SplitRow>>> {
        if let Some(rows) = self.split.get(&file) {
            return Some(rows.clone());
        }
        if let Some(rows) = self.backend.split_file_cached(&self.view_id, file) {
            self.split.insert(file, rows.clone());
            return Some(rows);
        }
        if self.split_loading.insert(file) {
            let view_id = self.view_id.clone();
            let task = self.backend.split_file(view_id.clone(), file);
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    if this.view_id != view_id {
                        return;
                    }
                    this.split_loading.remove(&file);
                    match result {
                        Ok(rows) => {
                            this.split.insert(file, rows);
                            cx.notify();
                        }
                        Err(err) => eprintln!("side-by-side rows of file {file}: {err}"),
                    }
                });
            })
            .detach();
        }
        None
    }

    /// Loads the side-by-side rows of the files around `file` in that mode, before they're drawn.
    pub fn prefetch_split(&mut self, file: usize, cx: &mut Context<Self>) {
        let lo = file.saturating_sub(3);
        let hi = (file + 4).min(self.built.segments.len());
        for f in lo..hi {
            if self.built.segments[f].mode == Mode::Split {
                let _ = self.split_rows(f, cx);
            }
        }
    }

    /// The unified row drawn at `offset` of a file.
    pub fn unified_at(&self, file: usize, offset: u32) -> UnifiedRow<'_> {
        let Some(map) = self.built.unified.get(&file) else { return UnifiedRow::None };
        let summary = &self.view.summary.files[file];
        match map.at(offset) {
            DisplayRow::Base { base } => UnifiedRow::Row(&self.view.rows[(summary.first_row + base) as usize]),
            DisplayRow::Line { o, n } => match self.head_lines.get(&file).and_then(|lines| lines.get(&n)) {
                Some(segs) => UnifiedRow::Line { o, n, segs: segs.clone() },
                None => UnifiedRow::Line { o, n, segs: Arc::new(Vec::new()) },
            },
            DisplayRow::Gap { gap, base } => UnifiedRow::Gap { gap, header: base.map(|b| &self.view.rows[(summary.first_row + b) as usize]) },
        }
    }

    /// Head line `n` and old line `o` of a unified row or a shown line.
    pub fn unified_lines(&self, file: usize, offset: u32) -> (Option<u32>, Option<u32>) {
        match self.unified_at(file, offset) {
            UnifiedRow::Row(row) => (row.n, row.o),
            UnifiedRow::Line { o, n, .. } => (Some(n), Some(o)),
            _ => (None, None),
        }
    }

    // ---------- comments: what goes where ----------

    fn is_hidden(&self, thread: &ReviewThread, pr_index: usize, cx: &App) -> bool {
        let s = self.s(cx);
        !self.revealed.contains(&thread.id)
            && (s.hidden_comment_prs.contains(&pr_key(&self.prs[pr_index]))
                || self.hidden_threads.contains(&thread.id)
                || settled_reason(thread).is_some())
    }

    /// Threads, drafts and the open composer of the PRs in the range, as placeable items.
    fn comment_items(&self, cx: &App) -> Vec<CommentItem> {
        let s = self.s(cx);
        let in_range = |i: usize| i >= self.lo && i <= self.hi;
        let mut items = Vec::new();
        for pr_threads in &s.threads {
            if !in_range(pr_threads.pr_index) {
                continue;
            }
            for thread in &pr_threads.threads {
                let anchor = if thread.file_level || thread.line.is_none() {
                    None
                } else {
                    Some(Anchor { pr: pr_threads.pr_index as u8, path: thread.path.clone(), side: thread.side, line: thread.line.unwrap() })
                };
                let hidden = self.is_hidden(thread, pr_threads.pr_index, cx).then(|| HiddenThread {
                    id: thread.id.clone(),
                    preview: thread_preview(thread),
                    settled: settled_reason(thread).is_some(),
                });
                items.push(CommentItem {
                    key: format!("thread:{}", thread.id),
                    anchor,
                    path: thread.path.clone(),
                    hidden,
                    tally: Some(settled_reason(thread).map(String::from).unwrap_or_else(|| "open".into())),
                    kind: CommentKind::Thread { pr_index: pr_threads.pr_index, thread: thread.clone() },
                });
            }
        }
        for shown in &s.drafts {
            let draft = &shown.draft;
            let pr_index = shown.pr_index as usize;
            if !in_range(pr_index) || !matches!(draft.kind, wispy_core::drafts::DraftKind::Line | wispy_core::drafts::DraftKind::File) {
                continue;
            }
            let path = shown.path.clone().or_else(|| draft.path.clone()).unwrap_or_default();
            let anchor = match (draft.kind == wispy_core::drafts::DraftKind::Line, shown.line, draft.side) {
                (true, Some(line), Some(side)) => Some(Anchor { pr: pr_index as u8, path: path.clone(), side, line }),
                _ => None,
            };
            items.push(CommentItem {
                key: format!("draft:{}", draft.id),
                anchor,
                path,
                hidden: None,
                tally: Some("draft".into()),
                kind: CommentKind::Draft(shown.clone()),
            });
        }
        if let Some((target, _)) = &self.composer {
            let anchor = match (target.file_kind, target.line, target.side) {
                (false, Some(line), Some(side)) => Some(Anchor { pr: target.pr_index as u8, path: target.path.clone(), side, line }),
                _ => None,
            };
            items.push(CommentItem { key: target.key.clone(), anchor, path: target.path.clone(), hidden: None, tally: None, kind: CommentKind::Composer });
        }
        items
    }

    /// Where a located line is drawn in a unified file: its row, or a shown gap line.
    fn unified_offset(&self, built: &Built, file: usize, location: &Location, anchor: Option<&Anchor>, line: Option<u32>) -> Option<u32> {
        let map = built.unified.get(&file)?;
        if let Some(u) = location.unified {
            return Some(map.from_base(u));
        }
        let head = line.or_else(|| anchor.filter(|a| a.side == Side::Right && a.pr as usize == self.hi).map(|a| a.line))?;
        map.from_new_line(head)
    }

    #[allow(clippy::type_complexity)]
    fn place_items(
        &self,
        built: &Built,
        items: &[CommentItem],
        _: &App,
    ) -> (Vec<Placed>, HashMap<(u32, bool), CommentMark>, HashMap<usize, HashMap<String, usize>>) {
        let anchors: Vec<Anchor> = {
            let mut unique: HashMap<String, Anchor> = HashMap::new();
            for item in items {
                if let Some(a) = &item.anchor {
                    unique.insert(anchor_key(a), a.clone());
                }
            }
            unique.into_values().collect()
        };
        let located: HashMap<String, Option<Location>> = match self.backend.locate_anchors(self.view_id.clone(), anchors.clone()) {
            Ok(found) => anchors.iter().map(anchor_key).zip(found).collect(),
            Err(_) => HashMap::new(),
        };
        let mut placed = Vec::new();
        let mut marks: HashMap<(u32, bool), CommentMark> = HashMap::new();
        let mut tallies: HashMap<usize, HashMap<String, usize>> = HashMap::new();
        for item in items {
            let (file, offset) = match &item.anchor {
                Some(anchor) => {
                    let Some(Some(location)) = located.get(&anchor_key(anchor)) else { continue };
                    let file = location.file as usize;
                    let segment = &built.segments[file];
                    let offset = match segment.mode {
                        Mode::Split => location.split,
                        Mode::Unified => self.unified_offset(built, file, location, Some(anchor), None),
                        // A collapsed file is just its header.
                        _ => Some(0),
                    };
                    (file, offset)
                }
                None => match file_index_for(&self.view.summary.files, &item.path) {
                    Some(file) => (file, None),
                    None => continue,
                },
            };
            let segment = &built.segments[file];
            if segment.mode == Mode::Hidden {
                continue;
            }
            let after = segment.start + offset.unwrap_or(0);
            if let Some(tally) = &item.tally {
                *tallies.entry(file).or_default().entry(tally.clone()).or_default() += 1;
                if segment.mode == Mode::Collapsed {
                    continue;
                }
            }
            if let Some(hidden) = &item.hidden {
                let right = segment.mode == Mode::Split && item.anchor.as_ref().is_some_and(|a| a.side == Side::Right);
                let mark = marks.entry((after, right)).or_insert_with(|| CommentMark { header: after == segment.start, right, settled: true, threads: Vec::new() });
                mark.threads.push(hidden.clone());
                mark.settled &= hidden.settled;
                continue;
            }
            placed.push(Placed { key: item.key.clone(), after });
        }
        (placed, marks, tallies)
    }

    pub fn hide_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        self.revealed.remove(id);
        self.hidden_threads.shift_remove(id);
        self.hidden_threads.insert(id.to_string());
        self.save_hidden_threads(cx);
        self.relayout(cx);
    }

    pub fn show_threads(&mut self, ids: &[String], cx: &mut Context<Self>) {
        for id in ids {
            self.revealed.insert(id.clone());
            self.hidden_threads.shift_remove(id);
        }
        self.save_hidden_threads(cx);
        self.relayout(cx);
    }

    fn save_hidden_threads(&self, cx: &App) {
        let list: Vec<String> = self.hidden_threads.iter().cloned().collect();
        let keep = list.len().saturating_sub(500);
        settings::set_list(cx, "hiddenThreads", list[keep..].to_vec());
    }

    // ---------- comment targets ----------

    /// The comment target for a row side (None for fillers, headers, gap rows).
    pub fn target_at(&mut self, file: usize, mode: Mode, offset: u32, new_side: bool, cx: &mut Context<Self>) -> Option<Target> {
        let summary = self.view.summary.files[file].clone();
        let anchor = match mode {
            Mode::Unified => match self.unified_at(file, offset) {
                UnifiedRow::Row(row) => unified_anchor(row, &summary, self.hi),
                UnifiedRow::Line { n, .. } => Some(Anchor { pr: self.hi as u8, side: Side::Right, line: n, path: summary.path.clone() }),
                _ => None,
            },
            Mode::Split => {
                let rows = self.split_rows(file, cx)?;
                let row = rows.get(offset.checked_sub(1)? as usize)?;
                split_anchor(row, &summary, self.hi, new_side)
            }
            _ => None,
        }?;
        Some(Target { file, mode, offset, anchor })
    }

    /// The new-side text of rows `start..=end` (None unless that's exactly `count` lines).
    fn head_text(&mut self, start: &Target, end: &Target, count: u32, cx: &mut Context<Self>) -> Option<String> {
        let mut lines = Vec::new();
        for offset in start.offset..=end.offset {
            let (kind, text) = match start.mode {
                Mode::Unified => match self.unified_at(start.file, offset) {
                    UnifiedRow::Row(row) => (row.k, seg_text(&row.s)),
                    UnifiedRow::Line { segs, .. } => (row_kind::CONTEXT, seg_text(&segs)),
                    _ => return None,
                },
                _ => {
                    let rows = self.split_rows(start.file, cx)?;
                    let row = rows.get(offset.checked_sub(1)? as usize)?;
                    (row.nk, seg_text(&row.ns))
                }
            };
            if kind == row_kind::ADDED || kind == row_kind::CONTEXT {
                lines.push(text);
            }
        }
        (lines.len() as u32 == count).then(|| lines.join("\n"))
    }

    /// A file opened for a look stays open once it's worked with (commented on, s, e).
    fn keep_open(&mut self, file: usize) {
        let path = self.view.summary.files[file].path.clone();
        if self.temporary.as_deref() != Some(path.as_str()) {
            return;
        }
        self.collapsed.remove(&path);
        self.expanded.insert(path);
        self.temporary = None;
    }

    pub fn open_composer(&mut self, start: Target, end: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_open(end.file);
        let (start_file, start_mode, start_offset) = (start.file, start.mode, start.offset);
        let same = start.anchor.pr == end.anchor.pr && start.anchor.side == end.anchor.side && start.anchor.path == end.anchor.path;
        let from = if same { start } else { end.clone() };
        let (a, b) = (from.anchor.line.min(end.anchor.line), from.anchor.line.max(end.anchor.line));
        let suggestion = if end.anchor.side == Side::Right { self.head_text(&from, &end, b - a + 1, cx) } else { None };
        let target = ComposerTarget {
            key: format!("composer:{}", anchor_key(&end.anchor)),
            file_kind: false,
            pr_index: end.anchor.pr as usize,
            path: end.anchor.path.clone(),
            side: Some(end.anchor.side),
            line: Some(b),
            start_line: (a != b).then_some(a),
            fallback: None,
        };
        let title = format!("Comment on #{} · {} {}", self.prs[target.pr_index].number, target.path, line_label(target.start_line, target.line));
        self.set_composer(target.clone(), title, suggestion, window, cx);
        let rows = if same && start_file == end.file && start_mode == end.mode {
            RowRange { file: end.file, mode: end.mode, from: start_offset.min(end.offset), to: start_offset.max(end.offset) }
        } else {
            RowRange { file: end.file, mode: end.mode, from: end.offset, to: end.offset }
        };
        self.composer_rows = Some((rows, end.anchor.side == Side::Right));
        self.selection = None;
        let task = self.backend.accepts_line_comment(self.stack_id.clone(), target.pr_index, target.path.clone(), end.anchor.side, a, b);
        let key = target.key.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(accepted) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some((t, composer)) = &mut this.composer {
                        if t.key == key {
                            t.fallback = Some(!accepted);
                            composer.update(cx, |c, cx| c.set_fallback(Some(!accepted), cx));
                        }
                    }
                });
            }
        })
        .detach();
    }

    pub fn open_file_composer(&mut self, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_open(file);
        let summary = &self.view.summary.files[file];
        let pr = summary.prs.iter().filter(|p| (**p as usize) >= self.lo && (**p as usize) <= self.hi).max().map_or(self.hi, |p| *p as usize);
        let path = path_in_pr(summary, pr as u8, Side::Right).to_string();
        let target = ComposerTarget {
            key: format!("composer:file:{}", summary.path),
            file_kind: true,
            pr_index: pr,
            path: path.clone(),
            side: None,
            line: None,
            start_line: None,
            fallback: Some(false),
        };
        let title = format!("Comment on #{} · {path}", self.prs[pr].number);
        self.set_composer(target, title, None, window, cx);
    }

    fn set_composer(&mut self, target: ComposerTarget, title: String, suggestion: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (stack_id, pr_index, path) = (self.stack_id.clone(), target.pr_index, target.path.clone());
        let backend = self.backend.clone();
        let fallback = target.fallback;
        let composer = cx.new(|cx| {
            Composer::new(title, String::new(), "Save draft", fallback, suggestion, Some((backend, stack_id, pr_index, path)), window, cx)
        });
        cx.subscribe_in(&composer, window, |this, _, event: &ComposerEvent, window, cx| match event {
            ComposerEvent::Save(body) => {
                let Some((target, _)) = this.composer.take() else { return };
                let draft = wispy_core::service::NewDraft {
                    pr_index: target.pr_index,
                    kind: if target.file_kind { wispy_core::drafts::DraftKind::File } else { wispy_core::drafts::DraftKind::Line },
                    path: Some(target.path),
                    side: target.side,
                    line: target.line,
                    start_line: target.start_line,
                    body: body.clone(),
                    thread_id: None,
                    reply_to: None,
                    assistant: false,
                };
                this.save_draft(draft, cx);
                window.focus(&this.focus, cx);
                this.relayout(cx);
            }
            ComposerEvent::Cancel => {
                this.composer = None;
                window.focus(&this.focus, cx);
                this.relayout(cx);
            }
            ComposerEvent::Resized => this.remeasure_insert_key(&this.composer_key(), cx),
        })
        .detach();
        self.composer = Some((target, composer));
        self.relayout(cx);
    }

    fn composer_key(&self) -> String {
        self.composer.as_ref().map(|(t, _)| t.key.clone()).unwrap_or_default()
    }

    /// An inserted item's height changed (typing, a reply box opened).
    pub fn remeasure_insert_key(&self, key: &str, cx: &mut Context<Self>) {
        for (index, item) in self.built.items.iter().enumerate() {
            if let Item::Insert(i) = item {
                if self.built.inserts[*i as usize].key == key {
                    self.list.remeasure_items(index..index + 1);
                }
            }
        }
        cx.notify();
    }

    pub fn save_draft(&mut self, draft: wispy_core::service::NewDraft, cx: &mut Context<Self>) {
        let task = self.backend.create_draft(self.stack_id.clone(), draft);
        let session = self.session.clone();
        cx.spawn(async move |_, cx| {
            match task.await {
                Ok(_) => {
                    let _ = session.update(cx, |s, cx| s.refresh_drafts(cx));
                }
                Err(err) => eprintln!("saving the draft failed: {err}"),
            }
        })
        .detach();
    }

    pub fn edit_draft(&mut self, id: String, body: String, cx: &mut Context<Self>) {
        let task = self.backend.update_draft(id, Some(body), None);
        let session = self.session.clone();
        cx.spawn(async move |_, cx| {
            if task.await.is_ok() {
                let _ = session.update(cx, |s, cx| s.refresh_drafts(cx));
            }
        })
        .detach();
    }

    pub fn delete_draft(&mut self, id: String, cx: &mut Context<Self>) {
        let task = self.backend.delete_draft(id);
        let session = self.session.clone();
        cx.spawn(async move |_, cx| {
            if task.await.is_ok() {
                let _ = session.update(cx, |s, cx| s.refresh_drafts(cx));
            }
        })
        .detach();
    }

    /// Gutter interactions: press = start, enter = extend, release = comment on the range.
    pub fn on_gutter(&mut self, event: GutterEvent, file: usize, mode: Mode, offset: u32, new_side: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.target_at(file, mode, offset, new_side, cx);
        match event {
            GutterEvent::Hover => self.hovered = target,
            GutterEvent::Down => {
                if target.is_some() {
                    self.selection = Some(RowRange { file, mode, from: offset, to: offset });
                }
                self.dragging = target;
                cx.notify();
            }
            GutterEvent::Enter => {
                if let (Some(start), Some(_)) = (&self.dragging, &target) {
                    if start.file == file && start.mode == mode {
                        self.selection = Some(RowRange { file, mode, from: start.offset.min(offset), to: start.offset.max(offset) });
                        cx.notify();
                    }
                }
            }
            GutterEvent::Up => {
                let Some(start) = self.dragging.take() else { return };
                let Some(target) = target else { return };
                if start.file != file || start.mode != mode {
                    return;
                }
                let (first, last) = if start.offset <= offset { (start, target) } else { (target, start) };
                self.open_composer(first, last, window, cx);
            }
        }
    }

    // ---------- gaps ----------

    /// Shows `next` of a file's gaps once their lines are loaded, keeping a line in view where it is
    /// on screen (the row of gap `pin` when one was clicked).
    pub fn change_gaps(&mut self, file: usize, next: Option<Vec<[u32; 2]>>, pin: Option<usize>, cx: &mut Context<Self>) {
        let request = self.gap_requests.get(&file).copied().unwrap_or(0) + 1;
        self.gap_requests.insert(file, request);
        let info = self.view.summary.files[file].clone();
        let mut wanted: Vec<(u32, u32)> = Vec::new();
        for (i, gap) in info.gaps.iter().enumerate() {
            let [down, up] = next.as_ref().and_then(|n| n.get(i)).copied().unwrap_or([0, 0]);
            let end = gap.new_start + gap.len - 1;
            if down + up >= gap.len {
                wanted.push((gap.new_start, end));
            } else {
                if down > 0 {
                    wanted.push((gap.new_start, gap.new_start + down - 1));
                }
                if up > 0 {
                    wanted.push((end - up + 1, end));
                }
            }
        }
        let have = self.head_lines.get(&file);
        wanted.retain(|(a, b)| !(have.is_some_and(|h| (*a..=*b).all(|n| h.contains_key(&n)))));
        let tasks: Vec<_> = wanted.iter().map(|(a, b)| (*a, self.backend.head_lines(self.view_id.clone(), file, *a, *b))).collect();
        let view_id = self.view_id.clone();
        cx.spawn(async move |this, cx| {
            let mut loaded = Vec::new();
            for (start, task) in tasks {
                match task.await {
                    Ok(lines) => loaded.push((start, lines)),
                    Err(err) => eprintln!("loading lines failed: {err}"),
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.view_id != view_id || this.gap_requests.get(&file) != Some(&request) {
                    return;
                }
                let cache = this.head_lines.entry(file).or_default();
                for (start, lines) in loaded {
                    for (i, segs) in lines.into_iter().enumerate() {
                        cache.insert(start + i as u32, Arc::new(segs));
                    }
                }
                this.apply_gaps(file, next, pin, cx);
            });
        })
        .detach();
    }

    fn apply_gaps(&mut self, file: usize, next: Option<Vec<[u32; 2]>>, pin: Option<usize>, cx: &mut Context<Self>) {
        self.change_layout(cx, |this, _| {
            this.keep_open(file);
            if this.built.segments.get(file).map(|s| s.mode) == Some(Mode::Unified) {
                let info = this.view.summary.files[file].clone();
                let before = this.built.unified[&file].clone();
                let after = UnifiedMap::new(&info, next.as_deref());
                let opening = after.rows >= before.rows;
                let start = this.built.segments[file].start;
                let top = this.list.logical_scroll_top();
                let top_row = this.built.row_of_item(top.item_ix);
                let mut kept: Option<(u32, u32)> = None;
                if let Some(pin) = pin {
                    if let Some(pinned) = before.gap_row(pin) {
                        let gap = &info.gaps[pin];
                        let first_hidden = gap.new_start + this.reveals.get(&file).and_then(|r| r.get(pin)).map_or(0, |r| r[0]);
                        if let Some(offset) = after.gap_row(pin).or_else(|| after.from_new_line(first_hidden)) {
                            kept = Some((start + pinned, offset));
                        }
                    }
                }
                let last = top_row + 60;
                // From the first row fully in view.
                let mut row = if top.offset_in_item > px(0.) && matches!(this.built.items.get(top.item_ix), Some(Item::Row(_))) { top_row + 1 } else { top_row };
                while kept.is_none() && row <= last && row < this.built.total_rows {
                    let segment = this.built.segment_at(row).cloned();
                    if let Some(segment) = segment {
                        if segment.file != file {
                            kept = Some((row, row - segment.start));
                        } else {
                            match before.at(row - segment.start) {
                                DisplayRow::Base { base } => kept = Some((row, after.from_base(base))),
                                DisplayRow::Line { n, .. } if opening => {
                                    if let Some(offset) = after.from_new_line(n) {
                                        kept = Some((row, offset));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    row += 1;
                }
                if kept.is_none() {
                    // Inside a gap that's closing (or a gap row that goes): where the gap is now.
                    let info = &this.view.summary.files[file];
                    let reveal = this.reveals.get(&file);
                    let gap = match before.at(top_row.saturating_sub(start)) {
                        DisplayRow::Gap { gap, .. } => Some((gap, None)),
                        DisplayRow::Line { n, .. } => info.gaps.iter().position(|g| n >= g.new_start && n < g.new_start + g.len).map(|g| (g, Some(n))),
                        _ => None,
                    };
                    if let Some((index, line)) = gap {
                        let g = &info.gaps[index];
                        let first_hidden = g.new_start + reveal.and_then(|r| r.get(index)).map_or(0, |r| r[0]);
                        let offset = after
                            .from_new_line(line.unwrap_or(first_hidden))
                            .unwrap_or(if g.offset < info.row_count { after.from_base(g.offset) } else { after.rows.saturating_sub(1) });
                        kept = Some((top_row, offset));
                    }
                }
                if let Some((row, offset)) = kept {
                    if let Some(segment) = this.built.segment_at(row) {
                        let delta = this.row_delta(row, top_row, top);
                        this.anchor = Some(Place { path: this.view.summary.files[segment.file].path.clone(), offset, mode: segment.mode, delta });
                    }
                }
            }
            match next {
                Some(next) => {
                    this.reveals.insert(file, next);
                }
                None => {
                    this.reveals.remove(&file);
                }
            }
        });
    }

    /// A click on a gap row: `up` shows lines above the hunk below it, `down` below the hunk above.
    pub fn expand_gap(&mut self, file: usize, index: usize, how: GapExpand, cx: &mut Context<Self>) {
        let info = self.view.summary.files[file].clone();
        let gap = &info.gaps[index];
        let current = self.reveals.get(&file).cloned();
        let hidden = hidden_lines(gap, current.as_ref().and_then(|r| r.get(index)));
        let all = how == GapExpand::All || hidden <= GAP_STEP;
        let up = gap.offset < info.row_count;
        let down = gap.new_start > 1;
        let next = if all {
            reveal_more(&info, current.as_deref(), index, false, u32::MAX)
        } else if how == GapExpand::Row && up && down {
            let half = reveal_more(&info, current.as_deref(), index, true, GAP_STEP / 2);
            reveal_more(&info, Some(&half), index, false, GAP_STEP / 2)
        } else {
            reveal_more(&info, current.as_deref(), index, how == GapExpand::Down || (how == GapExpand::Row && !up), GAP_STEP)
        };
        // The clicked row stays under the pointer (a second click of a double click lands on it).
        self.change_gaps(file, Some(next), if how == GapExpand::Down { None } else { Some(index) }, cx);
    }

    // ---------- performance scripts ----------

    /// `S`, for the performance script.
    pub fn perf_toggle_all_modes(&mut self, cx: &mut Context<Self>) {
        self.toggle_all_modes(cx);
    }

    /// The file with the most rows (the fixture's 20k-line file).
    pub fn largest_file(&self) -> usize {
        let files = &self.view.summary.files;
        (0..files.len()).max_by_key(|&f| files[f].row_count.max(files[f].split_rows)).unwrap_or(0)
    }

    /// Shows `file` (expanded, in `mode`) at the top, or the top of the diff.
    pub fn perf_prepare_scroll(&mut self, file: Option<(usize, Mode)>, cx: &mut Context<Self>) {
        match file {
            Some((index, mode)) => {
                let path = self.view.summary.files[index].path.clone();
                self.change_layout(cx, |this, _| {
                    this.collapsed.remove(&path);
                    this.expanded.insert(path.clone());
                    this.overrides.insert(path, if mode == Mode::Split { BaseMode::Split } else { BaseMode::Unified });
                });
                let start = self.built.segments[index].start;
                self.scroll_to_row(start, 0, Some(index), cx);
            }
            None => self.scroll_to_row(0, 0, None, cx),
        }
    }

    // ---------- moving between modes ----------

    /// Where the reader's place goes when files switch modes: the first line in view that's in both
    /// keeps its place (matched by head line).
    fn place_across_modes(&mut self, mode_after: impl Fn(usize) -> Option<Mode>, cx: &mut Context<Self>) -> Option<Place> {
        let top = self.list.logical_scroll_top();
        let first_row = self.built.row_of_item(top.item_ix);
        // From the first row fully in view.
        let start = if top.offset_in_item > px(0.) && matches!(self.built.items.get(top.item_ix), Some(Item::Row(_))) { first_row + 1 } else { first_row };
        for row in start..(start + 60).min(self.built.total_rows) {
            let segment = self.built.segment_at(row)?.clone();
            let offset = row - segment.start;
            let delta = self.row_delta(row, first_row, top);
            let to_mode = if matches!(segment.mode, Mode::Unified | Mode::Split) { mode_after(segment.file) } else { None };
            let path = self.view.summary.files[segment.file].path.clone();
            match to_mode {
                None => return Some(Place { path, offset, mode: segment.mode, delta }),
                Some(m) if m == segment.mode => return Some(Place { path, offset, mode: segment.mode, delta }),
                Some(m) if offset == 0 => return Some(Place { path, offset: 0, mode: m, delta }),
                Some(m) => {
                    let line = match segment.mode {
                        Mode::Split => self.split_rows(segment.file, cx).and_then(|r| r.get(offset as usize - 1).and_then(|r| r.n)),
                        _ => self.unified_lines(segment.file, offset).0,
                    };
                    let Some(line) = line else { continue };
                    let Ok(Some(location)) = self.backend.locate_line(self.view_id.clone(), segment.file, line) else { continue };
                    let to = match m {
                        Mode::Split => location.split,
                        _ => {
                            let map = UnifiedMap::new(&self.view.summary.files[segment.file], self.reveals.get(&segment.file).map(Vec::as_slice));
                            match location.unified {
                                Some(u) => Some(map.from_base(u)),
                                None => map.from_new_line(line).or_else(|| {
                                    // A line the unified diff leaves out: the row standing for its gap.
                                    let info = &self.view.summary.files[segment.file];
                                    let gap = info.gaps.iter().find(|g| line >= g.new_start && line < g.new_start + g.len)?;
                                    Some(if gap.offset < info.row_count { map.from_base(gap.offset) } else { map.rows - 1 })
                                }),
                            }
                        }
                    };
                    if let Some(to) = to {
                        return Some(Place { path, offset: to, mode: m, delta });
                    }
                }
            }
        }
        None
    }

    fn toggle_file_mode(&mut self, file: usize, cx: &mut Context<Self>) {
        let segment = self.built.segments[file].clone();
        let path = self.view.summary.files[file].path.clone();
        if segment.mode == Mode::Collapsed {
            self.keep_file = Some(file);
            self.change_layout(cx, |this, _| {
                this.expanded.insert(path.clone());
                this.collapsed.remove(&path);
            });
            return;
        }
        let next = if segment.mode == Mode::Split { BaseMode::Unified } else { BaseMode::Split };
        let next_mode = if next == BaseMode::Split { Mode::Split } else { Mode::Unified };
        if next == BaseMode::Split && self.split_rows(file, cx).is_none() {
            // Wait for the rows: the place across modes needs them.
            self.after_split_load(file, cx, move |this, cx| this.toggle_file_mode(file, cx));
            return;
        }
        let place = self.place_across_modes(|f| (f == file).then_some(next_mode), cx);
        self.change_layout(cx, |this, _| {
            this.keep_file = Some(file);
            if let Some(place) = place {
                this.anchor = Some(place);
            }
            this.keep_open(file);
            this.overrides.insert(path, next);
        });
    }

    /// Runs `then` once a file's side-by-side rows are loaded.
    fn after_split_load(&mut self, file: usize, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static) {
        let task = self.backend.split_file(self.view_id.clone(), file);
        let view_id = self.view_id.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.view_id != view_id {
                    return;
                }
                if let Ok(rows) = result {
                    this.split.insert(file, rows);
                    then(this, cx);
                }
            });
        })
        .detach();
    }

    fn toggle_all_modes(&mut self, cx: &mut Context<Self>) {
        let next = if self.s(cx).default_mode == BaseMode::Split { BaseMode::Unified } else { BaseMode::Split };
        // The file in view needs its side-by-side rows to keep the place.
        if next == BaseMode::Split {
            if let Some(file) = self.current() {
                if self.built.segments[file].mode == Mode::Unified && self.view.summary.files[file].split_rows > 0 && self.split_rows(file, cx).is_none() {
                    self.after_split_load(file, cx, |this, cx| this.toggle_all_modes(cx));
                    return;
                }
            }
        }
        let files = self.view.summary.files.clone();
        let place = self.place_across_modes(
            |f| Some(if next == BaseMode::Split && files[f].split_rows > 0 { Mode::Split } else { Mode::Unified }),
            cx,
        );
        self.change_layout(cx, |this, cx| {
            if let Some(place) = place {
                this.anchor = Some(place);
            }
            this.overrides.clear();
            this.session.update(cx, |s, cx| s.set_default_mode(next, cx));
        });
    }

    // ---------- moving between files ----------

    /// Puts file `index`'s header at the top (file list, n/p, ⇧↓/⇧↑, file search). With `open`, a
    /// collapsed file opens for a look; a file opened that way collapses again once another is shown.
    pub fn go_to_file(&mut self, index: usize, open: bool, cx: &mut Context<Self>) {
        let path = self.view.summary.files[index].path.clone();
        let opens = open && self.built.segments[index].mode == Mode::Collapsed;
        let closes = self.temporary.as_ref().is_some_and(|t| *t != path);
        if !opens && !closes {
            let start = self.built.segments[index].start;
            self.scroll_to_row(start, 0, Some(index), cx);
            return;
        }
        self.change_layout(cx, |this, _| {
            this.reveal_file = Some(index);
            this.temporary = opens.then_some(path);
        });
    }

    /// The next / previous file start (`unviewed`: skipping viewed files) from the top.
    fn file_step(&self, down: bool, unviewed: bool, cx: &App) -> Option<usize> {
        let current = self.current()?;
        let pinned_here = self.pinned.is_some_and(|(f, _)| f == current);
        let top_row = if pinned_here { self.built.segments[current].start } else { self.top_row() };
        let s = self.s(cx);
        let shown = self.built.segments.iter().filter(|s| s.mode != Mode::Hidden).filter(|seg| !unviewed || !s.is_viewed(&self.view.summary.files[seg.file].content_key));
        if down {
            shown.clone().find(|seg| seg.start > top_row).map(|s| s.file)
        } else {
            shown.filter(|seg| seg.start < top_row).last().map(|s| s.file)
        }
    }

    // ---------- the file filter ----------

    pub fn change_filter(&mut self, next: FileFilter, cx: &mut Context<Self>) {
        let key = self.filter_key();
        self.change_layout(cx, |this, cx| {
            this.filters.shift_remove(&key);
            if !next.exts.is_empty() {
                this.filters.insert(key, next.to_value());
            }
            let excess = this.filters.len().saturating_sub(300);
            let keys: Vec<String> = this.filters.keys().take(excess).cloned().collect();
            for k in keys {
                this.filters.shift_remove(&k);
            }
            settings::set_record(cx, "fileFilters", this.filters.clone());
        });
        self.sync_assistant(cx);
    }

    // ---------- the file tree ----------

    /// A directory folds once every file in it is viewed.
    fn update_tree_done(&mut self, cx: &App) {
        let filtered = self.filtered();
        let paths: Vec<Option<String>> = self.view.summary.files.iter().enumerate().map(|(i, f)| (!filtered[i]).then(|| f.path.clone())).collect();
        let tree = crate::logic::file_tree::build_tree(&paths);
        let s = self.s(cx);
        let done: HashSet<String> = crate::logic::file_tree::all_dirs(&tree)
            .into_iter()
            .filter(|d| d.files.iter().all(|&i| s.is_viewed(&self.view.summary.files[i].content_key)))
            .map(|d| d.path.clone())
            .collect();
        for d in done.difference(&self.tree_done) {
            self.tree_closed.insert(d.clone());
        }
        self.tree_done = done;
    }

    // ---------- jumps ----------

    /// Before a jump: remember where the reader was, for going back.
    pub fn remember(&mut self, cx: &mut Context<Self>) {
        if let Some(place) = self.here(cx) {
            self.history.push(place);
        }
    }

    fn here(&mut self, cx: &mut Context<Self>) -> Option<HistoryPlace> {
        if let Some(view) = &self.file_view {
            return Some(HistoryPlace::File { path: view.path.clone(), line: view.line, end: view.end });
        }
        let top = self.list.logical_scroll_top();
        let row = self.built.row_of_item(top.item_ix);
        let segment = self.built.segment_at(row)?.clone();
        let mut line = None;
        for r in row..(row + 40).min(segment.start + segment.rows) {
            let offset = r - segment.start;
            let n = match segment.mode {
                Mode::Split => self.split_rows(segment.file, cx).and_then(|rows| rows.get((offset as usize).checked_sub(1)?).and_then(|r| r.n)),
                Mode::Unified => self.unified_lines(segment.file, offset).0,
                _ => None,
            };
            if let Some(n) = n {
                line = Some((n, u_px(20., cx) * (r - row) as f32 - top.offset_in_item));
                break;
            }
        }
        Some(HistoryPlace::Diff {
            place_path: self.view.summary.files[segment.file].path.clone(),
            offset: row - segment.start,
            mode: segment.mode,
            delta: top.offset_in_item,
            line,
        })
    }

    fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(now) = self.here(cx) else { return };
        // Whether a remembered place can still be shown (a diff file the filter hides now can't).
        let shown: HashSet<String> = self
            .view
            .summary
            .files
            .iter()
            .enumerate()
            .filter(|(i, _)| self.built.segments[*i].mode != Mode::Hidden)
            .map(|(_, f)| f.path.clone())
            .collect();
        let usable = |p: &HistoryPlace| match p {
            HistoryPlace::File { .. } => true,
            HistoryPlace::Diff { place_path, .. } => shown.contains(place_path),
        };
        let place = if back { self.history.go_back(now, usable) } else { self.history.go_forward(now, usable) };
        if let Some(place) = place {
            self.restore_place(place, window, cx);
        }
    }

    /// Back (`true`) / forward through the places jumps left.
    pub fn go_history(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(back, window, cx);
    }

    fn restore_place(&mut self, place: HistoryPlace, window: &mut Window, cx: &mut Context<Self>) {
        match place {
            HistoryPlace::File { path, line, end } => {
                let filtered = self.view.summary.files.iter().position(|f| f.path == path).is_some_and(|i| self.built.segments[i].mode == Mode::Hidden);
                self.open_file_view(path, line, end, filtered, window, cx);
            }
            HistoryPlace::Diff { place_path, offset, mode, delta, line } => {
                let Some(index) = self.view.summary.files.iter().position(|f| f.path == place_path) else { return };
                self.file_view = None;
                let segment = self.built.segments[index].clone();
                // A collapsed file opens for a look, as when it was reached.
                if segment.mode == Mode::Collapsed {
                    self.go_to_file(index, true, cx);
                    return;
                }
                let mut target = ListOffset { item_ix: self.built.item_of_row(segment.start), offset_in_item: px(0.) };
                if segment.mode == mode {
                    target = ListOffset { item_ix: self.built.item_of_row(segment.start + offset.min(segment.rows - 1)), offset_in_item: delta };
                } else if let Some((n, y)) = line {
                    // The file switched modes since: find the line again.
                    if let Ok(Some(location)) = self.backend.locate_line(self.view_id.clone(), index, n) {
                        let to = match segment.mode {
                            Mode::Split => location.split,
                            _ => self.unified_offset(&self.built, index, &location, None, Some(n)),
                        };
                        if let Some(to) = to {
                            let rows_above = (y / u_px(20., cx)).floor().max(0.) as u32;
                            target = ListOffset { item_ix: self.built.item_of_row((segment.start + to).saturating_sub(rows_above)), offset_in_item: px(0.) };
                        }
                    }
                }
                if self.temporary.as_ref().is_some_and(|t| *t != place_path) {
                    // The file opened for a look collapses again, as when another file is shown.
                    let row = self.built.row_of_item(target.item_ix);
                    self.change_layout(cx, |this, _| {
                        this.anchor = Some(Place { path: place_path.clone(), offset: row - segment.start, mode: segment.mode, delta: target.offset_in_item });
                        this.temporary = None;
                    });
                    return;
                }
                self.glide = None;
                self.list.scroll_to(target);
                self.record_anchor_from(target);
                self.pinned = None;
                cx.notify();
            }
        }
    }

    /// Opens a head file read-only (files outside the diff, or hidden by the filter).
    pub fn open_file_view(&mut self, path: String, line: u32, end: u32, filtered: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.file_view = Some(FileViewState::new(path.clone(), line, end, filtered));
        // Escape (and the other keys) reach the diff, which closes the file view first.
        window.focus(&self.focus, cx);
        let task = self.backend.read_file(self.stack_id.clone(), self.hi, path.clone());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = &mut this.file_view {
                    if view.path == path {
                        match result {
                            Ok(lines) => view.set_lines(lines),
                            Err(err) => view.error = Some(err),
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Jumps to a usage / search hit / file reference: in the diff when the file is in it (and
    /// shown), else in the file view.
    pub fn jump(&mut self, path: &str, line: u32, end: Option<u32>, file: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.remember(cx);
        let file = file.or_else(|| self.view.summary.files.iter().position(|f| f.path == path));
        if let Some(file) = file {
            if self.view.summary.files[file].new_blob.is_some() && self.built.segments[file].mode != Mode::Hidden {
                self.jump_to_line(file, line, end.unwrap_or(line), cx);
                return;
            }
        }
        let filtered = file.is_some_and(|f| self.built.segments[f].mode == Mode::Hidden);
        self.open_file_view(path.to_string(), line, end.unwrap_or(line), filtered, window, cx);
    }

    /// Scrolls to head line `line` of file `file` (switching it to side by side if only the full
    /// file has it) and flashes the rows up to head line `end`.
    pub fn jump_to_line(&mut self, file: usize, line: u32, end: u32, cx: &mut Context<Self>) {
        let Ok(Some(location)) = self.backend.locate_line(self.view_id.clone(), file, line) else { return };
        let last = if end > line { self.backend.locate_line(self.view_id.clone(), file, end).ok().flatten() } else { None };
        let segment = self.built.segments[file].clone();
        let path = self.view.summary.files[file].path.clone();
        let shown = self.unified_offset(&self.built, file, &location, None, Some(line));
        let needs_split = segment.mode == Mode::Collapsed || (segment.mode == Mode::Unified && shown.is_none());
        if needs_split {
            self.pending_jump = Some((file, line, end));
            let to_split = shown.is_none();
            self.change_layout(cx, |this, _| {
                this.expanded.insert(path.clone());
                this.collapsed.remove(&path);
                if to_split {
                    this.overrides.insert(path, BaseMode::Split);
                }
            });
            return;
        }
        let offset = (if segment.mode == Mode::Split { location.split } else { shown }).unwrap_or(0);
        let to = last
            .and_then(|l| if segment.mode == Mode::Split { l.split } else { self.unified_offset(&self.built, file, &l, None, Some(end)) })
            .unwrap_or(offset);
        self.glide = None;
        // A third of a screen above it.
        let rows_above = ((self.list.viewport_bounds().size.height / 3.) / u_px(20., cx)).floor().max(0.) as u32;
        self.scroll_to_row(segment.start + offset, rows_above, None, cx);
        self.set_flash(RowRange { file, mode: segment.mode, from: offset, to: offset.max(to) }, cx);
    }

    fn set_flash(&mut self, range: RowRange, cx: &mut Context<Self>) {
        self.flash = Some(range);
        self.flash_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1400)).await;
            let _ = this.update(cx, |this, cx| {
                this.flash = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    // ---------- panels ----------

    pub fn open_usages(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let clean = name.trim_start_matches('$').to_string();
        if clean.is_empty() {
            return;
        }
        crate::perf_marks::mark("usages:start");
        self.assistant = None;
        assistant::forget_open(&self.session.clone(), &self.prs, cx);
        let panel = self.ensure_code_panel(window, cx);
        panel.update(cx, |p, cx| p.show_usages(clean, window, cx));
    }

    pub fn open_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.assistant = None;
        assistant::forget_open(&self.session.clone(), &self.prs, cx);
        let panel = self.ensure_code_panel(window, cx);
        panel.update(cx, |p, cx| p.show_search(query, window, cx));
    }

    fn ensure_code_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<CodePanel> {
        if let Some(Panel::Code(panel)) = &self.panel {
            return panel.clone();
        }
        let (backend, view_id, stack_id, hi) = (self.backend.clone(), self.view_id.clone(), self.stack_id.clone(), self.hi);
        let panel = cx.new(|cx| CodePanel::new(backend, view_id, stack_id, hi, window, cx));
        cx.subscribe_in(&panel, window, |this, _, event: &CodePanelEvent, window, cx| match event {
            CodePanelEvent::Jump { path, line, file } => {
                let (path, line, file) = (path.clone(), *line, *file);
                this.jump(&path, line, None, file, window, cx);
            }
            CodePanelEvent::Close => {
                this.panel = None;
                window.focus(&this.focus, cx);
                cx.notify();
            }
            CodePanelEvent::Resized(width) => {
                this.code_panel_width = *width;
                cx.notify();
            }
        })
        .detach();
        self.panel = Some(Panel::Code(panel.clone()));
        cx.notify();
        panel
    }

    pub fn code_panel(&self) -> Option<&Entity<CodePanel>> {
        match &self.panel {
            Some(Panel::Code(p)) => Some(p),
            None => None,
        }
    }

    fn open_file_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.view.summary.files.clone();
        let filtered = self.filtered();
        let viewed: Vec<bool> = files.iter().map(|f| self.is_viewed(&f.content_key, cx)).collect();
        let search = cx.new(|cx| FileSearch::new(files, filtered, viewed, window, cx));
        cx.subscribe_in(&search, window, |this, _, event: &FileSearchEvent, window, cx| {
            this.file_search = None;
            window.focus(&this.focus, cx);
            if let FileSearchEvent::Pick(index) = event {
                let index = *index;
                // Files the filter hides open in the file view, like usages in them.
                if this.built.segments[index].mode == Mode::Hidden {
                    let path = this.view.summary.files[index].path.clone();
                    this.jump(&path, 1, None, None, window, cx);
                } else {
                    this.remember(cx);
                    this.go_to_file(index, true, cx);
                }
            }
            cx.notify();
        })
        .detach();
        self.file_search = Some(search);
        cx.notify();
    }

    pub fn file_search(&self) -> Option<&Entity<FileSearch>> {
        self.file_search.as_ref()
    }

    pub fn open_assistant(&mut self, thread: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = None;
        let context = if thread.is_some() { assistant::AskContext::none(self.range_label()) } else { self.selection_context(cx) };
        let Some(panel) = self.assistant_panel.clone() else { return };
        assistant::open(self, &panel, context, thread, window, cx);
        self.assistant = Some(panel);
        cx.notify();
    }

    /// The assistant asks about what's shown (range, files, the filter).
    fn sync_assistant(&self, cx: &mut App) {
        if let Some(panel) = &self.assistant_panel {
            assistant::sync_view(self, panel, cx);
        }
    }

    pub fn range_label(&self) -> String {
        if self.lo == self.hi {
            format!("#{}", self.prs[self.lo].number)
        } else {
            format!("#{}–#{}", self.prs[self.lo].number, self.prs[self.hi].number)
        }
    }

    pub fn focus_handle_diff(&self) -> FocusHandle {
        self.focus.clone()
    }

    // ---------- the selected code (assistant context, copy) ----------

    /// The selected rows (text selection, else the gutter selection) as assistant context.
    pub fn selection_context(&mut self, cx: &mut Context<Self>) -> assistant::AskContext {
        let none = assistant::AskContext::none(self.range_label());
        let Some(range) = self.text_selection.as_ref().map(|s| s.rows()).or(self.selection) else { return none };
        let (file, mode, from, to) = (range.file, range.mode, range.from, range.to);
        let first = self.target_at(file, mode, from, true, cx).or_else(|| self.target_at(file, mode, from, false, cx));
        let last = self.target_at(file, mode, to, true, cx).or_else(|| self.target_at(file, mode, to, false, cx));
        let (Some(first), Some(last)) = (first, last) else { return none };
        let mut lines = Vec::new();
        let mut heads = Vec::new();
        for offset in from..=to {
            match mode {
                Mode::Unified => match self.unified_at(file, offset) {
                    UnifiedRow::Row(row) if row.k != row_kind::HUNK && row.k != row_kind::FILE => {
                        let sign = match row.k {
                            row_kind::ADDED => "+",
                            row_kind::DELETED => "-",
                            _ => " ",
                        };
                        lines.push(format!("{sign}{}", seg_text(&row.s)));
                        if let Some(n) = row.n {
                            heads.push(n);
                        }
                    }
                    UnifiedRow::Line { n, segs, .. } => {
                        lines.push(format!(" {}", seg_text(&segs)));
                        heads.push(n);
                    }
                    _ => {}
                },
                _ => {
                    let Some(rows) = self.split_rows(file, cx) else { continue };
                    let Some(row) = rows.get((offset as usize).wrapping_sub(1)) else { continue };
                    lines.push(seg_text(if row.nk == wispy_core::split::FILLER { &row.os } else { &row.ns }));
                    if let Some(n) = row.n {
                        heads.push(n);
                    }
                }
            }
        }
        let same = first.anchor.pr == last.anchor.pr && first.anchor.side == last.anchor.side && first.anchor.path == last.anchor.path;
        let (start, end) = if same {
            (first.anchor.line.min(last.anchor.line), first.anchor.line.max(last.anchor.line))
        } else {
            (last.anchor.line, last.anchor.line)
        };
        let (head_start, head_end) = (heads.iter().min().copied(), heads.iter().max().copied());
        assistant::AskContext {
            range_label: self.range_label(),
            selection: Some(wispy_core::assistant::Selection {
                path: self.view.summary.files[file].path.clone(),
                pr_label: format!("#{}", self.prs[last.anchor.pr as usize].number),
                start_line: start,
                end_line: end,
                text: lines.join("\n"),
                head_start,
                head_end,
            }),
            anchor: Some(wispy_core::assistant::ThreadAnchor {
                path: last.anchor.path.clone(),
                pr_index: last.anchor.pr as usize,
                side: last.anchor.side,
                start_line: start,
                end_line: end,
                head_start,
                head_end,
            }),
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.file_view {
            if let Some(text) = view.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            return;
        }
        if let Some(text) = self.text_selection.as_ref().map(|s| s.text.clone()).filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    // ---------- keys ----------

    fn keys_ok(&self) -> bool {
        self.file_search.is_none()
    }

    fn on_file_step(&mut self, down: bool, unviewed: bool, cx: &mut Context<Self>) {
        if !self.keys_ok() {
            return;
        }
        if let Some(file) = self.file_step(down, unviewed, cx) {
            self.go_to_file(file, false, cx);
        }
    }

    fn on_toggle_mode(&mut self, _: &ToggleMode, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(file) = self.current().filter(|_| self.keys_ok()) {
            self.toggle_file_mode(file, cx);
        }
    }

    fn on_toggle_wrap(&mut self, _: &ToggleWrap, _: &mut Window, cx: &mut Context<Self>) {
        if !self.keys_ok() {
            return;
        }
        let wrap = !self.s(cx).wrap;
        self.record_anchor(cx);
        self.pending_mark = Some("layout");
        crate::perf_marks::mark("toggle:start");
        self.session.update(cx, |s, cx| s.set_wrap(wrap, cx));
        self.list.remeasure();
        self.restore_anchor(true);
    }

    fn on_toggle_threads(&mut self, _: &ToggleThreads, _: &mut Window, cx: &mut Context<Self>) {
        if !self.keys_ok() {
            return;
        }
        let (keys, hidden) = self.s(cx).range_keys();
        self.session.update(cx, |s, cx| s.set_comments_hidden(&keys, !hidden, cx));
    }

    fn on_toggle_list_mode(&mut self, _: &ToggleListMode, _: &mut Window, cx: &mut Context<Self>) {
        if self.keys_ok() {
            self.toggle_list_mode(cx);
        }
    }

    pub fn toggle_list_mode(&mut self, cx: &mut Context<Self>) {
        self.list_mode_tree = !self.list_mode_tree;
        // The current file is revealed in the other layout too.
        self.file_list_current = None;
        settings::set_pref(cx, "fileList", if self.list_mode_tree { "tree" } else { "list" });
        cx.notify();
    }

    fn on_open_filter(&mut self, _: &OpenFilter, window: &mut Window, cx: &mut Context<Self>) {
        if !self.keys_ok() {
            return;
        }
        if !self.s(cx).show_files {
            self.session.update(cx, |s, cx| s.toggle_files(cx));
        }
        self.filter_open = !self.filter_open;
        self.filter_highlight = 0;
        // ↑/↓ and Space work the menu while it's open.
        if self.filter_open {
            let focus = self.filter_focus_handle.get_or_insert_with(|| cx.focus_handle()).clone();
            window.focus(&focus, cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn on_toggle_filter(&mut self, _: &ToggleFilter, _: &mut Window, cx: &mut Context<Self>) {
        let filter = self.filter();
        if self.keys_ok() && !filter.exts.is_empty() {
            self.change_filter(FileFilter { off: !filter.off, ..filter }, cx);
        }
    }

    fn on_hide_extension(&mut self, _: &HideExtension, _: &mut Window, cx: &mut Context<Self>) {
        let Some(here) = self.current().filter(|_| self.keys_ok()) else { return };
        // Hides the current file's most specific extension the menu lists (".unit.php" on a
        // unit test, not every ".php") and moves on to the next file still shown.
        let path = &self.view.summary.files[here].path;
        let listed: HashSet<String> = self.extensions().into_iter().map(|(ext, _, _)| ext).collect();
        let ext = extension_suffixes(path).into_iter().find(|s| listed.contains(s)).unwrap_or_else(|| extension_of(path));
        let mut filter = self.filter();
        if !filter.exts.contains(&ext) {
            filter.exts.push(ext);
        }
        filter.off = false;
        let next = self.view.summary.files.iter().enumerate().position(|(i, f)| i > here && !filter_hides(&filter, &f.path));
        if let Some(next) = next {
            self.pending_file = Some((here, next));
        }
        self.change_filter(filter, cx);
    }

    fn on_toggle_collapse(&mut self, _: &ToggleCollapse, _: &mut Window, cx: &mut Context<Self>) {
        let Some(here) = self.current().filter(|_| self.keys_ok()) else { return };
        let path = self.view.summary.files[here].path.clone();
        // A file opened for a look stays open instead.
        if self.temporary.as_deref() == Some(path.as_str()) {
            self.keep_open(here);
            cx.notify();
            return;
        }
        let mode = self.built.segments[here].mode;
        // Collapsing moves on to the next file that's still open, like `v`.
        let next = if mode == Mode::Collapsed {
            None
        } else {
            self.built.segments.iter().find(|s| s.file > here && s.mode != Mode::Collapsed && s.mode != Mode::Hidden).map(|s| s.file)
        };
        match next {
            Some(next) => self.pending_file = Some((here, next)),
            None => self.keep_file = Some(here),
        }
        self.change_layout(cx, |this, _| {
            if mode == Mode::Collapsed {
                this.expanded.insert(path.clone());
                this.collapsed.remove(&path);
            } else {
                this.collapsed.insert(path.clone());
                this.expanded.remove(&path);
            }
        });
    }

    fn on_toggle_gaps(&mut self, _: &ToggleGaps, _: &mut Window, cx: &mut Context<Self>) {
        let Some(here) = self.current().filter(|_| self.keys_ok()) else { return };
        let file = self.view.summary.files[here].clone();
        if self.built.segments[here].mode != Mode::Unified || file.gaps.is_empty() {
            return;
        }
        let next = if all_shown(&file, self.reveals.get(&here).map(Vec::as_slice)) { None } else { Some(reveal_all(&file)) };
        self.change_gaps(here, next, None, cx);
    }

    fn on_comment(&mut self, _: &Comment, window: &mut Window, cx: &mut Context<Self>) {
        if !self.keys_ok() || self.built.total_rows == 0 {
            return;
        }
        if let Some(target) = self.hovered.clone() {
            self.open_composer(target.clone(), target, window, cx);
        } else if let Some(here) = self.current() {
            self.open_file_composer(here, window, cx);
        }
    }

    fn on_usages(&mut self, _: &Usages_, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(word) = self.hover_word.clone().filter(|_| self.keys_ok()) {
            self.open_usages(&word, window, cx);
        }
    }

    fn on_search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        if self.keys_ok() {
            self.open_search(String::new(), window, cx);
        }
    }

    fn on_assistant(&mut self, _: &Assistant, window: &mut Window, cx: &mut Context<Self>) {
        if self.keys_ok() {
            self.open_assistant(None, window, cx);
        }
    }

    fn on_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_view.is_some() {
            self.file_view = None;
        } else if self.filter_open {
            self.filter_open = false;
        } else if self.assistant.is_some() {
            self.assistant = None;
            assistant::forget_open(&self.session.clone(), &self.prs, cx);
        } else if self.panel.is_some() {
            self.panel = None;
        } else if self.composer.is_some() {
            self.composer = None;
            self.relayout(cx);
        } else if self.selection.is_some() || self.text_selection.is_some() {
            self.selection = None;
            self.text_selection = None;
        } else {
            cx.propagate();
            return;
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn on_toggle_viewed(&mut self, _: &ToggleViewed, _: &mut Window, cx: &mut Context<Self>) {
        let Some(here) = self.current().filter(|_| self.keys_ok()) else { return };
        let file = self.view.summary.files[here].clone();
        let viewed = !self.is_viewed(&file.content_key, cx);
        if !viewed {
            self.apply_viewed_now(here, false, cx);
            return;
        }
        let this = cx.entity().downgrade();
        let later: crate::session::Later = std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |this, cx| this.apply_viewed_now(here, true, cx));
        });
        let confirmed = self.session.update(cx, |s, cx| s.confirm_viewed(&file.path, &file.content_key, later, cx));
        if confirmed {
            self.apply_viewed_now(here, true, cx);
        }
    }

    /// `v`: marks the file viewed (or not) and, marking, moves on to the next file still to review.
    fn apply_viewed_now(&mut self, here: usize, viewed: bool, cx: &mut Context<Self>) {
        let file = self.view.summary.files[here].clone();
        let filtered = self.filtered();
        let next = if viewed {
            self.view.summary.files.iter().enumerate().position(|(i, f)| i > here && !self.is_viewed(&f.content_key, cx) && !filtered[i])
        } else {
            None
        };
        self.change_layout(cx, |this, cx| {
            this.session.update(cx, |s, cx| s.set_viewed(&file.content_key, viewed, cx));
            this.expanded.remove(&file.path);
            if !viewed {
                this.collapsed.remove(&file.path);
            }
            if this.temporary.as_deref() == Some(file.path.as_str()) {
                this.temporary = None;
            }
            this.viewed_history.retain(|p| *p != file.path);
            if viewed {
                this.viewed_history.push(file.path.clone());
            }
            match next {
                Some(next) => this.pending_file = Some((here, next)),
                None => this.keep_file = Some(here),
            }
        });
    }

    fn on_undo_viewed(&mut self, _: &UndoViewed, _: &mut Window, cx: &mut Context<Self>) {
        if !self.keys_ok() {
            return;
        }
        // Undoes the latest `v` still in effect: un-views that file and goes back to it.
        let mut index = None;
        while let Some(path) = self.viewed_history.pop() {
            if let Some(i) = self.view.summary.files.iter().position(|f| f.path == path) {
                if self.is_viewed(&self.view.summary.files[i].content_key, cx) && self.built.segments[i].mode != Mode::Hidden {
                    index = Some(i);
                    break;
                }
            }
        }
        let Some(index) = index else { return };
        let file = self.view.summary.files[index].clone();
        self.change_layout(cx, |this, cx| {
            this.session.update(cx, |s, cx| s.set_viewed(&file.content_key, false, cx));
            this.expanded.remove(&file.path);
            this.collapsed.remove(&file.path);
            this.reveal_file = Some(index);
        });
    }

    fn on_modifiers(&mut self, event: &ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = event.modifiers;
        let only_shift = m.shift && !m.control && !m.alt && !m.platform && !m.function;
        if only_shift && !self.shift_down {
            let now = Instant::now();
            if self.keys_ok() && self.last_shift.is_some_and(|t| now - t < DOUBLE_SHIFT) && !window_has_input_focus(window, cx) {
                self.last_shift = None;
                self.open_file_search(window, cx);
            } else {
                self.last_shift = Some(now);
            }
        } else if !m.shift && (m.control || m.alt || m.platform) {
            self.last_shift = None;
        }
        self.shift_down = m.shift;
    }

    /// Any other key in between doesn't make ⇧⇧.
    fn reset_shift(&mut self) {
        self.last_shift = None;
    }
}

/// Typing in a text box (a comment, the panel's search) isn't ⇧⇧.
fn window_has_input_focus(window: &mut Window, cx: &mut App) -> bool {
    use gpui_kit::component::WindowExt;
    window.focused_input(cx).is_some()
}

/// A unified list row, resolved.
pub enum UnifiedRow<'a> {
    Row(&'a Row),
    /// A shown unchanged line of a gap.
    Line { o: u32, n: u32, segs: Arc<Vec<Seg>> },
    /// The row standing for what's still hidden of a gap (its hunk header, or none after the last hunk).
    Gap { gap: usize, header: Option<&'a Row> },
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GutterEvent {
    Down,
    Enter,
    Up,
    Hover,
}

/// What a click on a gap row shows: some above and below (the row), more of one side (its arrows), or all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GapExpand {
    Row,
    Up,
    Down,
    All,
}

pub fn seg_text(segs: &[Seg]) -> String {
    segs.iter().map(|(_, t)| t.as_str()).collect()
}

/// `css_px` at the current zoom.
pub fn u_px(css_px: f32, cx: &App) -> Pixels {
    crate::ui::upx(css_px, cx)
}

/// Where a comment on this unified row goes: the PR that last touched it, at its line there.
pub fn unified_anchor(row: &Row, file: &FileSummary, top: usize) -> Option<Anchor> {
    match (row.k, row.a, row.l) {
        (row_kind::ADDED, Some(a), Some(l)) => Some(Anchor { pr: a, side: Side::Right, line: l, path: path_in_pr(file, a, Side::Right).to_string() }),
        (row_kind::DELETED, Some(a), Some(l)) => Some(Anchor { pr: a, side: Side::Left, line: l, path: path_in_pr(file, a, Side::Left).to_string() }),
        (row_kind::CONTEXT, _, _) => row.n.map(|n| Anchor { pr: top as u8, side: Side::Right, line: n, path: file.path.clone() }),
        _ => None,
    }
}

pub fn split_anchor(row: &SplitRow, file: &FileSummary, top: usize, new_side: bool) -> Option<Anchor> {
    if !new_side {
        return match (row.ok, row.oa, row.ol) {
            (row_kind::DELETED, Some(a), Some(l)) => Some(Anchor { pr: a, side: Side::Left, line: l, path: path_in_pr(file, a, Side::Left).to_string() }),
            (row_kind::CONTEXT, _, _) => row.n.map(|n| Anchor { pr: top as u8, side: Side::Right, line: n, path: file.path.clone() }),
            _ => None,
        };
    }
    match (row.nk, row.na, row.nl) {
        (row_kind::ADDED, Some(a), Some(l)) => Some(Anchor { pr: a, side: Side::Right, line: l, path: path_in_pr(file, a, Side::Right).to_string() }),
        (row_kind::CONTEXT, _, _) => row.n.map(|n| Anchor { pr: top as u8, side: Side::Right, line: n, path: file.path.clone() }),
        _ => None,
    }
}

impl Focusable for DiffViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DiffViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((file, line, end)) = self.pending_jump_now.take() {
            self.jump_to_line(file, line, end, cx);
        }
        // Read before the list lays out: rows drawn inside it can't ask the list.
        self.frame_current = self.current();
        if let Some(name) = self.pending_mark.take() {
            crate::perf_marks::mark(&format!("{name}:rendered"));
            window.on_next_frame(move |_, _| crate::perf_marks::mark(&format!("{name}:first-visible")));
        }
        self.update_wrap_columns(window, cx);
        let p = palette(cx).clone();
        let s = self.s(cx);
        let show_files = s.show_files;
        let filtered = self.filtered();
        let filtered_count = filtered.iter().filter(|f| **f).count();
        let files_count = self.view.summary.files.len();
        let empty_since = files_count == 0 && self.since;
        crate::ui::load_images(&self.backend, cx);
        let file_view = self.file_view.is_some().then(|| self.render_file_view(window, cx));
        let rows = list(self.list.clone(), cx.processor(|this, index: usize, window, cx| this.render_item(index, window, cx))).size_full();
        div()
            .key_context("Diff")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ScrollDown, window, cx| {
                let step = u_px(ARROW_STEP, cx);
                this.glide_pixels(step, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ScrollUp, window, cx| {
                let step = u_px(ARROW_STEP, cx);
                this.glide_pixels(-step, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextFileAny, _, cx| this.on_file_step(true, false, cx)))
            .on_action(cx.listener(|this, _: &PrevFileAny, _, cx| this.on_file_step(false, false, cx)))
            .on_action(cx.listener(|this, _: &NextFile, _, cx| this.on_file_step(true, true, cx)))
            .on_action(cx.listener(|this, _: &PrevFile, _, cx| this.on_file_step(false, true, cx)))
            .on_action(cx.listener(Self::on_toggle_mode))
            .on_action(cx.listener(|this, _: &ToggleAllModes, _, cx| {
                if this.keys_ok() {
                    this.toggle_all_modes(cx)
                }
            }))
            .on_action(cx.listener(Self::on_toggle_wrap))
            .on_action(cx.listener(Self::on_toggle_threads))
            .on_action(cx.listener(Self::on_toggle_list_mode))
            .on_action(cx.listener(Self::on_open_filter))
            .on_action(cx.listener(Self::on_toggle_filter))
            .on_action(cx.listener(Self::on_hide_extension))
            .on_action(cx.listener(Self::on_toggle_collapse))
            .on_action(cx.listener(Self::on_toggle_gaps))
            .on_action(cx.listener(Self::on_comment))
            .on_action(cx.listener(Self::on_usages))
            .on_action(cx.listener(Self::on_search))
            .on_action(cx.listener(Self::on_assistant))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_toggle_viewed))
            .on_action(cx.listener(Self::on_undo_viewed))
            .on_action(cx.listener(|this, _: &Back, window, cx| this.navigate(true, window, cx)))
            .on_action(cx.listener(|this, _: &Forward, window, cx| this.navigate(false, window, cx)))
            .on_action(cx.listener(Self::copy))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .capture_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, _| {
                if event.keystroke.key != "shift" {
                    this.reset_shift();
                }
            }))
            .on_any_mouse_down(cx.listener(|this, _, _, _| this.reset_shift()))
            // The mouse's back / forward buttons, like in a browser.
            .on_drag_move(cx.listener(Self::on_sidebar_drag))
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _, window, cx| this.navigate(true, window, cx)))
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|this, _, window, cx| this.navigate(false, window, cx)))
            .size_full()
            .flex()
            .bg(p.bg)
            .text_color(p.fg)
            .when(show_files, |d| d.child(self.render_file_list(window, cx)))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .when(filtered_count > 0 && filtered_count == files_count, |d| {
                        let message = if filtered_count == 1 { "The only file is".to_string() } else { format!("All {filtered_count} files are") };
                        d.child(
                            div()
                                .p(u(16.))
                                .flex()
                                .gap(u(6.))
                                .text_size(u(13.))
                                .text_color(p.fg_muted)
                                .child(format!("{message} hidden by the file filter."))
                                .child(
                                    crate::ui::link_button("Show all files", cx)
                                        .id("show-all-files")
                                        .on_click(cx.listener(|this, _, _, cx| this.change_filter(FileFilter::default(), cx))),
                                ),
                        )
                    })
                    .when(files_count == 0, |d| {
                        d.child(
                            div().p(u(16.)).flex().gap(u(6.)).text_size(u(13.)).text_color(p.fg_muted).map(|d| {
                                if empty_since {
                                    d.child("Nothing changed since this checkpoint.")
                                        .child(
                                            crate::ui::link_button("Show the full diff", cx)
                                                .id("show-full-diff")
                                                .on_click(cx.listener(|this, _, _, cx| this.session.update(cx, |s, cx| s.toggle_since(cx)))),
                                        )
                                        .child("(d)")
                                } else {
                                    d.child("No changes.")
                                }
                            }),
                        )
                    })
                    .child(
                        div()
                            .id("diff-scroll")
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .font_family(crate::ui::mono_family(cx))
                            .text_size(u(12.))
                            .line_height(u(20.))
                            .on_scroll_wheel(cx.listener(Self::on_wheel))
                            .on_mouse_move(cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, _| this.pointer = Some(event.position)))
                            .child(rows)
                            .children(self.sticky_header(cx))
                            .child(gpui_kit::base::Scrollbar::vertical(&self.list))
                            .child(self.drag_listener(cx)),
                    ),
            )
            .children(self.code_panel().cloned().map(|panel| {
                div().relative().h_full().child(panel).child(file_list::resize_handle("code", true, cx)).into_any_element()
            }))
            .children(self.assistant.clone().map(|panel| {
                div().relative().h_full().child(panel).child(file_list::resize_handle("assistant", true, cx)).into_any_element()
            }))
            .children(file_view)
            .children(self.file_search.clone().map(|search| {
                let search_view = search.clone();
                crate::ui::backdrop(search, move |_, cx| search_view.update(cx, |_, cx| cx.emit(FileSearchEvent::Close)), cx)
            }))
    }
}

impl DiffViewer {
    fn on_wheel(&mut self, event: &gpui_kit::ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Wheel scrolling stops a glide; sideways scrolling moves the code (both halves together).
        self.glide = None;
        let delta = event.delta.pixel_delta(u_px(20., cx));
        if delta.y != px(0.) {
            self.pinned = None;
        }
        let dx = if event.modifiers.shift && delta.x == px(0.) { delta.y } else { delta.x };
        if dx == px(0.) || self.s(cx).wrap {
            return;
        }
        let char_width = self.char_width(window, cx);
        let content = char_width * (self.view.summary.max_line_chars + 4) as f32;
        let visible = self.list.viewport_bounds().size.width * 0.5;
        let max = (content - visible).max(px(0.));
        self.scroll_x = (self.scroll_x - dx).clamp(px(0.), max);
        cx.notify();
    }

    /// Window-wide mouse moves and releases while a selection is dragged (they may leave the list).
    fn drag_listener(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        gpui_kit::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let (moved, up) = (this.clone(), this.clone());
                window.on_mouse_event(move |event: &gpui_kit::MouseMoveEvent, phase, _, cx| {
                    if phase == gpui_kit::DispatchPhase::Bubble {
                        let _ = moved.update(cx, |this, cx| this.on_window_mouse_move(event, cx));
                    }
                });
                window.on_mouse_event(move |event: &gpui_kit::MouseUpEvent, phase, _, cx| {
                    if phase == gpui_kit::DispatchPhase::Bubble && event.button == gpui_kit::MouseButton::Left {
                        let _ = up.update(cx, |this, cx| {
                            this.drag_scroll = None;
                            if this.text_selection.as_ref().is_some_and(|s| s.active) {
                                this.finish_selection(cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// A mouse move anywhere in the window (a drag leaves the list): a selection being dragged
    /// near or past the list's top or bottom scrolls it.
    fn on_window_mouse_move(&mut self, event: &gpui_kit::MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(gpui_kit::MouseButton::Left) || !self.text_selection.as_ref().is_some_and(|s| s.active) {
            return;
        }
        self.pointer = Some(event.position);
        if self.drag_scroll.is_none() && self.drag_scroll_speed(cx) != px(0.) {
            self.drag_scroll = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let going = this.update(cx, |this, cx| this.drag_scroll_step(cx)).unwrap_or(false);
                if !going {
                    let _ = this.update(cx, |this, _| this.drag_scroll = None);
                    break;
                }
            }));
        }
    }

    /// Pixels per tick: faster the further the pointer is past the edge.
    fn drag_scroll_speed(&self, cx: &App) -> Pixels {
        let Some(pointer) = self.pointer else { return px(0.) };
        let bounds = self.list.viewport_bounds();
        let edge = u_px(20., cx);
        let (top, bottom) = (bounds.top() + edge, bounds.bottom() - edge);
        let over = if pointer.y < top {
            pointer.y - top
        } else if pointer.y > bottom {
            pointer.y - bottom
        } else {
            return px(0.);
        };
        (over * 0.5).clamp(-u_px(60., cx), u_px(60., cx))
    }

    fn drag_scroll_step(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.text_selection.as_ref().is_some_and(|s| s.active) {
            return false;
        }
        let speed = self.drag_scroll_speed(cx);
        if speed == px(0.) {
            return false;
        }
        self.glide = None;
        self.pinned = None;
        self.list.scroll_by(speed);
        // The selection follows to the row at the edge it scrolls towards.
        let top = self.list.logical_scroll_top().item_ix;
        let item = if speed > px(0.) {
            let bottom = self.list.viewport_bounds().bottom();
            (top..self.list.item_count()).take_while(|ix| self.list.bounds_for_item(*ix).is_some_and(|b| b.top() < bottom)).last().unwrap_or(top)
        } else {
            top
        };
        let row = self.built.row_of_item(item);
        if let (Some(segment), Some(sel)) = (self.built.segment_at(row).cloned(), self.text_selection.as_mut()) {
            if segment.file == sel.file && segment.mode == sel.mode && row > segment.start {
                let byte = if speed > px(0.) { usize::MAX } else { 0 };
                sel.head = selection::Point { offset: row - segment.start, byte };
            }
        }
        cx.notify();
        true
    }

    pub fn char_width(&mut self, window: &mut Window, cx: &App) -> Pixels {
        if let Some(w) = self.char_width {
            return w;
        }
        let font = gpui_kit::font(crate::ui::mono_family(cx));
        let size = u_px(12., cx);
        let runs = [gpui_kit::TextRun { len: 10, font, color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None }];
        let w = window.text_system().shape_line("0000000000".into(), size, &runs, None).width() / 10.;
        self.char_width = Some(w);
        w
    }

    /// The wrap widths from the list's last layout (rows are re-measured when they change).
    fn update_wrap_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let zoom = crate::ui::zoom(cx);
        if self.char_width.is_some_and(|_| self.char_zoom != zoom) {
            self.char_width = None;
        }
        self.char_zoom = zoom;
        let columns = if self.s(cx).wrap {
            let cw = self.char_width(window, cx);
            let width = self.list.viewport_bounds().size.width;
            let gutter = rows::gutter_width(self, cx);
            let unified = width - (gutter * 2. + u_px(3. + 13. + 16., cx) + self.tag_room(cx));
            let split = width * 0.5 - (gutter + u_px(1. + 3. + 16., cx));
            let fit = |w: Pixels| ((w / cw).floor().max(20.)) as usize;
            if width > px(0.) { (fit(unified), fit(split)) } else { (0, 0) }
        } else {
            (0, 0)
        };
        if columns != self.wrap_columns {
            self.wrap_columns = columns;
            self.list.remeasure();
        }
    }

    /// Room on the right of unified rows for the PR tags of a multi-PR range.
    pub fn tag_room(&self, cx: &App) -> Pixels {
        if self.hi == self.lo {
            return px(0.);
        }
        let longest = self.prs.iter().map(|p| p.number.to_string().len()).max().unwrap_or(1) as f32;
        u_px((longest + 1.) * 6.5 + 12. + 8., cx)
    }
}

#[cfg(test)]
mod extension_tests {
    use super::*;

    #[test]
    fn multi_part_extensions_and_hiding() {
        assert_eq!(extension_suffixes("src/A.unit.php"), [".unit.php", ".php"]);
        assert_eq!(extension_suffixes("a/b.spec.unit.php"), [".spec.unit.php", ".unit.php", ".php"]);
        assert_eq!(extension_suffixes("x.a.b.c.d.php"), [".c.d.php", ".d.php", ".php"]);
        assert_eq!(extension_suffixes("lib/v1.2.json"), [".json"]);
        assert_eq!(extension_suffixes("jquery.min.js"), [".min.js", ".js"]);
        assert!(extension_suffixes(".gitignore").is_empty());
        assert!(extension_suffixes("Makefile").is_empty());
        let filter = FileFilter { exts: vec![".unit.php".into()], off: false };
        assert!(filter_hides(&filter, "tests/A.unit.php"));
        assert!(!filter_hides(&filter, "src/A.php"));
        assert!(!filter_hides(&filter, "tests/A.integration.php"));
        let none = FileFilter { exts: vec![String::new()], off: false };
        assert!(filter_hides(&none, "Makefile"));
        assert!(!filter_hides(&none, "a.php"));
    }
}
