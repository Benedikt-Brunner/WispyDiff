//! ⇧⇧: the diff's files by fuzzy name, Spotlight style, then the rest of the head's files.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, KeyBinding,
    Render, StyledText, Subscription, Task, Window,
};
use wispy_core::model::FileSummary;

use crate::logic::fuzzy::{rank_paths, search_files, SearchResult, MAX_RESULTS};
use crate::ui::{floating, palette, u};

gpui_kit::actions!(file_search, [Down, Up, Close]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("FileSearch");
    cx.bind_keys([KeyBinding::new("down", Down, ctx), KeyBinding::new("up", Up, ctx), KeyBinding::new("escape", Close, ctx)]);
}

pub enum FileSearchEvent {
    /// A file of the diff, by index.
    Pick(usize),
    /// A file of the head outside the diff (opens read-only).
    Open(String),
    Close,
}

pub struct FileSearch {
    files: Vec<FileSummary>,
    filtered: Vec<bool>,
    viewed: Vec<bool>,
    /// The head's files outside the diff, once listed.
    others: Arc<Vec<String>>,
    input: Entity<InputState>,
    query: String,
    results: Vec<SearchResult>,
    /// Matches among `others`, listed after the diff's (ranked in the background: a repository
    /// can have many files).
    other_results: Vec<SearchResult>,
    ranking: Option<Task<()>>,
    selected: usize,
    scroll: gpui_kit::ScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FileSearchEvent> for FileSearch {}

impl FileSearch {
    pub fn new(files: Vec<FileSummary>, filtered: Vec<bool>, viewed: Vec<bool>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a file"));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.query = input.read(cx).value().to_string();
                this.results = search_files(&this.files, &this.filtered, &this.query);
                this.selected = 0;
                this.rank_others(cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.pick(this.selected, cx),
            _ => {}
        })];
        input.update(cx, |input, cx| input.focus(window, cx));
        let results = search_files(&files, &filtered, "");
        FileSearch {
            files,
            filtered,
            viewed,
            others: Arc::default(),
            input,
            query: String::new(),
            results,
            other_results: vec![],
            ranking: None,
            selected: 0,
            scroll: gpui_kit::ScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The head's files: those outside the diff are searched too.
    pub fn set_head_files(&mut self, head: Vec<String>, cx: &mut Context<Self>) {
        let diff: std::collections::HashSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        self.others = Arc::new(head.into_iter().filter(|p| !diff.contains(p.as_str())).collect());
        self.rank_others(cx);
    }

    /// Files outside the diff show only once something is typed.
    fn rank_others(&mut self, cx: &mut Context<Self>) {
        if self.others.is_empty() || crate::logic::js_trim(&self.query).is_empty() {
            self.other_results.clear();
            self.ranking = None;
            cx.notify();
            return;
        }
        let (others, query) = (self.others.clone(), self.query.clone());
        let ranked = cx.background_executor().spawn(async move { rank_paths(&others, &[], &query) });
        // A newer query replaces (and so cancels) the ranking still running.
        self.ranking = Some(cx.spawn(async move |this, cx| {
            let ranked = ranked.await;
            let _ = this.update(cx, |this, cx| {
                this.other_results = ranked;
                cx.notify();
            });
        }));
    }

    /// The other files' matches shown: the list holds at most `MAX_RESULTS`.
    fn shown_others(&self) -> &[SearchResult] {
        let room = MAX_RESULTS.saturating_sub(self.results.len());
        &self.other_results[..self.other_results.len().min(room)]
    }

    fn count(&self) -> usize {
        self.results.len() + self.shown_others().len()
    }

    fn pick(&self, i: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.results.get(i) {
            cx.emit(FileSearchEvent::Pick(r.index));
        } else if let Some(r) = self.shown_others().get(i - self.results.len()) {
            cx.emit(FileSearchEvent::Open(self.others[r.index].clone()));
        }
    }

    /// What the results list shows (for tests): each result's path and whether it's in the diff.
    pub fn listed(&self) -> Vec<(String, bool)> {
        let diff = self.results.iter().map(|r| (self.files[r.index].path.clone(), true));
        diff.chain(self.shown_others().iter().map(|r| (self.others[r.index].clone(), false))).collect()
    }
}

/// `path[from..to)` with the matched characters (byte offsets) marked.
fn marked(path: &str, positions: &[usize], from: usize, to: usize, mark: HighlightStyle) -> StyledText {
    let text = path[from..to].to_string();
    let highlights: Vec<_> = positions
        .iter()
        .filter(|p| **p >= from && **p < to)
        .map(|&p| {
            let len = path[p..].chars().next().map_or(1, char::len_utf8);
            (p - from..p - from + len, mark)
        })
        .collect();
    StyledText::new(text).with_highlights(highlights)
}

impl Focusable for FileSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FileSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let mark = HighlightStyle { color: Some(p.accent), font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let count = self.count();
        let selected = self.selected.min(count.saturating_sub(1));
        let diff_count = self.results.len();
        // The selected result stays in view (the other files' heading is a child of its own).
        self.scroll.scroll_to_item(if selected < diff_count { selected } else { selected + 1 });
        // `check`: the viewed mark (diff files only).
        let row = |i: usize, path: &str, positions: &[usize], check: bool, cx: &mut Context<Self>| {
            let name_start = path.rfind('/').map_or(0, |at| at + 1);
            let tip = path.to_string();
            div()
                .id(("file-search", i))
                .flex()
                .items_center()
                .gap(u(8.))
                .px(u(14.))
                .py(u(5.))
                .text_size(u(13.))
                .cursor_pointer()
                .when(i == selected, |d| d.bg(p.selection))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.selected != i {
                        this.selected = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.pick(i, cx)))
                .child(div().w(u(12.)).text_color(p.add_fg).child(if check { "✓" } else { "" }))
                .child(div().flex_none().child(marked(path, positions, name_start, path.len(), mark)))
                .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_size(u(11.)).text_color(p.fg_muted).child(marked(path, positions, 0, name_start, mark)))
        };
        let diff_rows: Vec<_> = self
            .results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let file = &self.files[r.index];
                let (filtered, viewed) = (self.filtered[r.index], self.viewed[r.index]);
                row(i, &file.path, &r.positions, viewed, cx)
                    .when(filtered, |d| d.opacity(0.55))
                    .when(filtered, |d| d.child(div().text_size(u(11.)).text_color(p.fg_muted).child("hidden by filter")))
                    .into_any_element()
            })
            .collect();
        let other_rows: Vec<_> = self
            .shown_others()
            .iter()
            .enumerate()
            .map(|(i, r)| row(diff_count + i, &self.others[r.index], &r.positions, false, cx).into_any_element())
            .collect();
        floating(620., cx)
            .key_context("FileSearch")
            .on_action(cx.listener(|this, _: &Down, _, cx| {
                this.selected = (this.selected + 1).min(this.count().saturating_sub(1));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Up, _, cx| {
                this.selected = this.selected.saturating_sub(1);
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &Close, _, cx| cx.emit(FileSearchEvent::Close)))
            // The input takes ↑/↓/Esc itself: the search needs them first.
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::MoveDown, _, cx| {
                cx.stop_propagation();
                this.selected = (this.selected + 1).min(this.count().saturating_sub(1));
                cx.notify();
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::MoveUp, _, cx| {
                cx.stop_propagation();
                this.selected = this.selected.saturating_sub(1);
                cx.notify();
            }))
            .capture_action(cx.listener(|_, _: &gpui_kit::component::input::Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(FileSearchEvent::Close)
            }))
            .child(div().p(u(8.)).child(Input::new(&self.input).appearance(false).text_size(u(15.))))
            .map(|d| {
                if count == 0 {
                    d.child(div().px(u(14.)).pb(u(12.)).text_size(u(12.)).text_color(p.fg_muted).child("No file matches."))
                } else {
                    let has_others = !other_rows.is_empty();
                    d.child(
                        div()
                            .id("file-search-results")
                            .track_scroll(&self.scroll)
                            .border_t_1()
                            .border_color(p.border)
                            .py(u(4.))
                            .overflow_y_scroll()
                            .max_h(u(460.))
                            .children(diff_rows)
                            .when(has_others, |d| {
                                d.child(
                                    div()
                                        .px(u(14.))
                                        .pt(u(if diff_count > 0 { 8. } else { 4. }))
                                        .pb(u(4.))
                                        .when(diff_count > 0, |d| d.mt(u(4.)).border_t_1().border_color(p.border))
                                        .text_size(u(11.))
                                        .text_color(p.fg_muted)
                                        .child("Not in this diff"),
                                )
                            })
                            .children(other_rows),
                    )
                }
            })
    }
}
