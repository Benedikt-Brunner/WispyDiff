//! Drawing the list's rows: file headers, collapsed files, unified and side-by-side rows, gap rows,
//! and the comment items inserted between them.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, font, pattern_slash, px, AnyElement, App, ClickEvent, Context, FontWeight, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, SharedString, StyledText, TextLayout, WeakEntity, Window,
};
use wispy_core::highlight::Seg;
use wispy_core::model::{row_kind, FileSummary, Row};
use wispy_core::split::{SplitRow, FILLER};

use super::layout::{Item, Mode};
use super::selection::{Point as SelPoint, Side as SelSide, TextSelection};
use super::{seg_text, u_px, CommentMark, DiffViewer, GapExpand, GutterEvent, UnifiedRow};
use crate::logic::gaps::{hidden_lines, GAP_STEP};
use crate::text::{code_line, highlight_bytes, original_byte};
use crate::theme::Palette;
use crate::ui::{palette, u};
use crate::wrap::wrap_columns;

const ROW: f32 = 20.;
/// The band above each file's header but the first (CSS px).
pub const FILE_GAP: f32 = 12.;
/// The header bar's height (CSS px).
pub const HEADER: f32 = ROW + 6.;

fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> gpui_kit::AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
}

/// Width of one line-number column (the widest number, at least three digits).
pub fn gutter_width(viewer: &mut DiffViewer, cx: &App) -> Pixels {
    let digits = viewer.view.summary.max_line_number.max(1).to_string().len().max(3) as f32;
    let cw = viewer.char_width.unwrap_or(u_px(7.2, cx));
    cw * digits + u_px(12., cx)
}

pub fn status_letter(status: wispy_core::diff::FileStatus) -> &'static str {
    use wispy_core::diff::FileStatus::*;
    match status {
        Added => "A",
        Deleted => "D",
        Renamed => "R",
        Copied => "C",
        _ => "M",
    }
}

/// A speech bubble (with a check mark for settled threads).
pub fn comment_icon(settled: bool, color: Hsla, cut: Hsla) -> impl IntoElement {
    div()
        .relative()
        .size(u(12.))
        .flex_none()
        .child(crate::ui::icon("comment", 12., color))
        .when(settled, |d| d.child(crate::ui::icon("check", 12., cut).absolute().top_0().left_0()))
}

/// The colour of a file's status letter (added, deleted, renamed or copied).
pub fn status_color(status: wispy_core::diff::FileStatus, p: &Palette) -> Hsla {
    use wispy_core::diff::FileStatus::*;
    match status {
        Added => p.add_fg,
        Deleted => p.del_fg,
        Renamed | Copied => p.accent,
        _ => p.fg_muted,
    }
}

impl DiffViewer {
    pub fn render_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.built.items.get(index).copied() else { return div().into_any_element() };
        match item {
            Item::Insert(i) => {
                let key = self.built.inserts[i as usize].key.clone();
                self.render_insert(&key, window, cx)
            }
            Item::Row(row) => {
                let Some(segment) = self.built.segment_at(row).cloned() else { return div().into_any_element() };
                let offset = row - segment.start;
                let file = segment.file;
                if offset == 0 {
                    return self.render_file_header(file, segment.mode, row, cx);
                }
                match segment.mode {
                    Mode::Collapsed => div().into_any_element(),
                    Mode::Split => {
                        self.prefetch_split(file, cx);
                        match self.split_rows(file, cx) {
                            Some(rows) => match rows.get(offset as usize - 1) {
                                Some(split) => self.render_split_row(file, offset, row, split, window, cx),
                                None => div().h(u(ROW)).into_any_element(),
                            },
                            None => {
                                crate::perf_marks::note_placeholder();
                                div().h(u(ROW)).w_full().bg(palette(cx).bg).into_any_element()
                            }
                        }
                    }
                    Mode::Unified => self.render_unified_row(file, offset, row, window, cx),
                    Mode::Hidden => div().into_any_element(),
                }
            }
        }
    }

    // ---------- file header ----------

    /// A file's header in the list: a band set off from the file above (where one file ends and
    /// the next begins), then the header bar.
    fn render_file_header(&mut self, file: usize, mode: Mode, row: u32, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let bar = self.file_header_bar(file, mode, row, false, cx);
        if row == 0 {
            return bar.into_any_element();
        }
        div().w_full().flex().flex_col().child(div().h(u(FILE_GAP)).w_full().bg(p.file_gap())).child(bar).into_any_element()
    }

    /// The header bar: in the list, or (`sticky`) pinned at the top for the file being read. It
    /// marks the current file (what v, s, e, … act on); a click makes it current, and a collapsed
    /// file's header (it's all there is of it: why, its threads, how to open it) opens it.
    pub fn file_header_bar(&mut self, file: usize, mode: Mode, row: u32, sticky: bool, cx: &mut Context<Self>) -> gpui_kit::Stateful<gpui_kit::Div> {
        let p = palette(cx).clone();
        let summary = self.view.summary.files[file].clone();
        let hidden = self.marks.get(&(row, false)).filter(|m| m.header).cloned();
        let collapsed = mode == Mode::Collapsed;
        let current = self.frame_current == Some(file);
        let group: &'static str = if sticky { "sticky-header" } else { "file-header" };
        let (why, tally) = if collapsed {
            let s = self.session.read(cx);
            let why = summary.noise.clone().unwrap_or_else(|| {
                if s.is_viewed(&summary.content_key) {
                    "viewed".into()
                } else if s.is_ignored(&summary.path) {
                    "ignored".into()
                } else {
                    "collapsed".into()
                }
            });
            (Some(why), self.tallies.get(&file).cloned().unwrap_or_default())
        } else {
            (None, Default::default())
        };
        let count = |k: &str| tally.get(k).copied().unwrap_or(0);
        let mut settled: Vec<(String, usize)> = tally.iter().filter(|(k, _)| *k != "open" && *k != "draft").map(|(k, v)| (k.clone(), *v)).collect();
        settled.sort();
        let (open, drafts) = (count("open"), count("draft"));
        let key = if why.as_deref() == Some("viewed") { "v" } else { "e" };
        let path = summary.path.clone();
        div()
            .id(if sticky { ("sticky-header", file) } else { ("file-header", file) })
            .group(group)
            .relative()
            .on_click(cx.listener(move |this, _, _, cx| {
                if collapsed {
                    let path = path.clone();
                    this.change_layout(cx, |this, _| {
                        this.expanded.insert(path.clone());
                        this.collapsed.remove(&path);
                    });
                }
                this.pin_file(file, cx);
            }))
            .h(u(HEADER))
            .w_full()
            .flex()
            .items_center()
            .gap(u(10.))
            .pl(u(12.))
            .pr(u(10.))
            .bg(p.file_bg)
            .border_t_1()
            .border_b_1()
            .border_color(p.border)
            .font_family(crate::ui::sans_family(cx))
            .text_size(u(12.))
            .cursor_pointer()
            // The current file: an accent edge.
            .when(current, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(u(3.)).bg(p.accent)))
            .child(div().w(u(12.)).flex_none().font_weight(FontWeight::SEMIBOLD).text_color(status_color(summary.status, &p)).child(status_letter(summary.status)))
            .child(
                div()
                    .flex()
                    .flex_shrink(1.)
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(collapsed, |d| d.text_color(p.fg_muted))
                    .when_some(summary.old_path.clone(), |d, old| d.child(div().text_color(p.fg_muted).child(format!("{old} → "))))
                    .child(div().min_w(px(0.)).overflow_hidden().text_ellipsis().child(summary.path.clone())),
            )
            .child(div().flex_none().text_color(p.add_fg).child(format!("+{}", summary.additions)))
            .child(div().flex_none().text_color(p.del_fg).child(format!("−{}", summary.deletions)))
            .when_some(summary.noise.clone().filter(|_| !collapsed), |d, noise| d.child(crate::ui::status_badge(noise, crate::ui::BadgeKind::Plain, cx).flex_none()))
            .when_some(why, |d, why| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(4.))
                        .text_color(p.fg_muted)
                        .child(crate::ui::status_badge(why, crate::ui::BadgeKind::Plain, cx))
                        .when(open > 0, |d| d.child(comment_icon(false, p.accent, p.file_bg)).child(div().text_color(p.accent).child(format!("{open} open"))))
                        .children(settled.into_iter().map(|(reason, n)| {
                            div().flex().items_center().gap(u(4.)).child(comment_icon(true, p.fg_muted, p.file_bg)).child(format!("{n} {}", reason.to_lowercase()))
                        }))
                        .when(drafts > 0, |d| d.child(format!("{drafts} draft{}", if drafts == 1 { "" } else { "s" }))),
                )
            })
            .child(div().flex_1())
            .when(mode == Mode::Split, |d| d.child(div().flex_none().text_size(u(11.)).text_color(p.fg_muted).child("side by side")))
            .when(collapsed, |d| {
                d.child(div().flex_none().text_size(u(11.)).text_color(p.fg_muted).group_hover(group, |s| s.text_color(p.fg)).child(format!("click or {key} to open")))
            })
            .when(!collapsed, |d| {
                d.child(
                    div()
                        .id(if sticky { ("sticky-comment", file) } else { ("file-comment", file) })
                        .flex_none()
                        .px(u(6.))
                        .rounded(u(4.))
                        .border_1()
                        .border_color(gpui_kit::transparent_black())
                        .text_size(u(11.))
                        // Shown while the header is hovered.
                        .text_color(gpui_kit::transparent_black())
                        .group_hover(group, |d| d.text_color(p.fg_muted).border_color(p.border))
                        .cursor_pointer()
                        .hover(|d| d.text_color(p.accent))
                        .tooltip(tooltip("Comment on this file"))
                        .child("Comment")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_file_composer(file, window, cx)
                        })),
                )
            })
            .when_some(hidden, |d, mark| d.child(self.mark_button((if sticky { "sticky-hidden" } else { "file-hidden" }, file), &mark, cx).child(format!("{}", mark.threads.len()))))
    }

    fn mark_button(&self, id: impl Into<gpui_kit::ElementId>, mark: &CommentMark, cx: &mut Context<Self>) -> gpui_kit::Stateful<gpui_kit::Div> {
        let p = palette(cx);
        let ids: Vec<String> = mark.threads.iter().map(|t| t.id.clone()).collect();
        let title = format!("{}\n\nClick to show", mark.threads.iter().map(|t| t.preview.clone()).collect::<Vec<_>>().join("\n"));
        div()
            .id(id.into())
            .flex()
            .items_center()
            .gap(u(2.))
            .cursor_pointer()
            .text_size(u(11.))
            .text_color(if mark.settled { p.fg_muted } else { p.accent })
            .tooltip(tooltip(title))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| this.show_threads(&ids, cx)))
            .child(comment_icon(mark.settled, if mark.settled { p.fg_muted } else { p.accent }, p.bg))
    }

    // ---------- rows ----------

    /// Rows selected in the gutter (or flashed after a jump).
    /// Rows a comment is about: dragged over in the gutter, or what the open composer is on
    /// (`new_side`: which half, side by side). Their gutters keep the "+".
    fn row_referenced(&self, file: usize, mode: Mode, offset: u32, new_side: Option<bool>) -> bool {
        let hit = |r: &super::RowRange| r.file == file && r.mode == mode && offset >= r.from && offset <= r.to;
        self.selection.as_ref().is_some_and(hit)
            || (self.composer.is_some()
                && self.composer_rows.as_ref().is_some_and(|(r, new)| hit(r) && new_side.is_none_or(|side| side == *new)))
    }

    fn row_selected(&self, file: usize, mode: Mode, offset: u32) -> bool {
        let hit = |r: &super::RowRange| r.file == file && r.mode == mode && offset >= r.from && offset <= r.to;
        self.selection.as_ref().is_some_and(hit)
            || self.flash.as_ref().is_some_and(hit)
            || (self.composer.is_some() && self.composer_rows.as_ref().is_some_and(|(r, _)| hit(r)))
    }

    fn render_unified_row(&mut self, file: usize, offset: u32, row_index: u32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let gutter = gutter_width(self, cx);
        let selected = self.row_selected(file, Mode::Unified, offset);
        let show_attribution = self.prs.len() > 1;
        let summary = self.view.summary.files[file].clone();
        let (row, line_segs): (Option<Row>, Option<(u32, u32, Rc<Vec<Seg>>)>) = match self.unified_at(file, offset) {
            UnifiedRow::Row(row) => (Some(row.clone()), None),
            UnifiedRow::Line { o, n, segs } => (None, Some((o, n, Rc::new(segs.as_ref().clone())))),
            UnifiedRow::Gap { gap, header } => {
                let text = header.and_then(|h| h.s.first().map(|s| s.1.clone())).unwrap_or_default();
                return self.render_gap_row(file, gap, header.is_some(), text, cx);
            }
            UnifiedRow::None => return div().h(u(ROW)).into_any_element(),
        };
        if let Some((o, n, segs)) = line_segs {
            // A shown unchanged line of a gap: like a context row.
            let fake = Row { k: row_kind::CONTEXT, f: file as u32, o: Some(o), n: Some(n), s: segs.as_ref().clone(), a: None, h: Vec::new(), l: None, w: Vec::new() };
            return self.unified_line(file, offset, row_index, &fake, &summary, gutter, selected, false, &p, window, cx);
        }
        let row = row.unwrap();
        match row.k {
            row_kind::HUNK | row_kind::NOTICE => {
                let text = row.s.first().map(|s| s.1.clone()).unwrap_or_default();
                let (bg, fg) = if row.k == row_kind::HUNK { (p.hunk_bg, p.hunk_fg) } else { (p.bg, p.fg_muted) };
                div()
                    .h(u(ROW))
                    .w_full()
                    .flex()
                    .bg(bg)
                    .text_color(fg)
                    .when_some(p.hunk_top_border.filter(|_| row.k == row_kind::HUNK), |d, c| d.border_t_1().border_color(c))
                    .when(row.k == row_kind::NOTICE, |d| d.italic().font_family(crate::ui::sans_family(cx)))
                    .child(div().flex_none().w(gutter * 2. + u_px(16., cx)))
                    .child(div().pl(u(8.)).whitespace_nowrap().overflow_hidden().child(text))
                    .into_any_element()
            }
            _ => {
                let previous = offset.checked_sub(1).and_then(|o| match self.unified_at(file, o) {
                    UnifiedRow::Row(r) => Some(r.clone()),
                    _ => None,
                });
                let tag = self.hi > self.lo && starts_run(&row, previous.as_ref());
                let _ = show_attribution;
                self.unified_line(file, offset, row_index, &row, &summary, gutter, selected, tag, &p, window, cx)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn unified_line(
        &mut self,
        file: usize,
        offset: u32,
        row_index: u32,
        row: &Row,
        summary: &FileSummary,
        gutter: Pixels,
        selected: bool,
        tag: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (bg, gutter_bg, gutter_fg, sign, word_bg) = match row.k {
            row_kind::ADDED => (p.add_bg, p.add_gutter, p.add_fg, "+", p.add_word),
            row_kind::DELETED => (p.del_bg, p.del_gutter, p.del_gutter_fg, "−", p.del_word),
            _ => (p.bg, p.bg, p.fg_muted, "", p.add_word),
        };
        let attributed = self.prs.len() > 1 && row.a.is_some();
        let marker = self.assistant_marker(&summary.path, row.n, cx);
        let mark = self.marks.get(&(row_index, false)).cloned();
        let history = attributed.then(|| line_history(row, &self.prs));
        let number = |n: Option<u32>| div().w(gutter).flex_none().pr(u(6.)).flex().justify_end().children(n.map(|n| n.to_string()));
        let weak = cx.entity().downgrade();
        let gutter_bg = if selected { blend(gutter_bg, p.accent, 0.12) } else { gutter_bg };
        let gutter_el = div()
            .id(("gutter", row_index as usize))
            .flex_none()
            .flex()
            .h_full()
            .bg(gutter_bg)
            .text_color(gutter_fg)
            .cursor_pointer()
            .child(div().w(u(3.)).flex_none().when_some(row.a.filter(|_| attributed), |d, a| d.bg(p.pr(a))))
            .child(number(row.o))
            .child(number(row.n))
            // Hovering the row shows "+" (comment here) in place of the sign.
            .child(if self.row_referenced(file, Mode::Unified, offset, None) {
                div().w(u(13.)).h(u(ROW)).flex_none().child(add_sign_shown(p))
            } else {
                div()
                    .relative()
                    .w(u(13.))
                    .h(u(ROW))
                    .flex_none()
                    .child(div().group_hover("diff-row", |s| s.invisible()).child(sign))
                    .child(add_sign_in(p, "diff-row").absolute().top_0().left_0().size_full())
            })
            .children(marker.map(|id| self.assistant_mark(id, cx)))
            .children(mark.map(|m| self.mark_button(("mark", row_index as usize), &m, cx)))
            .on_mouse_down(MouseButton::Left, gutter_handler(weak.clone(), GutterEvent::Down, file, Mode::Unified, offset, true))
            .on_mouse_up(MouseButton::Left, gutter_up(weak.clone(), file, Mode::Unified, offset, true))
            .on_mouse_move(gutter_move(weak.clone(), file, Mode::Unified, offset, true));
        let code = self.code_cell(file, Mode::Unified, SelSide::Unified, offset, &row.s, &row.w, word_bg, self.wrap_columns.0, window, cx);
        let tag_el = tag.then(|| {
            let a = row.a.unwrap_or(0);
            div()
                .absolute()
                .right(u(8.))
                .top(u(2.))
                .h(u(16.))
                .px(u(6.))
                .flex()
                .items_center()
                .rounded(u(4.))
                .text_size(u(10.))
                .text_color(p.pr(a))
                .bg(blend(p.bg, p.pr(a), 0.16))
                .child(format!("#{}", self.prs[a as usize].number))
        });
        div()
            .id(("row", row_index as usize))
            .group("diff-row")
            .relative()
            .min_h(u(ROW))
            .w_full()
            .flex()
            .bg(bg)
            .when(selected, |d| d.bg(blend(bg, p.accent, 0.12)))
            .when_some(history, |d, h| d.tooltip(tooltip(h)))
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.hovered.as_ref().is_none_or(|t| t.file != file || t.offset != offset || t.mode != Mode::Unified) {
                    this.hovered = this.target_at(file, Mode::Unified, offset, true, cx);
                }
            }))
            .child(gutter_el)
            .child(code)
            .children(tag_el)
            .into_any_element()
    }

    fn render_split_row(&mut self, file: usize, offset: u32, row_index: u32, row: &SplitRow, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let gutter = gutter_width(self, cx);
        let selected = self.row_selected(file, Mode::Split, offset);
        let path = self.view.summary.files[file].path.clone();
        let marker = self.assistant_marker(&path, row.n, cx);
        let left = self.split_half(file, offset, row_index, false, row.o, row.ok, &row.os, &row.ow, row.oa, gutter, None, selected, &p, window, cx);
        let right = self.split_half(file, offset, row_index, true, row.n, row.nk, &row.ns, &row.nw, row.na, gutter, marker, selected, &p, window, cx);
        div()
            .id(("split", row_index as usize))
            .min_h(u(ROW))
            .w_full()
            .flex()
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.hovered.as_ref().is_none_or(|t| t.file != file || t.offset != offset || t.mode != Mode::Split) {
                    this.hovered = this.target_at(file, Mode::Split, offset, true, cx);
                }
            }))
            .child(left)
            .child(right)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn split_half(
        &mut self,
        file: usize,
        offset: u32,
        row_index: u32,
        new_side: bool,
        number: Option<u32>,
        kind: u8,
        segs: &[Seg],
        words: &[(u32, u32)],
        pr: Option<u8>,
        gutter: Pixels,
        marker: Option<String>,
        selected: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (bg, gutter_bg, gutter_fg, word_bg) = match kind {
            row_kind::ADDED => (Some(p.add_bg), Some(p.add_gutter), p.add_fg, p.add_word),
            row_kind::DELETED => (Some(p.del_bg), Some(p.del_gutter), p.del_gutter_fg, p.del_word),
            FILLER => (None, None, p.fg_muted, p.add_word),
            _ => (Some(p.bg), Some(p.bg), p.fg_muted, p.add_word),
        };
        // A selected row is tinted across code and gutters alike.
        let tint = |c: Option<gpui_kit::Hsla>| if selected { Some(blend(c.unwrap_or(p.bg), p.accent, 0.12)) } else { c };
        let (bg, gutter_bg) = (tint(bg), tint(gutter_bg));
        let filler = kind == FILLER;
        let attributed = self.prs.len() > 1 && pr.is_some();
        let mark = self.marks.get(&(row_index, new_side)).cloned();
        let weak = cx.entity().downgrade();
        let side = if new_side { SelSide::New } else { SelSide::Old };
        let mut gutter_el = div()
            .id(("half-gutter", row_index as usize * 2 + new_side as usize))
            .flex_none()
            .flex()
            .h_full()
            .when_some(gutter_bg, |d, c| d.bg(c))
            .text_color(gutter_fg)
            .child(div().w(u(3.)).flex_none().when_some(pr.filter(|_| attributed), |d, a| d.bg(p.pr(a))))
            .child(
                div()
                    .relative()
                    .w(gutter + u_px(14., cx))
                    .flex_none()
                    .pr(u(6.))
                    .flex()
                    .justify_end()
                    .children(number.map(|n| n.to_string()))
                    // Hovering the half shows "+" (comment here) after the number.
                    .when(!filler, |d| {
                        let shown = self.row_referenced(file, Mode::Split, offset, Some(new_side));
                        d.child(if shown { add_sign_shown(p) } else { add_sign_in(p, "diff-half") }.pl(u(2.)))
                    }),
            )
            .children(marker.map(|id| self.assistant_mark(id, cx)))
            .children(mark.map(|m| self.mark_button(("half-mark", row_index as usize * 2 + new_side as usize), &m, cx)));
        if !filler {
            gutter_el = gutter_el
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, gutter_handler(weak.clone(), GutterEvent::Down, file, Mode::Split, offset, new_side))
                .on_mouse_up(MouseButton::Left, gutter_up(weak.clone(), file, Mode::Split, offset, new_side))
                .on_mouse_move(gutter_move(weak.clone(), file, Mode::Split, offset, new_side));
        }
        div()
            .group("diff-half")
            .flex_1()
            .min_w(px(0.))
            .flex()
            .overflow_hidden()
            .when_some(bg, |d, c| d.bg(c))
            .when(filler, |d| d.bg(pattern_slash(p.border.opacity(0.6), 1., 6.)))
            .when(!new_side, |d| d.border_r_1().border_color(p.border))
            .child(gutter_el)
            .when(!filler, |d| d.child(self.code_cell(file, Mode::Split, side, offset, segs, words, word_bg, self.wrap_columns.1, window, cx)))
            .into_any_element()
    }

    // ---------- gap rows ----------

    fn render_gap_row(&mut self, file: usize, gap_index: usize, up: bool, text: String, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let summary = &self.view.summary.files[file];
        let gap = summary.gaps[gap_index].clone();
        let hidden = hidden_lines(&gap, self.reveals.get(&file).and_then(|r| r.get(gap_index)));
        let down = gap.new_start > 1;
        let few = hidden <= GAP_STEP;
        let lines = |n: u32| format!("{n} unchanged line{}", if n == 1 { "" } else { "s" });
        let title = if few {
            format!("{} — click to show them", lines(hidden))
        } else if up && down {
            format!("{} — click to show {} more above and below, double-click to show all", lines(hidden), GAP_STEP / 2)
        } else {
            format!("{} — click to show {GAP_STEP} more, double-click to show all", lines(hidden))
        };
        let gutter = gutter_width(self, cx);
        let arrow = |id: &'static str, label: &'static str, how: GapExpand, title: String, cx: &mut Context<Self>| {
            div()
                .id((id, gap_index))
                .px(u(6.))
                .rounded(u(3.))
                .cursor_pointer()
                .hover(|d| d.bg(p.accent.opacity(0.2)))
                .tooltip(tooltip(title))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    let how = if event.click_count() >= 2 { GapExpand::All } else { how };
                    this.expand_gap(file, gap_index, how, cx);
                }))
                .child(label)
        };
        let arrows = if few {
            vec![arrow("gap-all", "↕", GapExpand::All, format!("Show {}", lines(hidden)), cx)]
        } else {
            let mut v = Vec::new();
            if down {
                v.push(arrow("gap-down", "↓", GapExpand::Down, format!("Show {GAP_STEP} lines below the change above"), cx));
            }
            if up {
                v.push(arrow("gap-up", "↑", GapExpand::Up, format!("Show {GAP_STEP} lines above the change below"), cx));
            }
            v
        };
        div()
            .id(("gap", file * 10_000 + gap_index))
            .h(u(ROW))
            .w_full()
            .flex()
            .bg(p.hunk_bg)
            .text_color(p.hunk_fg)
            .cursor_pointer()
            .hover(|d| d.bg(blend(p.hunk_bg, p.accent, 0.12)))
            .tooltip(tooltip(title))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                let how = if event.click_count() >= 2 || few { GapExpand::All } else { GapExpand::Row };
                this.expand_gap(file, gap_index, how, cx);
            }))
            .child(div().flex_none().w(gutter * 2. + u_px(16., cx)).flex().justify_center().children(arrows))
            .child(div().pl(u(8.)).whitespace_nowrap().overflow_hidden().child(if text.is_empty() { lines(hidden) } else { text }))
            .into_any_element()
    }

    // ---------- code ----------

    /// A row's code: highlighted, changed words marked, the selection painted, wrapped or shifted
    /// sideways; ⌘-click looks up usages, dragging selects.
    #[allow(clippy::too_many_arguments)]
    fn code_cell(
        &mut self,
        file: usize,
        mode: Mode,
        side: SelSide,
        offset: u32,
        segs: &[Seg],
        words: &[(u32, u32)],
        word_bg: Hsla,
        columns: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx).clone();
        let mono = font(crate::ui::mono_family(cx));
        let (text, mut runs) = code_line(segs, words, &mono, &p, word_bg);
        if let Some((a, b)) = self.text_selection.as_ref().and_then(|s| s.range_in(file, mode, side, offset, text.len())) {
            highlight_bytes(&mut runs, a, b, p.selection);
        }
        let weak = cx.entity().downgrade();
        let ctx = Rc::new(CodeContext { weak, file, mode, side, offset, text: text.clone() });
        let wrap = self.session.read(cx).wrap && columns > 0;
        let lines: Vec<(SharedString, Vec<gpui_kit::TextRun>, usize)> = if wrap {
            let mut base = 0;
            wrap_columns(&text, &runs, columns)
                .into_iter()
                .map(|(t, r)| {
                    let start = base;
                    base += t.len();
                    (t, r, start)
                })
                .collect()
        } else {
            vec![(text.clone(), runs, 0)]
        };
        let shift = if wrap || mode == Mode::Split && false { px(0.) } else { self.scroll_x };
        let lines = lines.into_iter().map(|(t, r, base)| {
            let styled = StyledText::new(t).with_runs(r);
            let layout = styled.layout().clone();
            let ctx = ctx.clone();
            let (down_ctx, move_ctx, up_ctx) = (ctx.clone(), ctx.clone(), ctx);
            let (down_layout, move_layout, up_layout) = (layout.clone(), layout.clone(), layout);
            div()
                .h(u(ROW))
                .whitespace_nowrap()
                .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| code_down(&down_ctx, &down_layout, base, event, window, cx))
                .on_mouse_move(move |event: &MouseMoveEvent, window, cx| code_move(&move_ctx, &move_layout, base, event, window, cx))
                .on_mouse_up(MouseButton::Left, move |event: &MouseUpEvent, window, cx| code_up(&up_ctx, &up_layout, base, event, window, cx))
                .child(styled)
        });
        div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .child(div().relative().left(-shift).pl(u(8.)).pr(u(8.)).children(lines))
            .into_any_element()
    }

    fn assistant_marker(&self, path: &str, line: Option<u32>, cx: &App) -> Option<String> {
        let line = line?;
        super::assistant::marker_at(self, path, line, cx)
    }

    fn assistant_mark(&self, id: String, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        div()
            .id(SharedString::from(format!("assistant-mark-{id}")))
            .absolute()
            .left(u(4.))
            .top(u(6.))
            .size(u(8.))
            .rounded_full()
            .bg(p.accent)
            .cursor_pointer()
            .tooltip(tooltip("Assistant conversation about these lines"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_assistant(Some(id.clone()), window, cx);
            }))
            .into_any_element()
    }

    /// Finishes a text selection: what's selected, with the files' own tabs.
    pub fn finish_selection(&mut self, cx: &mut Context<Self>) {
        let Some(sel) = self.text_selection.clone() else { return };
        if sel.is_empty() {
            self.text_selection = None;
            cx.notify();
            return;
        }
        let (a, b) = sel.ordered();
        let mut lines = Vec::new();
        for offset in a.offset..=b.offset {
            let original = match sel.mode {
                Mode::Unified => match self.unified_at(sel.file, offset) {
                    UnifiedRow::Row(r) if !matches!(r.k, row_kind::HUNK | row_kind::NOTICE | row_kind::FILE) => seg_text(&r.s),
                    UnifiedRow::Line { segs, .. } => seg_text(&segs),
                    _ => continue,
                },
                _ => {
                    let Some(rows) = self.split_rows(sel.file, cx) else { continue };
                    let Some(r) = rows.get((offset as usize).wrapping_sub(1)) else { continue };
                    let (kind, segs) = if sel.side == SelSide::Old { (r.ok, &r.os) } else { (r.nk, &r.ns) };
                    if kind == FILLER {
                        continue;
                    }
                    seg_text(segs)
                }
            };
            let start = if offset == a.offset { original_byte(&original, a.byte) } else { 0 };
            let end = if offset == b.offset { original_byte(&original, b.byte) } else { original.len() };
            lines.push(original[start.min(end)..end].to_string());
        }
        if let Some(s) = &mut self.text_selection {
            s.text = lines.join("\n");
            s.active = false;
        }
        cx.notify();
    }
}

struct CodeContext {
    weak: WeakEntity<DiffViewer>,
    file: usize,
    mode: Mode,
    side: SelSide,
    offset: u32,
    text: SharedString,
}

fn byte_at(layout: &TextLayout, base: usize, position: gpui_kit::Point<Pixels>) -> usize {
    base + match layout.index_for_position(position) {
        Ok(i) | Err(i) => i,
    }
}

/// The identifier around byte `at` (for ⌘-click and `u`).
fn word_at(text: &str, at: usize) -> Option<String> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    let at = at.min(text.len());
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| at + i);
    let word = text[start..end].trim_start_matches('$');
    word.chars().next().filter(|c| c.is_ascii_alphabetic() || *c == '_').map(|_| word.to_string())
}

fn code_down(ctx: &CodeContext, layout: &TextLayout, base: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut App) {
    let at = byte_at(layout, base, event.position);
    if event.modifiers.secondary() {
        if let Some(word) = word_at(&ctx.text, at) {
            let _ = ctx.weak.update(cx, |this, cx| this.open_usages(&word, window, cx));
        }
        return;
    }
    let point = SelPoint { offset: ctx.offset, byte: at };
    let (file, mode, side) = (ctx.file, ctx.mode, ctx.side);
    // A double click selects the word under it.
    let word = if event.click_count == 2 { word_bounds(&ctx.text, at) } else { None };
    let extend = event.modifiers.shift;
    let _ = ctx.weak.update(cx, |this, cx| {
        this.selection = None;
        if let Some((a, b)) = word {
            let mut sel = TextSelection::start(file, mode, side, SelPoint { offset: point.offset, byte: a });
            sel.head = SelPoint { offset: point.offset, byte: b };
            this.text_selection = Some(sel);
            this.finish_selection(cx);
            return;
        }
        // Shift-click extends the selection in the same column.
        if extend {
            if let Some(sel) = &mut this.text_selection {
                if sel.file == file && sel.mode == mode && sel.side == side {
                    sel.head = point;
                    sel.active = true;
                    cx.notify();
                    return;
                }
            }
        }
        this.text_selection = Some(TextSelection::start(file, mode, side, point));
        cx.notify();
    });
}

/// The bytes of the word (letters, digits, `_`) around byte `at` of `text`.
fn word_bounds(text: &str, at: usize) -> Option<(usize, usize)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let at = at.min(text.len());
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| at + i);
    (start < end).then_some((start, end))
}

fn code_move(ctx: &CodeContext, layout: &TextLayout, base: usize, event: &MouseMoveEvent, _: &mut Window, cx: &mut App) {
    let at = byte_at(layout, base, event.position);
    let word = word_at(&ctx.text, at);
    let dragging = event.pressed_button == Some(MouseButton::Left);
    let (file, mode, side, offset) = (ctx.file, ctx.mode, ctx.side, ctx.offset);
    let _ = ctx.weak.update(cx, |this, cx| {
        this.hover_word = word;
        if dragging {
            if let Some(sel) = &mut this.text_selection {
                if sel.active && sel.file == file && sel.mode == mode && sel.side == side {
                    let head = SelPoint { offset, byte: at };
                    if sel.head != head {
                        sel.head = head;
                        cx.notify();
                    }
                }
            }
        }
    });
}

fn code_up(ctx: &CodeContext, _: &TextLayout, _: usize, _: &MouseUpEvent, _: &mut Window, cx: &mut App) {
    let _ = ctx.weak.update(cx, |this, cx| this.finish_selection(cx));
}

fn gutter_handler(
    weak: WeakEntity<DiffViewer>,
    event: GutterEvent,
    file: usize,
    mode: Mode,
    offset: u32,
    new_side: bool,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        cx.stop_propagation();
        let _ = weak.update(cx, |this, cx| this.on_gutter(event, file, mode, offset, new_side, window, cx));
    }
}

fn gutter_up(weak: WeakEntity<DiffViewer>, file: usize, mode: Mode, offset: u32, new_side: bool) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        let _ = weak.update(cx, |this, cx| this.on_gutter(GutterEvent::Up, file, mode, offset, new_side, window, cx));
    }
}

fn gutter_move(weak: WeakEntity<DiffViewer>, file: usize, mode: Mode, offset: u32, new_side: bool) -> impl Fn(&MouseMoveEvent, &mut Window, &mut App) + 'static {
    move |event, window, cx| {
        let pressed = event.pressed_button == Some(MouseButton::Left);
        let _ = weak.update(cx, |this, cx| {
            let event = if pressed { GutterEvent::Enter } else { GutterEvent::Hover };
            this.on_gutter(event, file, mode, offset, new_side, window, cx);
        });
    }
}

/// A run of changed lines from one PR starts here (the previous row is another PR or not a change).
fn starts_run(row: &Row, previous: Option<&Row>) -> bool {
    let Some(a) = row.a else { return false };
    let changed = |r: &Row| r.k == row_kind::ADDED || r.k == row_kind::DELETED;
    !matches!(previous, Some(p) if changed(p) && p.a == Some(a))
}

fn line_history(row: &Row, prs: &[wispy_core::github::PullRequest]) -> String {
    let n = |i: u8| format!("#{}", prs.get(i as usize).map_or(0, |p| p.number));
    let a = row.a.unwrap_or(0);
    if row.k == row_kind::DELETED {
        return format!("Deleted in {}", n(a));
    }
    if row.h.len() > 1 {
        return format!("Added in {} · modified in {}", n(row.h[0]), row.h[1..].iter().map(|i| n(*i)).collect::<Vec<_>>().join(", "));
    }
    format!("Added in {}", n(a))
}

/// `over` mixed into `base` at `amount`.
/// The "+" on a row a comment is about.
fn add_sign_shown(p: &Palette) -> gpui_kit::Div {
    div().font_weight(gpui_kit::FontWeight::BOLD).text_color(p.accent).child("+")
}

/// A "+" (comment here) shown only while `group` (the row, or a side-by-side half) is hovered.
fn add_sign_in(p: &Palette, group: &'static str) -> gpui_kit::Div {
    let accent = p.accent;
    // Visibility, not display: a display change between prepaint and paint trips GPUI.
    div()
        .invisible()
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(accent)
        .group_hover(group, |s| s.visible())
        .child("+")
}

pub fn blend(base: Hsla, over: Hsla, amount: f32) -> Hsla {
    let (b, o) = (base.to_rgb(), over.to_rgb());
    let mix = |x: f32, y: f32| x + (y - x) * amount;
    gpui_kit::Rgba { r: mix(b.r, o.r), g: mix(b.g, o.g), b: mix(b.b, o.b), a: b.a }.into()
}

#[cfg(test)]
mod tests {
    use super::word_at;

    #[test]
    fn finds_identifiers() {
        assert_eq!(word_at("  $result = max($a);", 5).as_deref(), Some("result"));
        assert_eq!(word_at("foo(bar)", 5).as_deref(), Some("bar"));
        assert_eq!(word_at("x = 42;", 5), None);
        assert_eq!(word_at("a.b", 3).as_deref(), Some("b"));
    }
}
