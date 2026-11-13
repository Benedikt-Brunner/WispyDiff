//! ⇧⇧: the diff's files by fuzzy name, Spotlight style.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, KeyBinding,
    Render, StyledText, Subscription, Window,
};
use wispy_core::model::FileSummary;

use crate::logic::fuzzy::{search_files, SearchResult};
use crate::ui::{floating, palette, u};

gpui_kit::actions!(file_search, [Down, Up, Close]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("FileSearch");
    cx.bind_keys([KeyBinding::new("down", Down, ctx), KeyBinding::new("up", Up, ctx), KeyBinding::new("escape", Close, ctx)]);
}

pub enum FileSearchEvent {
    Pick(usize),
    Close,
}

pub struct FileSearch {
    files: Vec<FileSummary>,
    filtered: Vec<bool>,
    viewed: Vec<bool>,
    input: Entity<InputState>,
    results: Vec<SearchResult>,
    selected: usize,
    scroll: gpui_kit::ScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FileSearchEvent> for FileSearch {}

impl FileSearch {
    pub fn new(files: Vec<FileSummary>, filtered: Vec<bool>, viewed: Vec<bool>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a file in this diff"));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                this.results = search_files(&this.files, &this.filtered, &query);
                this.selected = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                if let Some(r) = this.results.get(this.selected) {
                    cx.emit(FileSearchEvent::Pick(r.index));
                }
            }
            _ => {}
        })];
        input.update(cx, |input, cx| input.focus(window, cx));
        let results = search_files(&files, &filtered, "");
        FileSearch { files, filtered, viewed, input, results, selected: 0, scroll: gpui_kit::ScrollHandle::new(), focus: cx.focus_handle(), _subscriptions: subscriptions }
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
        let selected = self.selected.min(self.results.len().saturating_sub(1));
        // The selected result stays in view.
        self.scroll.scroll_to_item(selected);
        floating(620., cx)
            .key_context("FileSearch")
            .on_action(cx.listener(|this, _: &Down, _, cx| {
                this.selected = (this.selected + 1).min(this.results.len().saturating_sub(1));
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
                this.selected = (this.selected + 1).min(this.results.len().saturating_sub(1));
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
                if self.results.is_empty() {
                    d.child(div().px(u(14.)).pb(u(12.)).text_size(u(12.)).text_color(p.fg_muted).child("No file matches."))
                } else {
                    d.child(div().id("file-search-results").track_scroll(&self.scroll).border_t_1().border_color(p.border).py(u(4.)).overflow_y_scroll().max_h(u(460.)).children(
                        self.results.iter().enumerate().map(|(i, r)| {
                            let file = &self.files[r.index];
                            let name_start = file.path.rfind('/').map_or(0, |at| at + 1);
                            let index = r.index;
                            let tip = file.path.clone();
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
                                .when(self.filtered[r.index], |d| d.opacity(0.55))
                                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                    if this.selected != i {
                                        this.selected = i;
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(FileSearchEvent::Pick(index))))
                                .child(div().w(u(12.)).text_color(p.add_fg).child(if self.viewed[r.index] { "✓" } else { "" }))
                                .child(div().flex_none().child(marked(&file.path, &r.positions, name_start, file.path.len(), mark)))
                                .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_size(u(11.)).text_color(p.fg_muted).child(marked(&file.path, &r.positions, 0, name_start, mark)))
                                .when(self.filtered[r.index], |d| d.child(div().text_size(u(11.)).text_color(p.fg_muted).child("hidden by filter")))
                        }),
                    ))
                }
            })
    }
}
