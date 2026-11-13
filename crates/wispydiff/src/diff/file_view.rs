//! A whole file from the head, read-only (search hits outside the diff, files the filter hides).

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, font, px, uniform_list, AnyElement, Context, FontWeight, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollStrategy,
    StyledText, TextLayout, UniformListScrollHandle, Window,
};
use wispy_core::highlight::Seg;

use super::DiffViewer;
use crate::text::{code_line, highlight_bytes, original_byte};
use crate::ui::{link_button, palette, u};

pub struct FileViewState {
    pub path: String,
    pub line: u32,
    /// Last line to highlight (`line` if just one).
    pub end: u32,
    /// In the diff, but left out by the file filter.
    pub filtered: bool,
    pub lines: Option<Rc<Vec<Vec<Seg>>>>,
    pub error: Option<String>,
    scroll: UniformListScrollHandle,
    /// The longest line: the list is as wide as it (scrolls sideways).
    longest: usize,
    /// Selected text, by (line index, byte of the tab-expanded line).
    pub selection: Option<FileSelection>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileSelection {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
    pub active: bool,
}

impl FileSelection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The selected bytes of line `i` (`len` bytes long, tabs expanded).
    fn range_in(&self, i: usize, len: usize) -> Option<(usize, usize)> {
        let (a, b) = self.ordered();
        if a == b || i < a.0 || i > b.0 {
            return None;
        }
        let start = if i == a.0 { a.1.min(len) } else { 0 };
        // A whole line selected through its end includes the newline: show a sliver past it.
        let end = if i == b.0 { b.1.min(len) } else { len };
        Some((start, end))
    }
}

impl FileViewState {
    pub fn new(path: String, line: u32, end: u32, filtered: bool) -> Self {
        FileViewState {
            path,
            line,
            end: end.max(line),
            filtered,
            lines: None,
            error: None,
            scroll: UniformListScrollHandle::new(),
            longest: 0,
            selection: None,
        }
    }

    /// The selected text, with the file's own tabs.
    pub fn selected_text(&self) -> Option<String> {
        let (sel, lines) = (self.selection?, self.lines.as_ref()?);
        let ((a_line, a_byte), (b_line, b_byte)) = sel.ordered();
        if (a_line, a_byte) == (b_line, b_byte) {
            return None;
        }
        let mut out = Vec::new();
        for i in a_line..=b_line.min(lines.len().saturating_sub(1)) {
            let original = crate::diff::seg_text(&lines[i]);
            let start = if i == a_line { original_byte(&original, a_byte) } else { 0 };
            let end = if i == b_line { original_byte(&original, b_byte) } else { original.len() };
            out.push(original[start.min(end)..end].to_string());
        }
        Some(out.join("\n"))
    }

    pub fn set_lines(&mut self, lines: Vec<Vec<Seg>>) {
        self.longest = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, l)| l.iter().map(|(_, t)| t.len() + t.matches('\t').count() * 3).sum::<usize>())
            .map_or(0, |(i, _)| i);
        self.lines = Some(Rc::new(lines));
        // A few lines of context above the target.
        self.scroll.scroll_to_item_strict(self.line.saturating_sub(6) as usize, ScrollStrategy::Top);
    }
}

impl DiffViewer {
    pub fn render_file_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let Some(view) = &self.file_view else { return div().into_any_element() };
        let (line, end) = (view.line, view.end);
        let lines = view.lines.clone().unwrap_or_default();
        let count = lines.len();
        let digits = count.max(1).to_string().len().max(3);
        let mono = font(crate::ui::mono_family(cx));
        let palette_for_rows = p.clone();
        let selection = view.selection;
        let weak = cx.entity().downgrade();
        let rows = uniform_list("file-view-rows", count, move |range, _, _| {
            range
                .map(|i| {
                    let n = i as u32 + 1;
                    let flashed = n >= line && n <= end;
                    let (text, mut runs) = code_line(&lines[i], &[], &mono, &palette_for_rows, palette_for_rows.add_word);
                    let selected = selection.and_then(|s| s.range_in(i, text.len()));
                    if let Some((a, b)) = selected {
                        highlight_bytes(&mut runs, a, b, palette_for_rows.selection);
                    }
                    // A selection running on past this line's end.
                    let past_end = selection.is_some_and(|s| {
                        let (a, b) = s.ordered();
                        a != b && i >= a.0 && i < b.0
                    });
                    let styled = StyledText::new(text).with_runs(runs);
                    let layout = styled.layout().clone();
                    let (down_layout, move_layout) = (layout.clone(), layout);
                    let (down, moved, up) = (weak.clone(), weak.clone(), weak.clone());
                    div()
                        .h(u(20.))
                        .flex()
                        .when(flashed, |d| d.bg(palette_for_rows.selection))
                        .child(div().w(u(digits as f32 * 7.5 + 16.)).flex_none().pr(u(8.)).flex().justify_end().text_color(palette_for_rows.fg_muted).child(n.to_string()))
                        .child(
                            div()
                                .flex_1()
                                .pl(u(8.))
                                .flex()
                                .whitespace_nowrap()
                                .cursor_text()
                                .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                                    let at = index_at(&down_layout, event.position);
                                    let _ = down.update(cx, |this, cx| {
                                        if let Some(view) = &mut this.file_view {
                                            let anchor = match (event.modifiers.shift, view.selection) {
                                                (true, Some(s)) => s.anchor,
                                                _ => (i, at),
                                            };
                                            view.selection = Some(FileSelection { anchor, head: (i, at), active: true });
                                            cx.notify();
                                        }
                                    });
                                })
                                .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                                    if event.pressed_button != Some(MouseButton::Left) {
                                        return;
                                    }
                                    let at = index_at(&move_layout, event.position);
                                    let _ = moved.update(cx, |this, cx| {
                                        if let Some(sel) = this.file_view.as_mut().and_then(|v| v.selection.as_mut()) {
                                            if sel.active && sel.head != (i, at) {
                                                sel.head = (i, at);
                                                cx.notify();
                                            }
                                        }
                                    });
                                })
                                .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                                    let _ = up.update(cx, |this, cx| {
                                        if let Some(sel) = this.file_view.as_mut().and_then(|v| v.selection.as_mut()) {
                                            sel.active = false;
                                            cx.notify();
                                        }
                                    });
                                })
                                .child(styled)
                                .when(past_end, |d| d.child(div().w(u(6.)).h_full().bg(palette_for_rows.selection))),
                        )
                })
                .collect()
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(view.longest))
        .track_scroll(&view.scroll)
        .size_full();
        let note = if view.filtered { "read-only · hidden by the file filter" } else { "read-only · not part of this diff" };
        let body = div()
            .id("file-view")
            .w(u(1100.))
            .max_w(gpui_kit::relative(0.9))
            .h(u(680.))
            .max_h(gpui_kit::relative(0.8))
            .flex()
            .flex_col()
            .rounded(u(10.))
            .border_1()
            .border_color(p.floating_border.unwrap_or(p.border))
            .bg(p.bg_elevated)
            .when(p.shadows, |d| d.shadow_lg())
            .overflow_hidden()
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .px(u(14.))
                    .py(u(8.))
                    .border_b_1()
                    .border_color(p.border)
                    .font_family(crate::ui::sans_family(cx))
                    .child(div().text_size(u(13.)).font_weight(FontWeight::SEMIBOLD).child(view.path.clone()))
                    .child(div().text_size(u(11.)).text_color(p.fg_muted).child(note))
                    .child(div().flex_1())
                    .child(link_button("close", cx).id("file-view-close").on_click(cx.listener(|this, _, _, cx| {
                        this.file_view = None;
                        cx.notify();
                    }))),
            )
            .when_some(view.error.clone(), |d, e| d.child(div().p(u(12.)).text_size(u(12.)).text_color(p.del_fg).child(e)))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .font_family(crate::ui::mono_family(cx))
                    .text_size(u(12.))
                    .child(rows)
                    .child(gpui_kit::base::Scrollbar::new(&view.scroll)),
            );
        let (back, forward, close) = (cx.entity().downgrade(), cx.entity().downgrade(), cx.entity().downgrade());
        // Like `ui::backdrop`, but the mouse's back / forward buttons work over it too.
        div()
            .id("file-view-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(u(90.))
            .bg(gpui_kit::Hsla { a: if p.dark { 0.45 } else { 0.18 }, ..gpui_kit::black() })
            .occlude()
            .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, cx| {
                let _ = close.update(cx, |this, cx| {
                    this.file_view = None;
                    cx.notify();
                });
            })
            .on_mouse_up(gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Back), move |_, window, cx| {
                let _ = back.update(cx, |this, cx| this.go_history(true, window, cx));
            })
            .on_mouse_up(gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Forward), move |_, window, cx| {
                let _ = forward.update(cx, |this, cx| this.go_history(false, window, cx));
            })
            .child(body)
            .into_any_element()
    }
}

fn index_at(layout: &TextLayout, position: gpui_kit::Point<gpui_kit::Pixels>) -> usize {
    match layout.index_for_position(position) {
        Ok(i) | Err(i) => i,
    }
}
