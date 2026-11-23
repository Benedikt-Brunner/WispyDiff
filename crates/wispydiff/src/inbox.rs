//! Home: review requests and your own PRs, grouped into stacks. j/k to move, Enter to open,
//! Space/e to collapse a stack, r to refresh, i (or a right-click) for a PR's description, f to
//! find PRs by number, title or branch.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::AnimationExt as _;
use gpui_kit::{
    div, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, Render, ScrollHandle,
    SharedString, Subscription, Window,
};
use wispy_core::inbox::InboxPr;
use wispy_core::service::{InboxEntry, PrProgress};

use crate::backend::RefreshProgress;
use crate::logic::stack_tree::guides;
use crate::logic::time::{ago, exact_local, now_unix};
use crate::settings;
use crate::ui::{key_label, link_button, palette, status_badge, u, wordmark, BadgeKind};

gpui_kit::actions!(inbox, [Down, Up, Open, Toggle, Refresh, Describe, Find]);

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
        KeyBinding::new("i", Describe, ctx),
        KeyBinding::new("f", Find, ctx),
    ]);
}

pub enum InboxEvent {
    Open(String),
    Refresh,
    /// Show this PR's description (`owner/repo#N`).
    Describe(String),
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
    pub refreshing: Option<RefreshProgress>,
    collapsed: HashSet<String>,
    /// The selected item: a PR label, or a collapsed stack as `group:<key>`.
    selected_id: Option<String>,
    /// The find box (`f`) while it's open, and what's typed in it.
    search: Option<Entity<InputState>>,
    query: String,
    _search_subscription: Option<Subscription>,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

/// The groups the find box leaves, best match first: each group's index, the PRs in it that
/// match, and its best score.
type Found = Vec<(usize, Vec<usize>, f64)>;

impl EventEmitter<InboxEvent> for Inbox {}

impl Inbox {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Keeps the "fetched … ago" ages current.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        Inbox {
            entries: Vec::new(),
            online: None,
            refreshing: None,
            collapsed: settings::list(cx, "inboxCollapsed").into_iter().collect(),
            selected_id: None,
            search: None,
            query: String::new(),
            _search_subscription: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    pub fn searching(&self) -> bool {
        self.search.is_some()
    }

    /// What j/k move over, in order (for tests): PR labels, or `group:…` for a collapsed stack.
    pub fn listed(&self) -> Vec<String> {
        self.items()
    }

    /// The selected item of [`Inbox::listed`] (for tests).
    pub fn selected_item(&self) -> Option<String> {
        let items = self.items();
        items.get(self.selected(&items)).cloned()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// What the find box leaves (none while nothing is typed).
    fn found(&self) -> Option<Found> {
        if self.query.trim().is_empty() {
            return None;
        }
        let mut found: Found = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let scored: Vec<(usize, f64)> = entry
                    .group
                    .prs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, pr)| crate::logic::inbox_search::score(&self.query, pr.number, &pr.title, &pr.head_ref).map(|s| (i, s)))
                    .collect();
                let best = scored.iter().map(|(_, s)| *s).reduce(f64::max)?;
                Some((index, scored.into_iter().map(|(i, _)| i).collect(), best))
            })
            .collect();
        found.sort_by(|a, b| b.2.total_cmp(&a.2));
        Some(found)
    }

    /// `f`: opens the find box (or goes back to it).
    fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search {
            search.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find by #number, title or branch"));
        self._search_subscription = Some(cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                this.query = input.read(cx).value().to_string();
                this.select_best();
                this.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.open_selected(cx),
            _ => {}
        }));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.search = Some(input);
        cx.notify();
    }

    /// Closes the find box: the whole inbox again, the selection kept.
    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.query.clear();
        self._search_subscription = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Opens a PR; the find box is done with (the inbox shows everything when it's back).
    fn open_pr(&mut self, label: String, cx: &mut Context<Self>) {
        self.search = None;
        self.query.clear();
        self._search_subscription = None;
        cx.emit(InboxEvent::Open(label));
    }

    /// Selects the PR matching what's typed best.
    fn select_best(&mut self) {
        let Some(found) = self.found() else { return };
        let best = found.first().and_then(|(index, prs, _)| {
            let entry = &self.entries[*index];
            prs.iter()
                .map(|&i| (i, crate::logic::inbox_search::score(&self.query, entry.group.prs[i].number, &entry.group.prs[i].title, &entry.group.prs[i].head_ref)))
                .max_by(|a, b| a.1.unwrap_or(0.).total_cmp(&b.1.unwrap_or(0.)))
                .map(|(i, _)| pr_label(&entry.group.prs[i]))
        });
        if best.is_some() {
            self.selected_id = best;
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

    pub fn set_refreshing(&mut self, refreshing: Option<RefreshProgress>, cx: &mut Context<Self>) {
        if self.refreshing != refreshing {
            self.refreshing = refreshing;
            cx.notify();
        }
    }

    /// How far the running refresh is, shown beside the refresh link.
    pub fn refresh_progress(&self) -> Option<String> {
        Some(match self.refreshing? {
            RefreshProgress::Listing => "checking GitHub…".into(),
            RefreshProgress::Prefetching { done, total } => format!("prefetching {}/{total}…", done + 1),
        })
    }

    /// What j/k move over: each PR, or a collapsed stack as a whole.
    fn items(&self) -> Vec<String> {
        if let Some(found) = self.found() {
            return found.iter().flat_map(|(index, prs, _)| prs.iter().map(|&i| pr_label(&self.entries[*index].group.prs[i]))).collect();
        }
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
        self.open_selected(cx);
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let items = self.items();
        let Some(current) = items.get(self.selected(&items)).cloned() else { return };
        if current.starts_with("group:") {
            if let Some(index) = self.entry_of(&current) {
                self.toggle_entry(index, cx);
            }
        } else {
            self.open_pr(current, cx);
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

    fn describe(&mut self, _: &Describe, _: &mut Window, cx: &mut Context<Self>) {
        let items = self.items();
        let Some(current) = items.get(self.selected(&items)).cloned() else { return };
        if !current.starts_with("group:") {
            cx.emit(InboxEvent::Describe(current));
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
    /// The find box while it's open (it keeps the keyboard when the inbox gets it back).
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.search {
            Some(search) => search.read(cx).focus_handle(cx),
            None => self.focus.clone(),
        }
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
            .on_action(cx.listener(Self::describe))
            .on_action(cx.listener(Self::find))
            // The find box takes ↑/↓/Esc itself: the list needs them first.
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::MoveDown, window, cx| {
                if this.search.is_some() {
                    cx.stop_propagation();
                    this.down(&Down, window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::MoveUp, window, cx| {
                if this.search.is_some() {
                    cx.stop_propagation();
                    this.up(&Up, window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                if this.search.is_some() {
                    cx.stop_propagation();
                    this.close_search(window, cx);
                }
            }))
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
        let now = now_unix();
        let status = if self.online == Some(false) {
            "offline · showing cached".to_string()
        } else {
            let n = self.entries.len();
            format!("{n} group{}", if n == 1 { "" } else { "s" })
        };
        let mono = crate::ui::mono_family(cx);
        // While finding: the groups with matches, best first, and only their matching PRs.
        let found = self.found();
        let filtering = found.is_some();
        let shown: Vec<(usize, Option<Vec<usize>>)> = match found {
            Some(found) => found.into_iter().map(|(index, prs, _)| (index, Some(prs))).collect(),
            None => (0..self.entries.len()).map(|index| (index, None)).collect(),
        };
        let nothing_found = filtering && shown.is_empty();
        let find = self.render_find(cx);
        let groups = shown.into_iter().map(|(index, only)| {
            let entry = &self.entries[index];
            let key = group_key(entry);
            let stack = entry.group.prs.len() > 1;
            let collapsed = stack && !filtering && self.collapsed.contains(&key);
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
                .when(stack, |d| {
                    let branches = entry.group.branches();
                    let text = match branches {
                        1 => format!("stack of {}", entry.group.prs.len()),
                        n => format!("stack of {} · {n} branches", entry.group.prs.len()),
                    };
                    d.child(status_badge(text, BadgeKind::Plain, cx))
                })
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
                .when_some(entry.fetched_at, |d, fetched| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("inbox-fetched-{key}")))
                            .flex_none()
                            .text_size(u(11.))
                            .text_color(p.fg_muted)
                            .tooltip(move |window, cx| {
                                let text = format!("Fetched from GitHub on {}", exact_local(fetched));
                                gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
                            })
                            .child(format!("fetched {}", ago(fetched, now))),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("inbox-ready-{key}")))
                        .flex_none()
                        .text_size(u(11.))
                        .text_color(if entry.ready { p.add_fg } else { p.fg_muted })
                        .tooltip(tip(if entry.ready { "Prefetched: opens instantly, offline too" } else { "Prefetching…" }))
                        .child(if entry.ready { "offline ready" } else { "prefetching…" }),
                );
            let prs = (!collapsed).then(|| {
                let guides = guides(entry.group.prs.len(), |i| entry.group.parent(i));
                let only = only.clone();
                let rows = entry.group.prs.iter().enumerate().filter(move |(i, _)| only.as_ref().is_none_or(|only| only.contains(i)));
                div().flex().flex_col().children(rows.map(|(i, pr)| {
                    // A subset of the tree has no lines to draw between its PRs.
                    let guide = if filtering { String::new() } else { guides[i].clone() };
                    let label = pr_label(pr);
                    let (open_label, hover_label) = (label.clone(), label.clone());
                    let (menu_label, button_label) = (label.clone(), label.clone());
                    let is_selected = selected.as_deref() == Some(&label);
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
                        .when(is_selected, |d| d.bg(p.selection))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_pr(open_label.clone(), cx)))
                        // A right-click shows the description.
                        .on_mouse_down(
                            gpui_kit::MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                this.selected_id = Some(menu_label.clone());
                                cx.emit(InboxEvent::Describe(menu_label.clone()));
                            }),
                        )
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if this.selected_id.as_deref() != Some(&hover_label) {
                                this.selected_id = Some(hover_label.clone());
                                cx.notify();
                            }
                        }))
                        // Where the stack branches: which PR this one builds on.
                        .when(!guide.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .font_family(mono.clone())
                                    .text_size(u(12.))
                                    .text_color(p.fg_muted.opacity(0.6))
                                    .whitespace_nowrap()
                                    .child(guide),
                            )
                        })
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
                        .when(pr.auto_merge, |d| d.child(crate::ui::auto_merge_badge(format!("inbox-auto-merge-{label}"), cx)))
                        .children(Self::progress_badges(&pr_label(pr), entry.progress.get(i), cx))
                        .child(div().flex_1())
                        .when_some(pr.stats, |d, stats| {
                            let files = format!("{} file{} changed", stats.files, if stats.files == 1 { "" } else { "s" });
                            d.child(
                                div()
                                    .id(SharedString::from(format!("inbox-stats-{label}")))
                                    .flex()
                                    .flex_none()
                                    .gap(u(4.))
                                    .text_size(u(12.))
                                    .whitespace_nowrap()
                                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(files.clone()).build(window, cx))
                                    .child(div().text_color(p.add_fg).child(format!("+{}", stats.additions)))
                                    .child(div().text_color(p.del_fg).child(format!("−{}", stats.deletions))),
                            )
                        })
                        .child(div().flex_shrink(100.).min_w(px(0.)).overflow_hidden().text_ellipsis().text_size(u(12.)).text_color(p.fg_muted).whitespace_nowrap().child(format!(
                            "{} · {} · {} ← {}",
                            if pr.requested { "review requested" } else { "yours" },
                            pr.author,
                            pr.base_ref,
                            pr.head_ref
                        )))
                        // On the row under the mouse (or selected with j/k), at its end.
                        .when(is_selected, |d| {
                            d.child(
                                link_button("description", cx)
                                    .id(SharedString::from(format!("inbox-description-{label}")))
                                    .flex_none()
                                    .tooltip(tip("Show the description (i, or right-click)"))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.emit(InboxEvent::Describe(button_label.clone()));
                                    })),
                            )
                        })
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
                            .relative()
                            .flex()
                            .items_baseline()
                            .gap(u(10.))
                            .child(find)
                            .child(div().text_size(u(18.)).font_weight(FontWeight::SEMIBOLD).child("Inbox"))
                            .child(div().text_size(u(12.)).text_color(p.fg_muted).child(status))
                            .child(link_button("refresh", cx).id("inbox-refresh").on_click(cx.listener(|_, _, _, cx| cx.emit(InboxEvent::Refresh))))
                            .when_some(self.refresh_progress(), |d, progress| {
                                d.child(
                                    div()
                                        .text_size(u(12.))
                                        .text_color(p.fg_muted)
                                        .child(progress)
                                        .with_animation(
                                            "inbox-refreshing",
                                            gpui_kit::Animation::new(Duration::from_millis(1200))
                                                .repeat()
                                                .with_easing(gpui_kit::pulsating_between(0.4, 1.)),
                                            |d, delta| d.opacity(delta),
                                        ),
                                )
                            }),
                    )
                    .children(groups)
                    .when(nothing_found, |d| {
                        d.child(div().px(u(12.)).py(u(8.)).text_size(u(13.)).text_color(p.fg_muted).child(format!("No PR matches “{}”", self.query.trim())))
                    }),
            ),
        )
        .child(gpui_kit::base::Scrollbar::vertical(&self.scroll))
        .into_any_element()
    }
}

impl Inbox {
    /// The find box, floating at the right of the heading so nothing moves when it opens; a
    /// "find (f)" link in its place while it's closed.
    fn render_find(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let p = palette(cx).clone();
        let place = div().absolute().right_0().top(u(-4.)).h(u(30.)).flex().items_center();
        let Some(search) = &self.search else {
            return place
                .child(
                    link_button("find (f)", cx)
                        .id("inbox-find")
                        .tooltip(tip("Find PRs by number, title or branch"))
                        .on_click(cx.listener(|this, _, window, cx| this.find(&Find, window, cx))),
                )
                .into_any_element();
        };
        let count = self.found().map(|found| found.iter().map(|(_, prs, _)| prs.len()).sum::<usize>());
        place
            .w(u(340.))
            .gap(u(6.))
            .pl(u(4.))
            .pr(u(10.))
            .rounded(u(8.))
            .bg(p.bg)
            .border_1()
            .border_color(p.accent.opacity(0.6))
            .when(p.shadows, |d| d.shadow_md())
            .child(div().flex_1().min_w(px(0.)).child(Input::new(search).appearance(false).text_size(u(13.))))
            .when_some(count, |d, count| {
                let text = match count {
                    0 => "none".to_string(),
                    1 => "1 PR".to_string(),
                    n => format!("{n} PRs"),
                };
                d.child(div().flex_none().text_size(u(11.)).text_color(p.fg_muted).child(text))
            })
            .child(div().flex_none().text_size(u(11.)).text_color(p.fg_muted.opacity(0.7)).child("esc"))
            .into_any_element()
    }
}

fn tip(text: &'static str) -> impl Fn(&mut gpui_kit::Window, &mut App) -> gpui_kit::AnyView + 'static {
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
}

fn with_tip(badge: gpui_kit::Div, id: String, text: &'static str) -> gpui_kit::Stateful<gpui_kit::Div> {
    badge.id(SharedString::from(id)).tooltip(tip(text))
}
