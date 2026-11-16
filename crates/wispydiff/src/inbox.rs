//! Home: review requests and your own PRs, grouped into stacks. j/k to move, Enter to open,
//! Space/e to collapse a stack, r to refresh.

use std::collections::HashSet;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, Render, ScrollHandle, SharedString,
    Window,
};
use wispy_core::inbox::InboxPr;
use wispy_core::service::{InboxEntry, PrProgress};

use crate::settings;
use crate::ui::{key_label, link_button, palette, status_badge, u, wordmark, BadgeKind};

gpui_kit::actions!(inbox, [Down, Up, Open, Toggle, Refresh]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Inbox && !Input");
    cx.bind_keys([
        KeyBinding::new("j", Down, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("k", Up, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("enter", Open, ctx),
        KeyBinding::new("space", Toggle, ctx),
        KeyBinding::new("e", Toggle, ctx),
        KeyBinding::new("r", Refresh, ctx),
    ]);
}

pub enum InboxEvent {
    Open(String),
    Refresh,
}

fn group_key(entry: &InboxEntry) -> String {
    format!("{}#{}", entry.group.repo, entry.group.prs[0].number)
}

fn pr_label(pr: &InboxPr) -> String {
    format!("{}#{}", pr.repo, pr.number)
}

/// Reviewed or marked at the PR's current head.
fn up_to_date(p: &PrProgress) -> bool {
    (p.review.is_some() && !p.changed_since_review) || (p.marked && !p.changed_since_marked)
}

fn changed_since(p: &PrProgress) -> bool {
    (p.review.is_some() || p.marked) && !up_to_date(p)
}

fn review_label(review: &str) -> String {
    match review {
        "APPROVED" => "you approved".into(),
        "CHANGES_REQUESTED" => "you requested changes".into(),
        "COMMENTED" => "you commented".into(),
        "DISMISSED" => "your review was dismissed".into(),
        other => other.to_lowercase(),
    }
}

pub struct Inbox {
    pub entries: Vec<InboxEntry>,
    pub online: Option<bool>,
    collapsed: HashSet<String>,
    /// The selected item: a PR label, or a collapsed stack as `group:<key>`.
    selected_id: Option<String>,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

impl EventEmitter<InboxEvent> for Inbox {}

impl Inbox {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Inbox {
            entries: Vec::new(),
            online: None,
            collapsed: settings::list(cx, "inboxCollapsed").into_iter().collect(),
            selected_id: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    pub fn set_entries(&mut self, entries: Vec<InboxEntry>, cx: &mut Context<Self>) {
        self.entries = entries;
        cx.notify();
    }

    pub fn set_online(&mut self, online: Option<bool>, cx: &mut Context<Self>) {
        self.online = online;
        cx.notify();
    }

    /// What j/k move over: each PR, or a collapsed stack as a whole.
    fn items(&self) -> Vec<String> {
        self.entries
            .iter()
            .flat_map(|e| {
                let key = group_key(e);
                if self.collapsed.contains(&key) && e.group.prs.len() > 1 {
                    vec![format!("group:{key}")]
                } else {
                    e.group.prs.iter().map(pr_label).collect()
                }
            })
            .collect()
    }

    fn selected(&self, items: &[String]) -> usize {
        self.selected_id.as_ref().and_then(|id| items.iter().position(|i| i == id)).unwrap_or(0)
    }

    fn entry_of(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| match id.strip_prefix("group:") {
            Some(key) => group_key(e) == key,
            None => e.group.prs.iter().any(|p| pr_label(p) == id),
        })
    }

    fn toggle_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let entry = &self.entries[index];
        let key = group_key(entry);
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key.clone());
        }
        settings::set_list(cx, "inboxCollapsed", self.collapsed.iter().cloned().collect());
        let entry = &self.entries[index];
        self.selected_id =
            Some(if self.collapsed.contains(&key) { format!("group:{key}") } else { pr_label(&entry.group.prs[0]) });
        cx.notify();
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let items = self.items();
        let next = (self.selected(&items) + 1).min(items.len().saturating_sub(1));
        self.selected_id = items.get(next).cloned();
        cx.notify();
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let items = self.items();
        let next = self.selected(&items).saturating_sub(1);
        self.selected_id = items.get(next).cloned();
        cx.notify();
    }

    fn open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        let items = self.items();
        let Some(current) = items.get(self.selected(&items)).cloned() else { return };
        if current.starts_with("group:") {
            if let Some(index) = self.entry_of(&current) {
                self.toggle_entry(index, cx);
            }
        } else {
            cx.emit(InboxEvent::Open(current));
        }
    }

    fn toggle(&mut self, _: &Toggle, _: &mut Window, cx: &mut Context<Self>) {
        let items = self.items();
        let Some(current) = items.get(self.selected(&items)).cloned() else { return };
        if let Some(index) = self.entry_of(&current) {
            if self.entries[index].group.prs.len() > 1 {
                self.toggle_entry(index, cx);
            }
        }
    }

    fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InboxEvent::Refresh);
    }

    fn progress_badges(label: &str, progress: Option<&PrProgress>, cx: &App) -> Vec<gpui_kit::AnyElement> {
        let Some(p) = progress else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(review) = &p.review {
            let mut text = review_label(review);
            if p.changed_since_review {
                text.push_str(" · new commits");
            }
            let tip = if p.changed_since_review { "New commits were pushed since your review" } else { "Your latest review on GitHub is on the current head" };
            let badge = status_badge(text, if p.changed_since_review { BadgeKind::Warn } else { BadgeKind::Ok }, cx);
            out.push(with_tip(badge.flex_none(), format!("review-{label}"), tip).into_any_element());
        }
        if p.marked {
            let text = if p.changed_since_marked { "marked reviewed · changed since" } else { "marked reviewed" };
            let tip = if p.changed_since_marked { "Changed since you marked it reviewed (d shows what's new)" } else { "Marked reviewed at the current head" };
            let badge = status_badge(text, if p.changed_since_marked { BadgeKind::Warn } else { BadgeKind::Ok }, cx);
            out.push(with_tip(badge.flex_none(), format!("marked-{label}"), tip).into_any_element());
        }
        out
    }
}

impl Focusable for Inbox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Inbox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let root = div()
            .key_context("Inbox")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::toggle))
            .on_action(cx.listener(Self::refresh))
            .size_full()
            .bg(p.bg)
            .text_color(p.fg);
        if self.entries.is_empty() {
            let hint = if self.online == Some(false) {
                "Offline — no cached inbox yet".to_string()
            } else {
                format!("Nothing waiting for you · {} to open any pull request", key_label("K"))
            };
            return root
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(u(14.))
                .child(wordmark(36., p.fg_muted.opacity(0.5)))
                .child(div().text_size(u(13.)).text_color(p.fg_muted).child(hint))
                .into_any_element();
        }

        let items = self.items();
        let selected = items.get(self.selected(&items)).cloned();
        let status = if self.online == Some(false) {
            "offline · showing cached".to_string()
        } else {
            let n = self.entries.len();
            format!("{n} group{}", if n == 1 { "" } else { "s" })
        };
        let groups = self.entries.iter().enumerate().map(|(index, entry)| {
            let key = group_key(entry);
            let stack = entry.group.prs.len() > 1;
            let collapsed = stack && self.collapsed.contains(&key);
            let head_selected = collapsed && selected.as_deref() == Some(&format!("group:{key}"));
            let current = entry.progress.iter().filter(|p| up_to_date(p)).count();
            let changed = entry.progress.iter().filter(|p| changed_since(p)).count();
            let group_id = format!("group:{key}");
            let head = div()
                .id(SharedString::from(format!("inbox-head-{key}")))
                .flex()
                .items_center()
                .gap(u(8.))
                .px(u(12.))
                .py(u(8.))
                .rounded(u(6.))
                .when(head_selected, |d| d.bg(p.selection))
                .when(collapsed, |d| {
                    let hover_id = group_id.clone();
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_entry(index, cx)))
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if this.selected_id.as_deref() != Some(&hover_id) {
                                this.selected_id = Some(hover_id.clone());
                                cx.notify();
                            }
                        }))
                })
                .when(stack, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("inbox-toggle-{key}")))
                            .w(u(16.))
                            .text_color(p.fg_muted)
                            .cursor_pointer()
                            .child(if collapsed { "▸" } else { "▾" })
                            .tooltip(tip(if collapsed { "Expand (Space)" } else { "Collapse (Space)" }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_entry(index, cx);
                            })),
                    )
                })
                .child(div().font_weight(FontWeight::SEMIBOLD).text_size(u(13.)).child(entry.group.repo.clone()))
                .when(stack, |d| d.child(status_badge(format!("stack of {}", entry.group.prs.len()), BadgeKind::Plain, cx)))
                .when(collapsed && current > 0, |d| {
                    d.child(status_badge(format!("{current}/{} reviewed", entry.group.prs.len()), BadgeKind::Ok, cx))
                })
                .when(collapsed && changed > 0, |d| d.child(status_badge(format!("{changed} changed since"), BadgeKind::Warn, cx)))
                .when(entry.drafts > 0, |d| {
                    d.child(status_badge(
                        format!("{} draft{}", entry.drafts, if entry.drafts == 1 { "" } else { "s" }),
                        BadgeKind::Warn,
                        cx,
                    ))
                })
                .child(div().flex_1())
                .child(
                    div()
                        .id(SharedString::from(format!("inbox-ready-{key}")))
                        .text_size(u(11.))
                        .text_color(if entry.ready { p.add_fg } else { p.fg_muted })
                        .tooltip(tip(if entry.ready { "Prefetched: opens instantly, offline too" } else { "Prefetching…" }))
                        .child(if entry.ready { "offline ready" } else { "prefetching…" }),
                );
            let prs = (!collapsed).then(|| {
                div().flex().flex_col().children(entry.group.prs.iter().enumerate().map(|(i, pr)| {
                    let label = pr_label(pr);
                    let (open_label, hover_label) = (label.clone(), label.clone());
                    div()
                        .id(SharedString::from(format!("inbox-pr-{label}")))
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .px(u(12.))
                        .pl(u(36.))
                        .py(u(6.))
                        .rounded(u(6.))
                        .cursor_pointer()
                        .when(selected.as_deref() == Some(&label), |d| d.bg(p.selection))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(InboxEvent::Open(open_label.clone()))))
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if this.selected_id.as_deref() != Some(&hover_label) {
                                this.selected_id = Some(hover_label.clone());
                                cx.notify();
                            }
                        }))
                        .child(div().flex_none().text_size(u(12.)).text_color(p.fg_muted).child(format!("#{}", pr.number)))
                        // Up to half the row; the branch names on the right give way first.
                        .child(
                            div()
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .max_w(gpui_kit::relative(0.5))
                                .text_size(u(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(pr.title.clone()),
                        )
                        .when(pr.draft, |d| d.child(status_badge("draft", BadgeKind::Plain, cx).flex_none()))
                        .children(Self::progress_badges(&pr_label(pr), entry.progress.get(i), cx))
                        .child(div().flex_1())
                        .child(div().flex_shrink(100.).min_w(px(0.)).overflow_hidden().text_ellipsis().text_size(u(12.)).text_color(p.fg_muted).whitespace_nowrap().child(format!(
                            "{} · {} · {} ← {}",
                            if pr.requested { "review requested" } else { "yours" },
                            pr.author,
                            pr.base_ref,
                            pr.head_ref
                        )))
                }))
            });
            div().flex().flex_col().gap(u(2.)).child(head).children(prs)
        });
        root.relative().child(
            div().id("inbox-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(
                div()
                    .max_w(u(1000.))
                    .mx_auto()
                    .p(u(24.))
                    .flex()
                    .flex_col()
                    .gap(u(10.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(u(10.))
                            .child(div().text_size(u(18.)).font_weight(FontWeight::SEMIBOLD).child("Inbox"))
                            .child(div().text_size(u(12.)).text_color(p.fg_muted).child(status))
                            .child(link_button("refresh", cx).id("inbox-refresh").on_click(cx.listener(|_, _, _, cx| cx.emit(InboxEvent::Refresh)))),
                    )
                    .children(groups),
            ),
        )
        .child(gpui_kit::base::Scrollbar::vertical(&self.scroll))
        .into_any_element()
    }
}

fn tip(text: &'static str) -> impl Fn(&mut gpui_kit::Window, &mut App) -> gpui_kit::AnyView + 'static {
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
}

fn with_tip(badge: gpui_kit::Div, id: String, text: &'static str) -> gpui_kit::Stateful<gpui_kit::Div> {
    badge.id(SharedString::from(id)).tooltip(tip(text))
}
