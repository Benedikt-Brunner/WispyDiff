//! Side-panel conversations with Claude Code or Codex about the range or the selected lines (was
//! `AssistantPanel.tsx`). The panel is kept per repo across diff viewers (a newer version of the
//! stack, a trip to the inbox): it stays open on the stack it was opened on, and an answer still
//! streaming lands in its thread.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use futures::channel::mpsc;
use futures::StreamExt;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    anchored, deferred, div, px, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    KeyBinding, Render, ScrollHandle, SharedString, Subscription, Task, Window,
};
use wispy_core::assistant::{AssistantEvent, CommentAction, CommentOutcome, Provider, Selection, Thread, ThreadAnchor};
use wispy_core::github::PullRequest;
use wispy_core::service::NewThread;

use super::DiffViewer;
use crate::backend::Backend;
use crate::logic::file_refs::CodeRef;
use crate::markdown::{CodeRefs, RenderOptions};
use crate::session::Session;
use crate::settings;
use crate::ui::{button, link_button, palette, u};

gpui_kit::actions!(assistant, [CloseAssistant]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseAssistant, Some("Assistant"))]);
}

const CLAUDE_MODELS: [&str; 5] = ["", "opus", "sonnet", "haiku", "fable"];
const CLAUDE_EFFORTS: [&str; 6] = ["", "low", "medium", "high", "xhigh", "max"];
const CODEX_EFFORTS: [&str; 5] = ["", "minimal", "low", "medium", "high"];

fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    }
}

fn provider_id(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// What a new conversation is about.
#[derive(Clone, Debug)]
pub struct AskContext {
    pub selection: Option<Selection>,
    pub anchor: Option<ThreadAnchor>,
    /// Shown when there's no selection, e.g. "#2–#3".
    pub range_label: String,
}

impl AskContext {
    pub fn none(range_label: String) -> Self {
        AskContext { selection: None, anchor: None, range_label }
    }
}

/// The question being answered, the answer so far and what the assistant did to the drafts.
struct Pending {
    thread_id: Option<String>,
    question: String,
    text: String,
    comments: Vec<CommentOutcome>,
}

enum SignIn {
    Waiting(Option<String>),
    Done,
    Failed(String),
}

pub enum AssistantPanelEvent {
    Close,
    OpenRef(CodeRef),
    DraftsChanged,
    Resized,
}

/// The panels, per repo, kept by the window's `Session`. A panel lives on when it's closed (its
/// threads give the gutter markers, and an answer still streaming or an unsent question survives)
/// and across diff viewers; `open` says which are shown.
#[derive(Default)]
pub struct Panels {
    panels: HashMap<String, Entity<AssistantPanel>>,
    open: HashSet<String>,
}

pub struct AssistantPanel {
    backend: Arc<Backend>,
    /// The stack (PR numbers) it was opened on.
    prs: Vec<u64>,
    stack_id: String,
    lo: usize,
    hi: usize,
    /// The diff's file paths and hidden ones, for file references and the filter.
    files: Vec<String>,
    hidden: Vec<String>,
    exists: HashMap<String, bool>,
    asked: HashSet<String>,
    context: AskContext,
    pub active: Option<String>,
    note: Option<String>,
    pub threads: Vec<Thread>,
    pending: Option<Pending>,
    provider: Provider,
    model: String,
    effort: String,
    sign_in: Option<SignIn>,
    question: Entity<TextareaState>,
    codex_model: Entity<InputState>,
    /// Model and effort chosen for the active conversation's next turns (its id first).
    turn: Option<(String, String, String)>,
    /// The agent / model / effort popup.
    picker_open: bool,
    menus: crate::menu::MenuSlot,
    /// The question's placeholder needs updating (an answer started a conversation).
    placeholder_stale: bool,
    pub width: f32,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _ask: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AssistantPanelEvent> for AssistantPanel {}

/// The repo's panel (created on first use, its threads loaded), kept up to date with `viewer`'s
/// view, and wired to `viewer`; shown when it's open on this stack.
pub fn attach(viewer: &mut DiffViewer, window: &mut Window, cx: &mut Context<DiffViewer>) -> (Entity<AssistantPanel>, Subscription, bool) {
    let repo = viewer.prs[0].base_repo.clone();
    let session = viewer.session.clone();
    let existing = session.read(cx).assistant.panels.get(&repo).cloned();
    let panel = match existing {
        Some(panel) => panel,
        None => {
            let (backend, width) = (viewer.backend.clone(), viewer.assistant_width);
            let prs: Vec<u64> = viewer.prs.iter().map(|p| p.number).collect();
            let panel = cx.new(|cx| AssistantPanel::new(backend, prs, width, window, cx));
            session.update(cx, |s, _| s.assistant.panels.insert(repo.clone(), panel.clone()));
            panel
        }
    };
    sync_view(viewer, &panel, cx);
    let subscription = cx.subscribe_in(&panel, window, |this, _, event: &AssistantPanelEvent, window, cx| match event {
        AssistantPanelEvent::Close => {
            this.assistant = None;
            forget_open(&this.session.clone(), &this.prs, cx);
            window.focus(&this.focus_handle_diff(), cx);
            cx.notify();
        }
        AssistantPanelEvent::OpenRef(code_ref) => {
            let code_ref = code_ref.clone();
            this.open_ref(code_ref, window, cx);
        }
        AssistantPanelEvent::DraftsChanged => this.session.update(cx, |s, cx| s.refresh_drafts(cx)),
        // New threads: the gutter markers change.
        AssistantPanelEvent::Resized => cx.notify(),
    });
    let shown = session.read(cx).assistant.open.contains(&repo)
        && viewer.prs.iter().any(|p| panel.read(cx).prs.contains(&p.number));
    (panel, subscription, shown)
}

/// The panel's view inputs follow the viewer (range, files, the filter).
pub fn sync_view(viewer: &DiffViewer, panel: &Entity<AssistantPanel>, cx: &mut App) {
    let (stack, lo, hi, files, hidden) = view_inputs(viewer);
    panel.update(cx, |p, cx| p.set_view(stack, lo, hi, files, hidden, cx));
}

/// Closes the panel for this stack's repo (it lives on, hidden).
pub fn forget_open(session: &Entity<Session>, prs: &[PullRequest], cx: &mut App) {
    if let Some(repo) = prs.first().map(|p| p.base_repo.clone()) {
        session.update(cx, |s, _| s.assistant.open.remove(&repo));
    }
}

fn view_inputs(viewer: &DiffViewer) -> (String, usize, usize, Vec<String>, Vec<String>) {
    let filtered = viewer.filtered();
    let files = viewer.view.summary.files.iter().map(|f| f.path.clone()).collect();
    let hidden = viewer.view.summary.files.iter().enumerate().filter(|(i, _)| filtered[*i]).map(|(_, f)| f.path.clone()).collect();
    (viewer.stack_id.clone(), viewer.lo, viewer.hi, files, hidden)
}

/// Opens (or re-targets) the panel on `context` / `thread`.
pub fn open(viewer: &mut DiffViewer, panel: &Entity<AssistantPanel>, context: AskContext, thread: Option<String>, window: &mut Window, cx: &mut Context<DiffViewer>) {
    let repo = viewer.prs[0].base_repo.clone();
    viewer.session.update(cx, |s, _| s.assistant.open.insert(repo));
    let prs: Vec<u64> = viewer.prs.iter().map(|p| p.number).collect();
    sync_view(viewer, panel, cx);
    panel.update(cx, |p, cx| {
        p.prs = prs;
        p.context = context;
        p.active = thread;
        p.note = None;
        p.sign_in = None;
        p.menus.close(window, cx);
        p.load_threads(cx);
        p.update_placeholder(window, cx);
        p.scroll.scroll_to_bottom();
        p.question.update(cx, |q, cx| q.focus(window, cx));
        cx.notify();
    });
}

/// The assistant thread asked about head line `line` of `path`, if any (a gutter marker).
pub fn marker_at(viewer: &DiffViewer, path: &str, line: u32, cx: &App) -> Option<String> {
    let repo = &viewer.prs.first()?.base_repo;
    let panel = viewer.session.read(cx).assistant.panels.get(repo)?;
    let files = &viewer.view.summary.files;
    panel.read(cx).threads.iter().find_map(|t| {
        let a = t.selection.as_ref()?;
        let (from, to) = (a.head_start?, a.head_end?);
        let file = files.iter().find(|f| f.path == a.path || f.pr_paths.iter().any(|(_, p)| *p == a.path))?;
        (file.path == path && line >= from && line <= to).then(|| t.id.clone())
    })
}

impl DiffViewer {
    /// A file reference in an answer: the file's header for a whole file in the diff, else a jump.
    pub fn open_ref(&mut self, r: CodeRef, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.view.summary.files.iter().position(|f| f.path == r.path);
        if let (None, Some(file)) = (r.line, file) {
            if self.built.segments[file].mode != super::layout::Mode::Hidden {
                self.remember(cx);
                let start = self.built.segments[file].start;
                self.scroll_to_row(start, 0, Some(file), cx);
                return;
            }
        }
        self.jump(&r.path, r.line.unwrap_or(0), r.end, None, window, cx);
    }
}

impl AssistantPanel {
    fn new(backend: Arc<Backend>, prs: Vec<u64>, width: f32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let provider = if settings::pref(cx, "assistant.provider", &["claude", "codex"], "claude") == "codex" { Provider::Codex } else { Provider::Claude };
        let model = settings::get::<String>(cx, &format!("assistant.model.{}", provider_id(provider))).unwrap_or_default();
        let effort = settings::get::<String>(cx, &format!("assistant.effort.{}", provider_id(provider))).unwrap_or_default();
        let question = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true).auto_grow(3, 12).placeholder("Ask… (↵ to send, ⇧↵ for a new line)"));
        let codex_model = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("default model");
            s.set_value(if provider == Provider::Codex { model.clone() } else { String::new() }, window, cx);
            s
        });
        let subscriptions = vec![
            cx.subscribe_in(&question, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send(window, cx);
                }
            }),
            cx.subscribe_in(&codex_model, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value().trim().to_string();
                    if value != this.choice().1 {
                        this.set_model(value, cx);
                    }
                }
                InputEvent::PressEnter { .. } => this.send(window, cx),
                _ => {}
            }),
        ];
        AssistantPanel {
            backend,
            prs,
            stack_id: String::new(),
            lo: 0,
            hi: 0,
            files: Vec::new(),
            hidden: Vec::new(),
            exists: HashMap::new(),
            asked: HashSet::new(),
            context: AskContext::none(String::new()),
            active: None,
            note: None,
            threads: Vec::new(),
            pending: None,
            provider,
            model,
            effort,
            sign_in: None,
            question,
            codex_model,
            turn: None,
            picker_open: false,
            menus: Default::default(),
            placeholder_stale: false,
            width,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _ask: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.width = width;
        cx.notify();
    }

    fn set_view(&mut self, stack: String, lo: usize, hi: usize, files: Vec<String>, hidden: Vec<String>, cx: &mut Context<Self>) {
        if self.files != files {
            self.exists.clear();
            self.asked.clear();
        }
        let new_stack = self.stack_id != stack;
        self.stack_id = stack;
        self.lo = lo;
        self.hi = hi;
        self.files = files;
        self.hidden = hidden;
        if new_stack {
            self.load_threads(cx);
        }
        cx.notify();
    }

    /// "Ask…" for a new conversation, "Follow up…" in one.
    fn update_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = if self.active.is_some() { "Follow up… (↵ to send, ⇧↵ for a new line)" } else { "Ask… (↵ to send, ⇧↵ for a new line)" };
        self.question.update(cx, |q, cx| q.set_placeholder(text, window, cx));
    }

    fn load_threads(&mut self, cx: &mut Context<Self>) {
        let task = self.backend.list_assistant_threads(self.stack_id.clone());
        cx.spawn(async move |this, cx| {
            if let Ok(threads) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.threads = threads;
                    cx.emit(AssistantPanelEvent::Resized);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remember(&mut self, key: &str, value: String, cx: &mut Context<Self>) {
        settings::set(cx, &format!("assistant.{key}.{}", provider_id(self.provider)), value.clone());
        if key == "model" {
            self.model = value;
        } else {
            self.effort = value;
        }
        cx.notify();
    }

    fn change_provider(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        self.provider = provider;
        settings::set_pref(cx, "assistant.provider", provider_id(provider));
        self.model = settings::get::<String>(cx, &format!("assistant.model.{}", provider_id(provider))).unwrap_or_default();
        self.effort = settings::get::<String>(cx, &format!("assistant.effort.{}", provider_id(provider))).unwrap_or_default();
        let model = self.model.clone();
        self.codex_model.update(cx, |s, cx| s.set_value(model, window, cx));
        cx.notify();
    }

    /// The agent, model and effort the next turn uses: an open conversation's (its agent is fixed;
    /// model and effort as last changed for it), or for a new one the remembered choice.
    fn choice(&self) -> (Provider, String, String) {
        match self.active.as_ref().and_then(|id| self.threads.iter().find(|t| t.id == *id)) {
            Some(t) => {
                let (model, effort) = match &self.turn {
                    Some((id, m, e)) if *id == t.id => (m.clone(), e.clone()),
                    _ => (t.model.clone().unwrap_or_default(), t.effort.clone().unwrap_or_default()),
                };
                (t.provider, model, effort)
            }
            None => (self.provider, self.model.clone(), self.effort.clone()),
        }
    }

    fn set_model(&mut self, model: String, cx: &mut Context<Self>) {
        match self.active.clone() {
            Some(id) => self.turn = Some((id, model, self.choice().2)),
            None => self.remember("model", model, cx),
        }
        cx.notify();
    }

    fn set_effort(&mut self, effort: String, cx: &mut Context<Self>) {
        match self.active.clone() {
            Some(id) => self.turn = Some((id, self.choice().1, effort)),
            None => self.remember("effort", effort, cx),
        }
        cx.notify();
    }

    fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        if self.picker_open {
            let model = self.choice().1;
            self.codex_model.update(cx, |s, cx| s.set_value(model, window, cx));
        }
        self.focus_question(window, cx);
        cx.notify();
    }

    /// The agent / model / effort popup is open (tests).
    pub fn picker_is_open(&self) -> bool {
        self.picker_open
    }

    /// Typing goes on in the question after the chip or the popup was clicked (a click in the
    /// panel otherwise leaves the focus on the panel).
    fn focus_question(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.question.update(cx, |q, cx| q.focus(window, cx));
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let question = self.question.read(cx).value().trim().to_string();
        if question.is_empty() || self.pending.is_some() {
            return;
        }
        let (provider, model, effort) = self.choice();
        let choice = (provider, (!model.is_empty()).then_some(model), (!effort.is_empty()).then_some(effort));
        self.picker_open = false;
        self.ask(question, choice, cx);
        self.question.update(cx, |q, cx| q.set_value(String::new(), window, cx));
    }

    fn ask(&mut self, question: String, choice: (Provider, Option<String>, Option<String>), cx: &mut Context<Self>) {
        let thread_id = self.active.clone();
        self.pending = Some(Pending { thread_id: thread_id.clone(), question: question.clone(), text: String::new(), comments: Vec::new() });
        // Files hidden by the filter are named to the assistant but their diff is left out.
        // A follow-up goes on with the model and effort chosen now (the agent stays).
        let retune = thread_id.as_ref().map(|_| (choice.1.clone(), choice.2.clone()));
        let new_thread = thread_id.is_none().then(|| NewThread {
            provider: choice.0,
            model: choice.1,
            effort: choice.2,
            selection: self.context.selection.clone(),
            anchor: self.context.anchor.clone(),
            hidden: self.hidden.clone(),
        });
        let (tx, mut rx) = mpsc::unbounded::<AssistantEvent>();
        let task = self.backend.ask_assistant(self.stack_id.clone(), self.lo, self.hi, thread_id, new_thread, retune, question, move |event| {
            let _ = tx.unbounded_send(event);
        });
        self._ask = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    let Some(pending) = &mut this.pending else { return };
                    match event {
                        AssistantEvent::Delta { text } => pending.text.push_str(&text),
                        AssistantEvent::Text { text } => pending.text = text,
                        AssistantEvent::Comment { outcome } => {
                            // The assistant changed the drafts mid-answer: show them right away.
                            let ok = outcome.error.is_none();
                            pending.comments.push(outcome);
                            if ok {
                                cx.emit(AssistantPanelEvent::DraftsChanged);
                            }
                        }
                        _ => {}
                    }
                    this.scroll.scroll_to_bottom();
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(thread) => {
                        this.placeholder_stale = true;
                        let comments = thread.messages.last().map(|m| m.comments.clone()).unwrap_or_default();
                        this.note = (!comments.is_empty()).then(|| comments_note(&comments));
                        this.active = Some(thread.id.clone());
                        this.threads.retain(|t| t.id != thread.id);
                        this.threads.push(thread);
                    }
                    Err(err) => this.note = Some(err),
                }
                this.pending = None;
                // A list read while the answer was streaming lacks it.
                this.load_threads(cx);
                this.scroll.scroll_to_bottom();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn start_sign_in(&mut self, provider: Provider, cx: &mut Context<Self>) {
        self.sign_in = Some(SignIn::Waiting(None));
        let (tx, mut rx) = mpsc::unbounded::<String>();
        let task = self.backend.sign_in_assistant(provider, move |url| {
            let _ = tx.unbounded_send(url);
        });
        cx.spawn(async move |this, cx| {
            while let Some(url) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    this.sign_in = Some(SignIn::Waiting(Some(url)));
                    cx.notify();
                });
            }
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.sign_in = Some(match result {
                    Ok(()) => SignIn::Done,
                    Err(e) => SignIn::Failed(e),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn ask_again(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.active.as_ref().and_then(|id| self.threads.iter().find(|t| t.id == *id)).cloned() else { return };
        let Some(last) = thread.messages.iter().rev().find(|m| m.role == "user") else { return };
        if self.pending.is_some() {
            return;
        }
        self.sign_in = None;
        self.ask(last.text.clone(), (thread.provider, thread.model.clone(), thread.effort.clone()), cx);
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        let task = self.backend.delete_assistant_thread(id.clone());
        cx.background_executor().spawn(async move {
            let _ = task.await;
        })
        .detach();
        self.threads.retain(|t| t.id != id);
        self.active = None;
        cx.notify();
    }

    /// References resolve to files of the diff (a bare name if only one file has it), or other
    /// files of the head, checked in batches (until a path's answer is back it isn't a link).
    fn code_refs(&self, cx: &mut Context<Self>) -> CodeRefs {
        let files = Rc::new(self.files.clone());
        let exists = Rc::new(self.exists.clone());
        let weak = cx.entity().downgrade();
        let resolve_weak = weak.clone();
        CodeRefs {
            resolve: Rc::new(move |r: &CodeRef| {
                let known = |path: &str| -> Option<String> {
                    if files.iter().any(|f| f == path) {
                        return Some(path.to_string());
                    }
                    let suffix = format!("/{path}");
                    let matches: Vec<&String> = files.iter().filter(|f| f.ends_with(&suffix)).collect();
                    (matches.len() == 1).then(|| matches[0].clone())
                };
                let stripped = r.path.strip_prefix("a/").or_else(|| r.path.strip_prefix("b/")).unwrap_or(&r.path).to_string();
                if let Some(path) = known(&r.path).or_else(|| known(&stripped)) {
                    return Some(CodeRef { path, ..r.clone() });
                }
                match exists.get(&r.path) {
                    Some(true) => Some(r.clone()),
                    Some(false) => None,
                    None => {
                        let path = r.path.clone();
                        let weak = resolve_weak.clone();
                        CHECKS.with(|c| c.borrow_mut().push((weak, path)));
                        None
                    }
                }
            }),
            open: Rc::new(move |r: &CodeRef, _: &mut Window, cx: &mut App| {
                let r = r.clone();
                let _ = weak.update(cx, |_, cx| cx.emit(AssistantPanelEvent::OpenRef(r)));
            }),
        }
    }

    /// Asks the backend which of the paths seen while rendering are files of the head.
    fn check_paths(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let paths: Vec<String> = CHECKS.with(|c| {
            let mut c = c.borrow_mut();
            let (mine, rest): (Vec<_>, Vec<_>) = c.drain(..).partition(|(w, _)| w == &me);
            *c = rest;
            mine.into_iter().map(|(_, p)| p).collect()
        });
        let paths: Vec<String> = paths.into_iter().filter(|p| self.asked.insert(p.clone())).collect();
        if paths.is_empty() {
            return;
        }
        let task = self.backend.existing_files(self.stack_id.clone(), self.hi, paths.clone());
        cx.spawn(async move |this, cx| {
            if let Ok(found) = task.await {
                let _ = this.update(cx, |this, cx| {
                    for p in paths {
                        let there = found.contains(&p);
                        this.exists.insert(p, there);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn render_options(&self, cx: &mut Context<Self>) -> RenderOptions {
        let mut opts = crate::ui::markdown_options(&self.backend, cx);
        opts.refs = Some(self.code_refs(cx));
        opts.image = Some(crate::ui::image_resolver());
        opts
    }

    fn comment_cards(&self, id: &str, comments: &[CommentOutcome], opts: &RenderOptions, cx: &App) -> AnyElement {
        let p = palette(cx).clone();
        div()
            .flex()
            .flex_col()
            .gap(u(6.))
            .children(comments.iter().enumerate().map(|(i, c)| {
                let (action, done, failed) = match c.action {
                    CommentAction::Add => ("Comment", "✓ draft added", "not added"),
                    CommentAction::Edit => ("Edit", "✓ updated", "not updated"),
                    CommentAction::Delete => ("Delete", "✓ deleted", "not deleted"),
                };
                let lines = c.lines.map(|(a, b)| if a == b { format!(":{a}") } else { format!(":{a}-{b}") }).unwrap_or_default();
                // An added comment's own lines say where it is, unless they're another PR's.
                let label = if c.action == CommentAction::Add && c.lines.is_some() { None } else { c.label.clone() };
                let where_: Vec<String> = [c.target.clone(), label].into_iter().flatten().collect();
                let status = match &c.error {
                    Some(e) => format!("{failed}: {e}"),
                    None => done.to_string(),
                };
                let path_ref = c.path.clone().map(|path| CodeRef { path, line: c.lines.map(|l| l.0), end: c.lines.map(|l| l.1).filter(|e| Some(*e) != c.lines.map(|l| l.0)) });
                let refs = opts.refs.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    .p(u(8.))
                    .rounded(u(6.))
                    .border_1()
                    .border_color(if c.error.is_some() { p.del_fg } else { p.border })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(u(6.))
                            .text_size(u(12.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(action))
                            .when_some(c.path.clone(), |d, path| {
                                let text = format!("{path}{lines}");
                                let link = path_ref.clone().and_then(|r| refs.as_ref().and_then(|refs| (refs.resolve)(&r)));
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("comment-ref-{id}-{i}")))
                                        .font_family(crate::ui::mono_family(cx))
                                        .when(link.is_some(), |d| d.text_color(p.accent).cursor_pointer())
                                        .child(text)
                                        .when_some(link.zip(refs.clone()), |d, (r, refs)| {
                                            d.on_click(move |_, window, cx| (refs.open)(&r, window, cx))
                                        }),
                                )
                            })
                            .when(!where_.is_empty(), |d| d.child(div().text_color(p.fg_muted).child(where_.join(" · "))))
                            .child(div().text_color(if c.error.is_some() { p.del_fg } else { p.add_fg }).child(status)),
                    )
                    .when_some(c.body.clone(), |d, body| d.child(crate::markdown::markdown(SharedString::from(format!("comment-body-{id}-{i}")), &body, true, opts)))
            }))
            .into_any_element()
    }

    fn dropdown(&self, id: &'static str, label: String, options: Vec<(String, String)>, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let menu = self.menus.get(id).filter(|_| enabled);
        let selected = options.iter().position(|(_, text)| *text == label);
        let (values, labels): (Vec<String>, Vec<SharedString>) = options.into_iter().map(|(v, t)| (v, t.into())).unzip();
        div()
            .id(id)
            .relative()
            .px(u(8.))
            .py(u(3.))
            .rounded(u(4.))
            .border_1()
            .border_color(p.border)
            .bg(p.bg_elevated)
            .text_size(u(12.))
            .when(!enabled, |d| d.opacity(0.6))
            .when(enabled, |d| d.cursor_pointer())
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(format!("{label} ▾"))
            .on_click(cx.listener(move |this, _, window, cx| {
                if !enabled {
                    return;
                }
                let values = values.clone();
                this.menus.toggle(
                    id,
                    labels.clone(),
                    selected,
                    |this: &mut Self| &mut this.menus,
                    move |this, i, window, cx| {
                        if let Some(value) = values.get(i) {
                            this.pick(id, value.clone(), window, cx);
                        }
                    },
                    window,
                    cx,
                );
                cx.notify();
            }))
            .when_some(menu, |d, menu| d.child(deferred(anchored().child(div().mt(u(20.)).child(menu)))))
            .into_any_element()
    }

    fn pick(&mut self, menu: &str, value: String, window: &mut Window, cx: &mut Context<Self>) {
        match menu {
            "assistant-threads" => {
                self.active = (!value.is_empty()).then_some(value);
                self.note = None;
                self.sign_in = None;
                self.scroll.scroll_to_bottom();
                self.update_placeholder(window, cx);
                self.question.update(cx, |q, cx| q.focus(window, cx));
            }
            _ => {}
        }
        cx.notify();
    }
}

/// Text shown as is (a question, an error, a URL), selectable.
fn plain_text(id: SharedString, text: &str, opts: &RenderOptions) -> AnyElement {
    let html: String = crate::logic::file_refs::escape_html(text).replace('\n', "<br>");
    crate::markdown::html(id, &format!("<p>{html}</p>"), opts)
}

thread_local! {
    /// Paths seen while rendering answers that need an existence check.
    static CHECKS: std::cell::RefCell<Vec<(gpui_kit::WeakEntity<AssistantPanel>, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// "Added 2 draft comments · updated 1 · 1 failed"
fn comments_note(comments: &[CommentOutcome]) -> String {
    let count = |action: CommentAction| comments.iter().filter(|c| c.error.is_none() && c.action == action).count();
    let mut parts: Vec<String> = Vec::new();
    for (verb, action) in [("Added", CommentAction::Add), ("Updated", CommentAction::Edit), ("Deleted", CommentAction::Delete)] {
        let n = count(action);
        if n == 0 {
            continue;
        }
        if parts.is_empty() {
            parts.push(format!("{verb} {n} draft comment{}", if n == 1 { "" } else { "s" }));
        } else {
            parts.push(format!("{} {n}", verb.to_lowercase()));
        }
    }
    let failed = comments.iter().filter(|c| c.error.is_some()).count();
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    parts.join(" · ")
}

impl Focusable for AssistantPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AssistantPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        if std::mem::take(&mut self.placeholder_stale) {
            self.update_placeholder(window, cx);
        }
        let opts = self.render_options(cx);
        let active = self.active.as_ref().and_then(|id| self.threads.iter().find(|t| t.id == *id)).cloned();
        let messages: Vec<AnyElement> = active
            .as_ref()
            .map(|t| {
                let last = t.messages.len().saturating_sub(1);
                t.messages
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let body = if m.role == "assistant" && !m.error {
                            crate::markdown::markdown(SharedString::from(format!("msg-{}-{i}", t.id)), &m.text, false, &opts)
                        } else {
                            // Plain text, selectable like the answers.
                            plain_text(SharedString::from(format!("msg-{}-{i}", t.id)), &m.text, &opts)
                        };
                        let sign_in = m.sign_in && i == last && self.pending.is_none();
                        div()
                            .flex()
                            .flex_col()
                            .gap(u(6.))
                            .p(u(10.))
                            .rounded(u(6.))
                            .when(m.role == "user", |d| d.bg(p.selection.opacity(0.5)))
                            .when(m.error, |d| d.text_color(p.del_fg))
                            .child(body)
                            .when(!m.comments.is_empty(), |d| d.child(self.comment_cards(&format!("{}-{i}", t.id), &m.comments, &opts, cx)))
                            .when(sign_in, |d| d.child(self.render_sign_in(t.provider, cx)))
                            .into_any_element()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pending = self.pending.as_ref().filter(|pd| pd.thread_id == self.active).map(|pd| {
            let answer = if pd.text.is_empty() {
                div().text_color(p.fg_muted).child("Thinking…").into_any_element()
            } else {
                crate::markdown::markdown("streaming", &pd.text, false, &opts)
            };
            div()
                .flex()
                .flex_col()
                .gap(u(8.))
                .child(div().p(u(10.)).rounded(u(6.)).bg(p.selection.opacity(0.5)).child(plain_text("pending-question".into(), &pd.question, &opts)))
                .child(
                    div()
                        .p(u(10.))
                        .flex()
                        .flex_col()
                        .gap(u(6.))
                        .child(answer)
                        .when(!pd.comments.is_empty(), |d| d.child(self.comment_cards("pending", &pd.comments, &opts, cx))),
                )
        });
        let about = match &self.context.selection {
            Some(s) => format!("{} L{}–{} · {}", s.path.rsplit('/').next().unwrap_or_default(), s.start_line, s.end_line, s.pr_label),
            None => self.context.range_label.clone(),
        };
        let empty_hint = match &self.context.selection {
            Some(s) => format!("Ask about {} L{}–{} ({}).", s.path.rsplit('/').next().unwrap_or_default(), s.start_line, s.end_line, s.pr_label),
            None => format!("Ask about {}. Select code first to ask about specific lines.", self.context.range_label),
        };
        let thread_options: Vec<(String, String)> = std::iter::once((String::new(), "New conversation".to_string()))
            .chain(self.threads.iter().map(|t| (t.id.clone(), t.messages.first().map(|m| m.text.chars().take(40).collect()).unwrap_or_else(|| "…".into()))))
            .collect();
        let thread_label = active.as_ref().and_then(|t| t.messages.first()).map(|m| m.text.chars().take(40).collect()).unwrap_or_else(|| "New conversation".to_string());
        let fresh = active.is_none();
        let busy = self.pending.is_some();
        let delete_id = active.as_ref().map(|t| t.id.clone());
        let el = div()
            .key_context("Assistant")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseAssistant, _, cx| cx.emit(AssistantPanelEvent::Close)))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                cx.stop_propagation();
                // An open menu closes first.
                if this.picker_open {
                    this.picker_open = false;
                    cx.notify();
                } else if this.menus.is_open() {
                    this.menus.close(window, cx);
                    cx.notify();
                } else {
                    cx.emit(AssistantPanelEvent::Close)
                }
            }))
            .relative()
            .flex_none()
            .w(u(self.width))
            .h_full()
            .flex()
            .flex_col()
            .bg(p.bg_sidebar)
            .border_l_1()
            .border_color(p.border)
            .font_family(crate::ui::sans_family(cx))
            .text_size(u(13.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .p(u(8.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().flex_1().min_w(px(0.)).child(self.dropdown("assistant-threads", thread_label, thread_options, true, cx)))
                    .when_some(delete_id, |d, id| d.child(link_button("delete", cx).id("assistant-delete").on_click(cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx)))))
                    .child(link_button("close", cx).id("assistant-close").on_click(cx.listener(|_, _, _, cx| cx.emit(AssistantPanelEvent::Close)))),
            )
            .child(
                div().relative().flex_1().min_h(px(0.)).flex().flex_col().child(
                div()
                    .id("assistant-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(u(10.))
                    .flex()
                    .flex_col()
                    .gap(u(8.))
                    .children(messages)
                    .children(pending)
                    .when(fresh && self.pending.is_none(), |d| d.child(div().text_color(p.fg_muted).child(empty_hint))),
                )
                .child(gpui_kit::base::Scrollbar::vertical(&self.scroll)),
            )
            .when_some(self.note.clone(), |d, note| d.child(div().px(u(10.)).py(u(4.)).text_size(u(12.)).text_color(p.fg_muted).child(note)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(6.))
                    .p(u(8.))
                    .border_t_1()
                    .border_color(p.border)
                    .when(fresh, |d| d.child(div().text_size(u(11.)).text_color(p.fg_muted).child(about)))
                    .child(self.render_composer(busy, cx)),
            );
        self.check_paths(cx);
        el
    }
}

impl AssistantPanel {
    /// The question box as one card: the text, then the agent / model / effort chip and send.
    fn render_composer(&mut self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let (provider, model, effort) = self.choice();
        let empty = self.question.read(cx).value().trim().is_empty();
        let part = |text: String, set: bool| div().text_color(if set { p.fg } else { p.fg_muted }).child(text);
        let chip = div()
            .id("assistant-agent")
            .flex()
            .items_center()
            .gap(u(5.))
            .px(u(8.))
            .py(u(3.))
            .rounded(u(6.))
            .text_size(u(12.))
            .cursor_pointer()
            .when(self.picker_open, |d| d.bg(p.selection))
            .hover(|d| d.bg(p.selection))
            .when(!self.picker_open, |d| d.tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Agent, model and reasoning effort").build(window, cx)))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx)))
            .child(div().size(u(7.)).rounded_full().bg(if provider == Provider::Claude { p.pr(1) } else { p.add_fg }))
            .child(div().text_color(p.fg).child(provider_name(provider)))
            .child(div().text_color(p.fg_muted).child("·"))
            .child(part(if model.is_empty() { "default model".into() } else { model.clone() }, !model.is_empty()))
            .child(div().text_color(p.fg_muted).child("·"))
            .child(part(if effort.is_empty() { "default effort".into() } else { effort.clone() }, !effort.is_empty()))
            .child(div().text_color(p.fg_muted).child("▾"));
        let send = div()
            .id("assistant-send")
            .size(u(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(p.accent)
            .text_color(p.bg_elevated)
            .font_weight(gpui_kit::FontWeight::BOLD)
            .child(if busy { "…" } else { "↑" })
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Ask (↵)").build(window, cx))
            .map(|d| if busy || empty { d.opacity(0.4).cursor_default() } else { d.cursor_pointer() })
            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)));
        div()
            .flex()
            .flex_col()
            .rounded(u(10.))
            .border_1()
            .border_color(p.border)
            .bg(p.bg_elevated)
            .child(div().px(u(4.)).pt(u(2.)).child(Textarea::new(&self.question).appearance(false)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .px(u(6.))
                    .pb(u(6.))
                    .child(div().flex_1())
                    .child(div().relative().child(chip).when(self.picker_open, |d| d.child(self.render_picker(cx))))
                    .child(send),
            )
            .into_any_element()
    }

    /// The agent / model / effort popup, above the chip. A conversation keeps its agent (only the
    /// CLI that started a session can resume it); model and effort apply to its next turns.
    fn render_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let (provider, model, effort) = self.choice();
        let locked = self.active.is_some();
        let heading = |text: &'static str| div().px(u(10.)).pt(u(8.)).pb(u(4.)).text_size(u(11.)).text_color(p.fg_muted).child(text);
        let row = |id: SharedString, label: String, on: bool, enabled: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(u(8.))
                .mx(u(4.))
                .px(u(8.))
                .py(u(4.))
                .rounded(u(5.))
                .text_size(u(12.))
                .map(|d| if enabled { d.cursor_pointer().hover(|d| d.bg(p.selection)) } else { d.opacity(0.45) })
                .child(div().flex_1().min_w(px(0.)).child(label))
                .child(div().w(u(12.)).text_color(p.accent).child(if on { "✓" } else { "" }))
        };
        let agents = [(Provider::Claude, "Claude Code"), (Provider::Codex, "Codex")].into_iter().map(|(which, name)| {
            let enabled = !locked || which == provider;
            row(SharedString::from(format!("agent-{}", provider_id(which))), name.to_string(), which == provider, enabled).on_click(cx.listener(move |this, _, window, cx| {
                if this.active.is_none() && this.provider != which {
                    this.change_provider(which, window, cx);
                }
                this.focus_question(window, cx);
            }))
        });
        let models: Vec<AnyElement> = if provider == Provider::Claude {
            CLAUDE_MODELS
                .iter()
                .map(|m| {
                    let value = m.to_string();
                    let label = if m.is_empty() { "Default".to_string() } else { capitalize(m) };
                    row(SharedString::from(format!("model-{m}")), label, model == *m, true)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_model(value.clone(), cx);
                            this.focus_question(window, cx);
                        }))
                        .into_any_element()
                })
                .collect()
        } else {
            vec![div().mx(u(8.)).child(Input::new(&self.codex_model)).into_any_element()]
        };
        let efforts = if provider == Provider::Claude { CLAUDE_EFFORTS.to_vec() } else { CODEX_EFFORTS.to_vec() };
        let efforts = efforts.into_iter().map(|e| {
            let value = e.to_string();
            let label = match e {
                "" => "Default".to_string(),
                "xhigh" => "Extra high".to_string(),
                e => capitalize(e),
            };
            row(SharedString::from(format!("effort-{e}")), label, effort == e, true).on_click(cx.listener(move |this, _, window, cx| {
                this.set_effort(value.clone(), cx);
                this.focus_question(window, cx);
            }))
        });
        gpui_kit::deferred(
            div().absolute().right_0().bottom(gpui_kit::relative(1.)).mb(u(6.)).child(
                div()
                    .id("assistant-picker")
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.picker_open = false;
                        cx.notify();
                    }))
                    .w(u(340.))
                    .flex()
                    .rounded(u(10.))
                    .border_1()
                    .border_color(p.floating_border.unwrap_or(p.border))
                    .bg(p.bg_elevated)
                    .when(p.shadows, |d| d.shadow_lg())
                    .pb(u(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(heading("Agent"))
                            .children(agents)
                            .when(locked, |d| d.child(div().px(u(12.)).pt(u(2.)).text_size(u(11.)).text_color(p.fg_muted).child("Fixed for this conversation")))
                            .child(heading("Model"))
                            .children(models),
                    )
                    .child(
                        div()
                            .w(u(130.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(p.border)
                            .child(heading("Reasoning"))
                            .children(efforts),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn render_sign_in(&self, provider: Provider, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let row = div().flex().items_center().gap(u(8.)).text_size(u(12.));
        match &self.sign_in {
            None => row
                .child(button(format!("Sign in to {} again", provider_name(provider)), true, cx).id("assistant-sign-in").on_click(cx.listener(move |this, _, _, cx| this.start_sign_in(provider, cx))))
                .into_any_element(),
            Some(SignIn::Waiting(url)) => row
                .child(if url.is_some() { "Finish signing in in your browser…" } else { "Starting sign-in…" })
                .when_some(url.clone(), |d, url| d.child(div().text_color(p.fg_muted).child(plain_text("sign-in-url".into(), &url, &crate::ui::markdown_options(&self.backend, cx)))))
                .into_any_element(),
            Some(SignIn::Done) => row
                .child("Signed in.")
                .child(button("Ask again", true, cx).id("assistant-ask-again").on_click(cx.listener(|this, _, _, cx| this.ask_again(cx))))
                .into_any_element(),
            Some(SignIn::Failed(message)) => row
                .child(div().text_color(p.del_fg).child(message.clone()))
                .child(link_button("try again", cx).id("assistant-retry-sign-in").on_click(cx.listener(move |this, _, _, cx| this.start_sign_in(provider, cx))))
                .into_any_element(),
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
