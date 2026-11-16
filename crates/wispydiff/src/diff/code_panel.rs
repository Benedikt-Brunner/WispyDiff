//! Usages of a symbol in the touched files, and whole-repo search (streamed `git grep`).

use std::sync::Arc;

use futures::channel::mpsc;
use futures::StreamExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, Render, SharedString,
    Subscription, Task, Window,
};
use wispy_core::service::GrepHit;
use wispy_core::symbols::{Hit, Usages};

use crate::backend::{Backend, GrepEvent};
use crate::ui::{link_button, palette, u};

gpui_kit::actions!(code_panel, [ClosePanel]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ClosePanel, Some("CodePanel"))]);
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelKind {
    Usages(String),
    Search(String),
}

pub enum CodePanelEvent {
    Jump { path: String, line: u32, file: Option<usize> },
    Close,
    Resized(f32),
}

#[derive(Clone, PartialEq)]
enum GrepStatus {
    Idle,
    Running,
    Done,
    Failed(String),
}

pub struct CodePanel {
    backend: Arc<Backend>,
    view_id: String,
    stack_id: String,
    hi: usize,
    kind: PanelKind,
    input: Entity<InputState>,
    usages: Option<Usages>,
    /// Usages: the whole repo (git grep) instead of this diff's files.
    repo_scope: bool,
    hits: Vec<GrepHit>,
    status: GrepStatus,
    unsearched: usize,
    run: u64,
    pub width: f32,
    focus: FocusHandle,
    _grep: Option<Task<()>>,
    /// Perf marks for results that arrived; marked once the frame showing them is presented.
    marks: Vec<&'static str>,
    scroll: gpui_kit::ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CodePanelEvent> for CodePanel {}

impl CodePanel {
    pub fn new(backend: Arc<Backend>, view_id: String, stack_id: String, hi: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search the whole repo…"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let query = input.read(cx).value().trim().to_string();
                if !query.is_empty() {
                    this.search(query, cx);
                }
            }
        })];
        let width = crate::settings::get::<f32>(cx, "width.code").filter(|w| *w > 0.).unwrap_or(380.);
        CodePanel {
            backend,
            view_id,
            stack_id,
            hi,
            kind: PanelKind::Search(String::new()),
            input,
            usages: None,
            repo_scope: false,
            hits: Vec::new(),
            status: GrepStatus::Idle,
            unsearched: 0,
            run: 0,
            width,
            focus: cx.focus_handle(),
            _grep: None,
            marks: Vec::new(),
            scroll: gpui_kit::ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.width = width;
        cx.notify();
    }

    pub fn show_usages(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.kind = PanelKind::Usages(name.clone());
        self.usages = None;
        self.repo_scope = false;
        self.hits.clear();
        self.status = GrepStatus::Idle;
        self.input.update(cx, |input, cx| input.set_value(name.clone(), window, cx));
        let task = self.backend.usages(self.view_id.clone(), name.clone());
        cx.spawn(async move |this, cx| match task.await {
            Ok(result) => {
                let _ = this.update(cx, |this, cx| {
                    if this.kind == PanelKind::Usages(name) {
                        this.usages = Some(result);
                        this.marks.push("usages:result");
                        cx.notify();
                    }
                });
            }
            Err(err) => eprintln!("usages failed: {err}"),
        })
        .detach();
        cx.notify();
    }

    /// The diff moved to another range: searches and usages look there (usages again at once).
    pub fn set_view(&mut self, view_id: String, stack_id: String, hi: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.view_id == view_id {
            return;
        }
        self.view_id = view_id;
        self.stack_id = stack_id;
        self.hi = hi;
        if let PanelKind::Usages(name) = self.kind.clone() {
            self.show_usages(name, window, cx);
        }
    }

    pub fn show_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.kind = PanelKind::Search(query.clone());
        self.input.update(cx, |input, cx| {
            input.set_value(query.clone(), window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Streams `git grep` hits for `query` (whole words for identifiers).
    pub fn search(&mut self, query: String, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        crate::perf_marks::mark("grep:start");
        if matches!(self.kind, PanelKind::Usages(_)) {
            self.repo_scope = true;
        } else {
            self.kind = PanelKind::Search(query.clone());
        }
        self.hits.clear();
        self.status = GrepStatus::Running;
        self.unsearched = 0;
        let whole_word = !query.is_empty() && query.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        let (tx, mut rx) = mpsc::unbounded::<GrepEvent>();
        let task = self.backend.grep(self.stack_id.clone(), self.hi, query, whole_word, move |event| {
            let _ = tx.unbounded_send(event);
        });
        self._grep = Some(cx.spawn(async move |this, cx| {
            let mut first = true;
            while let Some(event) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if this.run != run {
                        return;
                    }
                    match event {
                        GrepEvent::Hits { hits } => {
                            if first {
                                first = false;
                                this.marks.push("grep:first-hit");
                            }
                            this.hits.extend(hits);
                        }
                        GrepEvent::Done { unsearched, .. } => {
                            this.unsearched = unsearched;
                            this.status = GrepStatus::Done;
                        }
                        GrepEvent::Failed { message } => this.status = GrepStatus::Failed(message),
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            if let Err(err) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.run == run {
                        this.status = GrepStatus::Failed(err);
                        cx.notify();
                    }
                });
            }
        }));
        cx.notify();
    }

    fn section(&self, title: &str, hits: &[Hit], cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if hits.is_empty() {
            return None;
        }
        let p = palette(cx).clone();
        Some(
            div()
                .flex()
                .flex_col()
                .child(section_title(title, hits.len(), &p))
                .children(hits.iter().enumerate().map(|(i, hit)| {
                    let (path, line, file) = (hit.path.clone(), hit.line, hit.file as usize);
                    hit_row(
                        SharedString::from(format!("{title}-{i}")),
                        format!("{}:{}", hit.path.rsplit('/').next().unwrap_or_default(), hit.line),
                        hit.text.clone(),
                        &p,
                        cx,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(CodePanelEvent::Jump { path: path.clone(), line, file: Some(file) })))
                }))
                .into_any_element(),
        )
    }
}

fn section_title(title: &str, count: usize, p: &crate::theme::Palette) -> impl IntoElement {
    div()
        .flex()
        .gap(u(6.))
        .px(u(12.))
        .pt(u(10.))
        .pb(u(4.))
        .text_size(u(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.fg_muted)
        .child(title.to_uppercase())
        .child(div().font_weight(FontWeight::NORMAL).child(count.to_string()))
}

fn hit_row(id: SharedString, place: String, text: String, p: &crate::theme::Palette, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .px(u(12.))
        .py(u(3.))
        .cursor_pointer()
        .hover(|d| d.bg(p.selection))
        .child(div().text_size(u(11.)).text_color(p.accent).child(place))
        .child(div().text_size(u(12.)).font_family(crate::ui::mono_family(cx)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(text.trim().to_string()))
}

impl Focusable for CodePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CodePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Results count as shown once the frame drawing them is presented.
        for name in self.marks.drain(..) {
            window.on_next_frame(move |_, _| crate::perf_marks::mark(name));
        }
        let p = palette(cx).clone();
        let usages = match (&self.kind, &self.usages) {
            (PanelKind::Usages(_), Some(u)) => Some(u.clone()),
            _ => None,
        };
        let repo_scope = self.repo_scope;
        let grep = (usages.is_none() || repo_scope) && (self.status != GrepStatus::Idle || !self.hits.is_empty());
        div()
            .key_context("CodePanel")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &ClosePanel, _, cx| cx.emit(CodePanelEvent::Close)))
            .capture_action(cx.listener(|_, _: &gpui_kit::component::input::Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(CodePanelEvent::Close)
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .p(u(8.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().flex_1().child(Input::new(&self.input)))
                    .child(link_button("close", cx).id("code-panel-close").on_click(cx.listener(|_, _, _, cx| cx.emit(CodePanelEvent::Close)))),
            )
            .child(
                div().relative().flex_1().min_h(px(0.)).flex().flex_col().child(
                div()
                    .id("code-panel-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .pb(u(12.))
                    .when_some(usages, |d, usages| {
                        let empty = usages.definitions.len() + usages.changed.len() + usages.unchanged.len() == 0;
                        let name = usages.name.clone();
                        let scope = |id: &'static str, label: &'static str, on: bool| {
                            div()
                                .id(id)
                                .px(u(8.))
                                .py(u(2.))
                                .rounded(u(4.))
                                .cursor_pointer()
                                .text_color(if on { p.fg } else { p.fg_muted })
                                .when(on, |d| d.bg(p.selection))
                                .when(!on, |d| d.hover(|d| d.text_color(p.fg)))
                                .child(label)
                        };
                        // Where the usages come from, switchable at the top.
                        d.child(
                            div()
                                .px(u(12.))
                                .pt(u(10.))
                                .flex()
                                .flex_col()
                                .gap(u(6.))
                                .text_size(u(12.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(u(4.))
                                        .child("Usages of")
                                        .child(div().font_family(crate::ui::mono_family(cx)).child(usages.name.clone()))
                                        .child(if repo_scope { "in the whole repo" } else { "in this diff's files" }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(u(2.))
                                        .p(u(2.))
                                        .rounded(u(6.))
                                        .border_1()
                                        .border_color(p.border)
                                        .self_start()
                                        .child(scope("scope-diff", "This diff", !repo_scope).on_click(cx.listener(|this, _, _, cx| {
                                            this.repo_scope = false;
                                            cx.notify();
                                        })))
                                        .child(scope("search-repo", "Whole repo", repo_scope).on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.repo_scope {
                                                this.search(name.clone(), cx);
                                            }
                                        }))),
                                ),
                        )
                        .when(!repo_scope, |d| {
                            d.children(self.section("Definitions", &usages.definitions, cx))
                                .children(self.section("In changed lines", &usages.changed, cx))
                                .children(self.section("In unchanged code", &usages.unchanged, cx))
                                .when(empty, |d| d.child(div().px(u(12.)).pt(u(8.)).text_size(u(12.)).text_color(p.fg_muted).child("No uses in the touched files.")))
                        })
                    })
                    .when(grep, |d| {
                        let status = self.status.clone();
                        d.child(
                            div()
                                .flex()
                                .gap(u(6.))
                                .px(u(12.))
                                .pt(u(10.))
                                .pb(u(4.))
                                .text_size(u(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.fg_muted)
                                .child("REPOSITORY")
                                .child(div().font_weight(FontWeight::NORMAL).child(self.hits.len().to_string()))
                                .when(status == GrepStatus::Running, |d| d.child(div().font_weight(FontWeight::NORMAL).child("· searching…"))),
                        )
                        .children(self.hits.iter().enumerate().map(|(i, hit)| {
                            let (path, line) = (hit.path.clone(), hit.line);
                            hit_row(SharedString::from(format!("grep-{i}")), format!("{}:{}", hit.path, hit.line), hit.text.clone(), &p, cx)
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(CodePanelEvent::Jump { path: path.clone(), line, file: None })))
                        }))
                        .when_some(match &status {
                            GrepStatus::Failed(m) => Some(m.clone()),
                            _ => None,
                        }, |d, m| d.child(div().px(u(12.)).pt(u(6.)).text_size(u(12.)).text_color(p.del_fg).child(m)))
                        .when(status == GrepStatus::Done && self.hits.is_empty(), |d| {
                            d.child(div().px(u(12.)).pt(u(6.)).text_size(u(12.)).text_color(p.fg_muted).child("No matches."))
                        })
                        .when(status == GrepStatus::Done && self.unsearched > 0, |d| {
                            let n = self.unsearched;
                            d.child(div().px(u(12.)).pt(u(6.)).text_size(u(12.)).text_color(p.fg_muted).child(format!(
                                "Offline — {n} file{} not downloaded yet {} searched.",
                                if n == 1 { "" } else { "s" },
                                if n == 1 { "wasn’t" } else { "weren’t" }
                            )))
                        })
                    }),
                )
                .child(gpui_kit::base::Scrollbar::vertical(&self.scroll)),
            )
    }
}
