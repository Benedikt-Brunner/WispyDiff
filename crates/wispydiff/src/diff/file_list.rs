//! The file list: a flat list or a directory tree (`t`), the file filter menu (`f`), and the
//! sidebar's resizable edge.

use std::collections::BTreeMap;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, AnyElement, App, Context, DragMoveEvent, FontWeight, KeyBinding, Render, SharedString,
    Window,
};

use super::layout::Mode;
use super::rows::{comment_icon, status_color, status_letter};
use super::{extension_suffixes, DiffViewer, FileFilter};
use crate::logic::file_tree::{ancestors, build_tree, tree_rows, TreeNode};
use crate::settings;
use crate::ui::{palette, u};

gpui_kit::actions!(file_filter, [FilterDown, FilterUp, FilterToggle, FilterClose]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("FileFilter");
    cx.bind_keys([
        KeyBinding::new("down", FilterDown, ctx),
        KeyBinding::new("up", FilterUp, ctx),
        KeyBinding::new("space", FilterToggle, ctx),
        KeyBinding::new("enter", FilterToggle, ctx),
        KeyBinding::new("escape", FilterClose, ctx),
    ]);
}

const MIN_WIDTH: f32 = 180.;
/// A sidebar never takes more than this share of the window.
const MAX_SHARE: f32 = 0.6;

/// A line of the file list.
pub enum FileRow {
    File { index: usize, depth: Option<usize> },
    Dir { path: String, name: String, depth: usize, files: Vec<usize> },
}

/// Dragging a sidebar's edge (the payload names the sidebar).
#[derive(Clone)]
pub struct SidebarDrag(pub &'static str);

impl Render for SidebarDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A sidebar's draggable edge (double-click resets the width); `left_edge` for a right sidebar.
pub fn resize_handle(key: &'static str, left_edge: bool, cx: &mut Context<DiffViewer>) -> AnyElement {
    let p = palette(cx).clone();
    div()
        .id(SharedString::from(format!("resize-{key}")))
        .absolute()
        .top_0()
        .bottom_0()
        .when(left_edge, |d| d.left(px(-3.)))
        .when(!left_edge, |d| d.right(px(-3.)))
        .w(px(6.))
        .cursor_col_resize()
        .hover(|d| d.bg(p.accent.opacity(0.3)))
        .on_drag(SidebarDrag(key), |drag, _, _, cx| cx.new(|_| drag.clone()))
        .on_click(cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
            if event.click_count() >= 2 {
                this.set_sidebar_width(key, None, cx);
            }
        }))
        .into_any_element()
}

impl DiffViewer {
    /// Sets (None: resets) a sidebar's width, remembered per sidebar.
    pub fn set_sidebar_width(&mut self, key: &str, width: Option<f32>, cx: &mut Context<Self>) {
        let fallback = match key {
            "files" => 280.,
            "code" => 380.,
            _ => 420.,
        };
        let value = width.unwrap_or(fallback);
        match key {
            "files" => self.file_list_width = value,
            "code" => self.code_panel_width = value,
            _ => self.assistant_width = value,
        }
        if let Some(panel) = self.code_panel() {
            let w = self.code_panel_width;
            panel.update(cx, |p, cx| p.set_width(w, cx));
        }
        if let Some(a) = &self.assistant {
            let w = self.assistant_width;
            a.update(cx, |a, cx| a.set_width(w, cx));
        }
        settings::set(cx, &format!("width.{key}"), width.map(|w| w.round()));
        cx.notify();
    }

    /// While an edge is dragged: the new width (in CSS px at 100%).
    pub fn on_sidebar_drag(&mut self, event: &DragMoveEvent<SidebarDrag>, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.drag(cx).0;
        let zoom = crate::ui::zoom(cx);
        let window_width = f32::from(window.viewport_size().width);
        let x = f32::from(event.event.position.x);
        let width_px = match key {
            "files" => x - f32::from(event.bounds.left()),
            _ => window_width - x,
        };
        let max = window_width * MAX_SHARE / zoom;
        let width = (width_px / zoom).clamp(MIN_WIDTH, max.max(MIN_WIDTH));
        self.set_sidebar_width(key, Some(width), cx);
    }

    pub fn render_file_list(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let files = self.view.summary.files.clone();
        let filtered = self.filtered();
        let filtered_count = filtered.iter().filter(|f| **f).count();
        let current = self.current();
        // The current file's directories open when it is reached (not again while it stays current,
        // so they can be folded by hand).
        if current != self.file_list_current {
            if let Some(path) = current.and_then(|c| files.get(c)).map(|f| f.path.clone()) {
                for dir in ancestors(&path) {
                    self.tree_closed.remove(&dir);
                }
            }
        }
        // The rows (files, and directories in the tree), drawn as they scroll into view.
        let rows: Vec<FileRow> = if self.list_mode_tree {
            let paths: Vec<Option<String>> = files.iter().enumerate().map(|(i, f)| (!filtered[i]).then(|| f.path.clone())).collect();
            let tree: Vec<TreeNode> = build_tree(&paths);
            tree_rows(&tree, &self.tree_closed)
                .into_iter()
                .map(|row| match row.node {
                    TreeNode::File(i) => FileRow::File { index: *i, depth: Some(row.depth) },
                    TreeNode::Dir(dir) => FileRow::Dir { path: dir.path.clone(), name: dir.name.clone(), depth: row.depth, files: dir.files.clone() },
                })
                .collect()
        } else {
            (0..files.len()).filter(|i| !filtered[*i]).map(|index| FileRow::File { index, depth: None }).collect()
        };
        // The current file stays in view in the list.
        if current != self.file_list_current {
            self.file_list_current = current;
            if let Some(at) = current.and_then(|c| rows.iter().position(|r| matches!(r, FileRow::File { index, .. } if *index == c))) {
                self.file_scroll.scroll_to_item(at, gpui_kit::ScrollStrategy::Nearest);
            }
        }
        let count = rows.len();
        self.file_rows = std::rc::Rc::new(rows);
        let list = gpui_kit::uniform_list("file-list", count, cx.processor(|this, range: std::ops::Range<usize>, _, cx| this.render_file_rows(range, cx)))
            .track_scroll(&self.file_scroll)
            .flex_1()
            .min_h(px(0.))
            .py(u(4.));

        let filter = self.filter();
        let active = !filter.exts.is_empty() && !filter.off;
        let tree = self.list_mode_tree;
        let mode_button = |id: &'static str, label: &'static str, on: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(u(6.))
                .py(u(2.))
                .rounded(u(4.))
                .cursor_pointer()
                .text_size(u(12.))
                .when(on, |d| d.bg(p.selection))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if id == "file-list-list" { "Flat list (t)" } else { "Directory tree (t)" }).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !on {
                        this.toggle_list_mode(cx);
                    }
                }))
                .flex()
                .items_center()
                .child(crate::ui::icon(label, 14., if on { p.fg } else { p.fg_muted }))
        };
        div()
            .relative()
            .flex_none()
            .w(u(self.file_list_width))
            .h_full()
            .flex()
            .flex_col()
            .bg(p.bg_sidebar)
            .border_r_1()
            .border_color(p.border)
            .font_family(crate::ui::sans_family(cx))
            .child(resize_handle("files", false, cx))
            .when(files.is_empty(), |d| d.child(div().p(u(12.)).text_size(u(12.)).text_color(p.fg_muted).child("No changed files")))
            .child(div().relative().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(gpui_kit::base::Scrollbar::vertical(&self.file_scroll)))
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(u(4.))
                    .px(u(8.))
                    .py(u(4.))
                    .border_t_1()
                    .border_color(p.border)
                    .child(mode_button("file-list-list", "list", !tree, cx))
                    .child(mode_button("file-list-tree", "tree", tree, cx))
                    // The filter, its menu opening right above it.
                    .child(div().relative().ml(u(6.)).flex_none().child(
                        div()
                            .id("file-filter")
                            .flex()
                            .items_center()
                            .gap(u(4.))
                            .px(u(6.))
                            .py(u(2.))
                            .rounded(u(6.))
                            .border_1()
                            .border_color(p.border)
                            .cursor_pointer()
                            .text_size(u(11.))
                            .text_color(p.fg_muted)
                            .when(active, |d| d.bg(p.selection).text_color(p.fg))
                            .tooltip(move |window, cx| {
                                let tip = if filter.off { "Filter files (f) — switched off, F switches it back on" } else { "Filter files (f)" };
                                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.filter_open = !this.filter_open;
                                this.filter_highlight = 0;
                                // ↑/↓ and Space work the menu however it was opened.
                                if this.filter_open {
                                    let focus = this.filter_focus(cx);
                                    window.focus(&focus, cx);
                                } else {
                                    window.focus(&this.focus_handle_diff(), cx);
                                }
                                cx.notify();
                            }))
                            .child(crate::ui::icon("filter", 14., if active { p.accent } else { p.fg_muted }))
                            .when(filtered_count > 0, |d| d.child(format!("{filtered_count} hidden")))
                            .when(self.filter().off && !self.filter().exts.is_empty(), |d| d.child("filter off")),
                    )
                    .when(self.filter_open, |d| d.child(self.render_filter_menu(cx)))),
            )
            .into_any_element()
    }

    fn render_file_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = palette(cx).clone();
        let rows = self.file_rows.clone();
        let current = self.file_list_current;
        let show_prs = self.hi > self.lo;
        range
            .filter_map(|at| rows.get(at))
            .map(|row| match row {
                FileRow::File { index, depth } => {
                    let (i, depth) = (*index, *depth);
                    let f = &self.view.summary.files[i];
                    let viewed = self.session.read(cx).is_viewed(&f.content_key);
                    let collapsed = self.built.segments.get(i).is_some_and(|s| s.mode == Mode::Collapsed);
                    let threads = self.tallies.get(&i).and_then(|t| t.get("open")).copied().unwrap_or(0);
                    let (dir, name) = match f.path.rfind('/') {
                        Some(at) => (f.path[..=at].to_string(), f.path[at + 1..].to_string()),
                        None => (String::new(), f.path.clone()),
                    };
                    let path = f.path.clone();
                    div()
                        .id(("file", i))
                        .w_full()
                        .h(u(24.))
                        .flex()
                        .items_center()
                        .gap(u(6.))
                        .pr(u(10.))
                        .pl(u(depth.map_or(12., |d| 12. + d as f32 * 8.)))
                        .cursor_pointer()
                        .text_size(u(12.))
                        .whitespace_nowrap()
                        .when(Some(i) == current, |d| d.bg(p.selection))
                        .when(Some(i) != current, |d| d.hover(|d| d.bg(p.selection.opacity(0.5))))
                        .when(collapsed, |d| d.text_color(p.fg_muted))
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remember(cx);
                            this.go_to_file(i, true, cx);
                        }))
                        .child(div().w(u(12.)).flex_none().font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(if viewed { p.add_fg } else { status_color(f.status, &p) }).child(if viewed { "✓" } else { status_letter(f.status) }))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                // The name stays whole; a long directory gives way.
                                .when(depth.is_none(), |d| {
                                    d.child(div().min_w(px(0.)).flex_shrink(1.).overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(p.fg_muted).child(dir))
                                })
                                .child(div().flex_none().when(viewed, |d| d.text_color(p.fg_muted)).child(name)),
                        )
                        .when(show_prs, |d| d.child(div().flex().gap(u(2.)).children(f.prs.iter().map(|&pr| div().size(u(6.)).rounded_full().bg(p.pr(pr))))))
                        .when(threads > 0, |d| {
                            let tip = format!("{threads} open thread{}", if threads == 1 { "" } else { "s" });
                            d.child(
                                div()
                                    .id(SharedString::from(format!("file-threads-{}", f.path)))
                                    .flex()
                                    .items_center()
                                    .gap(u(2.))
                                    .text_color(p.accent)
                                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                                    .child(comment_icon(false, p.accent, p.bg_sidebar))
                                    .child(threads.to_string()),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .gap(u(4.))
                                .text_size(u(11.))
                                .when(f.additions > 0, |d| d.child(div().text_color(p.add_fg).child(format!("+{}", f.additions))))
                                .when(f.deletions > 0, |d| d.child(div().text_color(p.del_fg).child(format!("−{}", f.deletions)))),
                        )
                        .into_any_element()
                }
                FileRow::Dir { path, name, depth, files } => {
                    let open = !self.tree_closed.contains(path);
                    let s = self.session.read(cx);
                    let all_viewed = files.iter().all(|&i| s.is_viewed(&self.view.summary.files[i].content_key));
                    let toggle = path.clone();
                    div()
                        .id(SharedString::from(format!("dir-{path}")))
                        .w_full()
                        .h(u(24.))
                        .flex()
                        .items_center()
                        .gap(u(6.))
                        .pl(u(12. + *depth as f32 * 8.))
                        .pr(u(10.))
                        .cursor_pointer()
                        .text_size(u(12.))
                        .hover(|d| d.bg(p.selection.opacity(0.5)))
                        .when(all_viewed, |d| d.text_color(p.fg_muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.tree_closed.remove(&toggle) {
                                this.tree_closed.insert(toggle.clone());
                            }
                            cx.notify();
                        }))
                        .child(div().w(u(12.)).text_color(p.fg_muted).child(if open { "▾" } else { "▸" }))
                        .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(FontWeight::MEDIUM).child(name.clone()))
                        .when(all_viewed, |d| d.child(div().text_color(p.add_fg).child("✓")))
                        .when(!all_viewed && !open, |d| d.child(div().text_size(u(11.)).text_color(p.fg_muted).child(files.len().to_string())))
                        .into_any_element()
                }
            })
            .collect()
    }

    /// The menu's extensions: each with its file count and depth. Under an extension come the
    /// multi-part ones ending in it that two files or more share (".unit.php" under ".php"), so
    /// e.g. unit tests can be left out alone; files without one last.
    pub(crate) fn extensions(&self) -> Vec<(String, usize, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for f in &self.view.summary.files {
            let suffixes = extension_suffixes(&f.path);
            if suffixes.is_empty() {
                *counts.entry(String::new()).or_default() += 1;
            }
            for s in suffixes {
                *counts.entry(s).or_default() += 1;
            }
        }
        let none = counts.remove("");
        let parts = |s: &str| s.matches('.').count();
        let mut out = Vec::new();
        for (base, n) in counts.iter().filter(|(s, _)| parts(s) == 1) {
            out.push((base.clone(), *n, 0));
            // Longer ones after the shorter ones they end in (".a.unit.php" under ".unit.php").
            let mut more: Vec<(&String, &usize)> = counts.iter().filter(|(s, c)| parts(s) > 1 && **c >= 2 && s.ends_with(base.as_str())).collect();
            more.sort_by(|a, b| a.0.split('.').rev().cmp(b.0.split('.').rev()));
            out.extend(more.into_iter().map(|(s, c)| (s.clone(), *c, parts(s) - 1)));
        }
        if let Some(n) = none {
            out.push((String::new(), n, 0));
        }
        out
    }

    /// Ticking an extension (or "Show all") switches a filter that was off back on.
    fn toggle_extension(&mut self, ext: &str, cx: &mut Context<Self>) {
        let filter = self.filter();
        let exts = if filter.exts.iter().any(|e| e == ext) {
            filter.exts.into_iter().filter(|e| e != ext).collect()
        } else {
            let mut e = filter.exts;
            e.push(ext.to_string());
            e
        };
        self.change_filter(FileFilter { exts, off: false }, cx);
    }

    fn render_filter_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let exts = self.extensions();
        let filter = self.filter();
        let highlight = self.filter_highlight.min(exts.len().saturating_sub(1));
        let count = exts.len();
        gpui_kit::deferred(
            div().absolute().left_0().bottom(gpui_kit::relative(1.)).mb(u(6.)).child(
                div()
                    .id("file-filter-menu")
                    .key_context("FileFilter")
                    .track_focus(&self.filter_focus(cx))
                    .on_action(cx.listener(move |this, _: &FilterDown, _, cx| {
                        this.filter_highlight = (this.filter_highlight + 1).min(count.saturating_sub(1));
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &FilterUp, _, cx| {
                        this.filter_highlight = this.filter_highlight.saturating_sub(1);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &FilterToggle, _, cx| {
                        let exts = this.extensions();
                        if let Some((ext, _, _)) = exts.get(this.filter_highlight.min(exts.len().saturating_sub(1))) {
                            let ext = ext.clone();
                            this.toggle_extension(&ext, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &FilterClose, window, cx| {
                        this.filter_open = false;
                        window.focus(&this.focus_handle_diff(), cx);
                        cx.notify();
                    }))
                    .w(u(220.))
                    .max_h(u(360.))
                    .overflow_y_scroll()
                    .rounded(u(8.))
                    .border_1()
                    .border_color(p.floating_border.unwrap_or(p.border))
                    .bg(p.bg_elevated)
                    .when(p.shadows, |d| d.shadow_lg())
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.filter_open = false;
                        cx.notify();
                    }))
                    .py(u(4.))
                    .text_size(u(12.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .px(u(10.))
                            .py(u(4.))
                            .text_size(u(11.))
                            .text_color(p.fg_muted)
                            .child("File extensions")
                            .when(!filter.exts.is_empty(), |d| {
                                d.child(
                                    crate::ui::link_button("Show all", cx)
                                        .id("file-filter-reset")
                                        .on_click(cx.listener(|this, _, _, cx| this.change_filter(FileFilter::default(), cx))),
                                )
                            }),
                    )
                    .children(exts.into_iter().enumerate().map(|(i, (ext, n, depth))| {
                        let shown = !filter.exts.contains(&ext);
                        // Under an unticked extension, its longer ones are out either way.
                        let covered = depth > 0 && filter.exts.iter().any(|e| e != &ext && ext.ends_with(e.as_str()) && e.starts_with('.'));
                        let label = if ext.is_empty() { "No extension".to_string() } else { ext.clone() };
                        div()
                            .id(SharedString::from(format!("file-filter-ext{ext}")))
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .px(u(10.))
                            .py(u(3.))
                            .cursor_pointer()
                            .when(i == highlight, |d| d.bg(p.selection))
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.filter_highlight != i {
                                    this.filter_highlight = i;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_extension(&ext, cx)))
                            .when(covered, |d| d.opacity(0.5))
                            .pl(u(10. + depth as f32 * 14.))
                            .child(div().w(u(12.)).text_color(p.accent).child(if shown && !covered { "✓" } else { "" }))
                            .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
                            .child(div().text_color(p.fg_muted).child(n.to_string()))
                    })),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn filter_focus(&mut self, cx: &mut Context<Self>) -> gpui_kit::FocusHandle {
        self.filter_focus_handle.get_or_insert_with(|| cx.focus_handle()).clone()
    }
}
