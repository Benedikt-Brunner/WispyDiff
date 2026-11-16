//! One step from the diff: review each PR of the stack with its drafts, verdict and summary, and
//! merge it if wanted (was `SubmitSheet.tsx`).

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::input::{IndentInline, Input, InputEvent, InputState, OutdentInline, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    anchored, deferred, div, px, AnyElement, App, ClipboardEntry, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, KeyBinding, Render, SharedString, Subscription, Window,
};
use wispy_core::drafts::{Draft, DraftKind, DraftStatus};
use wispy_core::github::Verdict;
use wispy_core::merge::{MergeInfo, MergeMethod, MergeOutcome, MergeRequest};
use wispy_core::review::{Outcome, Plan, Planned};

use crate::backend::{Backend, OpenedStack, SubmitPlan};
use crate::logic::comments::line_label;
use crate::session::Session;
use crate::ui::{button, floating, key_label, link_button, palette, status_badge, u, BadgeKind};

gpui_kit::actions!(sheet, [CloseSheet, SubmitAll, ToggleMerge, VerdictComment, VerdictApprove, VerdictChanges, NextField, PrevField]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("SubmitSheet");
    cx.bind_keys([
        KeyBinding::new("escape", CloseSheet, ctx),
        KeyBinding::new("secondary-enter", SubmitAll, ctx),
        KeyBinding::new("secondary-shift-m", ToggleMerge, ctx),
        KeyBinding::new("secondary-1", VerdictComment, ctx),
        KeyBinding::new("secondary-2", VerdictApprove, ctx),
        KeyBinding::new("secondary-3", VerdictChanges, ctx),
        KeyBinding::new("tab", NextField, ctx),
        KeyBinding::new("shift-tab", PrevField, ctx),
    ]);
}

pub enum SheetEvent {
    Close,
    /// Something went out. `done`: everything did and nothing is left to send.
    Submitted { fresh: OpenedStack, done: bool, message: String },
}

#[derive(Clone)]
enum Result_ {
    Sending,
    Done(Outcome),
    Error(String),
}

#[derive(Clone)]
enum MergeResult {
    Sending,
    Done(MergeOutcome),
    Error(String),
}

/// Merging a PR along with its review: what GitHub says about it, and the commit message.
struct Merge {
    info: Option<MergeInfo>,
    error: Option<String>,
    method: MergeMethod,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
}

fn method_label(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Squash => "Squash and merge",
        MergeMethod::Merge => "Merge commit",
        MergeMethod::Rebase => "Rebase and merge",
    }
}

/// Why a PR can't be merged at all (a review can't change it), if so.
fn merge_blocked(info: &MergeInfo) -> Option<&'static str> {
    if !info.open {
        Some("This PR isn't open.")
    } else if info.draft {
        Some("Draft PRs can't be merged.")
    } else if info.methods.is_empty() {
        Some("The repository allows no merge method.")
    } else {
        None
    }
}

/// What submitting will do: merge now when green, otherwise enable auto-merge, and why.
fn merge_status(info: &MergeInfo) -> String {
    if info.merge_state == "CLEAN" || info.merge_state == "HAS_HOOKS" {
        return "Checks green — merges now".into();
    }
    let why = match (info.merge_state.as_str(), info.checks.as_str()) {
        ("DIRTY", _) => "has conflicts".to_string(),
        ("BEHIND", _) => "branch is behind its base".to_string(),
        (_, "PENDING" | "EXPECTED") => "checks running".to_string(),
        (_, "FAILURE" | "ERROR") | ("UNSTABLE", _) => "checks failing".to_string(),
        ("BLOCKED", _) => "waiting for required reviews or checks".to_string(),
        ("", _) => "state unknown".to_string(),
        (other, _) => other.to_lowercase(),
    };
    if info.auto_merge_enabled {
        format!("Auto-merge already enabled ({why}); submitting enables it again with this message")
    } else if !info.auto_merge_allowed {
        format!("Can't merge yet ({why}), and the repository doesn't allow auto-merge")
    } else {
        format!("Enables auto-merge — {why}")
    }
}

fn plan_label(plan: &Plan, draft: &Draft) -> String {
    let name = |p: &str| p.rsplit('/').next().unwrap_or_default().to_string();
    match plan {
        Plan::Line { path, line, start_line, .. } => format!("{} {}", name(path), line_label(*start_line, Some(*line))),
        Plan::File { path, quoted: true } => format!("{} · file comment (outside the diff)", name(path)),
        Plan::File { path, .. } => format!("{} · file", name(path)),
        Plan::Outdated => format!("⚠ outdated · {} {}", name(draft.path.as_deref().unwrap_or_default()), line_label(draft.start_line, draft.line)),
        Plan::Reply => "reply".into(),
        Plan::Resolve => "resolve".into(),
        Plan::Summary => "summary".into(),
    }
}

pub struct SubmitSheet {
    session: Entity<Session>,
    backend: Arc<Backend>,
    stack_id: String,
    shown: (usize, usize),
    plan: Option<SubmitPlan>,
    error: Option<String>,
    verdicts: HashMap<usize, Verdict>,
    summaries: HashMap<usize, Entity<TextareaState>>,
    results: HashMap<usize, Result_>,
    merges: HashMap<usize, Merge>,
    merge_results: HashMap<usize, MergeResult>,
    submitting: bool,
    /// The PR whose field has the focus (keys act on it).
    focused: Option<usize>,
    menus: crate::menu::MenuSlot,
    body_scroll: gpui_kit::ScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SheetEvent> for SubmitSheet {}

impl SubmitSheet {
    /// The current verdict for a PR, including the default before it is edited.
    pub fn verdict(&self, index: usize) -> Verdict {
        self.verdicts.get(&index).copied().unwrap_or(Verdict::Comment)
    }

    /// Whether a dropdown is currently taking keyboard input.
    pub fn menu_is_open(&self) -> bool {
        self.menus.is_open()
    }

    pub fn new(session: Entity<Session>, stack_id: String, shown: (usize, usize), window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = session.read(cx).backend.clone();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut sheet = SubmitSheet {
            session,
            backend,
            stack_id,
            shown,
            plan: None,
            error: None,
            verdicts: HashMap::new(),
            summaries: HashMap::new(),
            results: HashMap::new(),
            merges: HashMap::new(),
            merge_results: HashMap::new(),
            submitting: false,
            focused: None,
            menus: Default::default(),
            body_scroll: gpui_kit::ScrollHandle::new(),
            focus,
            _subscriptions: Vec::new(),
        };
        sheet.load(true, window, cx);
        sheet
    }

    fn load(&mut self, focus_first: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let task = self.backend.prepare_submit(self.stack_id.clone());
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(plan) => {
                            this.ensure_fields(&plan, window, cx);
                            this.plan = Some(plan);
                            if focus_first {
                                this.focus_first(window, cx);
                            }
                        }
                        Err(err) => this.error = Some(err),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn ensure_fields(&mut self, plan: &SubmitPlan, window: &mut Window, cx: &mut Context<Self>) {
        for index in 0..plan.stack.prs.len() {
            if self.summaries.contains_key(&index) {
                continue;
            }
            let summary = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true).auto_grow(2, 8).placeholder("Summary (optional)"));
            let sub = cx.subscribe_in(&summary, window, move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, secondary: false } => this.submit(vec![index], window, cx),
                InputEvent::Focus => this.focus_pr(index),
                InputEvent::Change => cx.notify(),
                _ => {}
            });
            self._subscriptions.push(sub);
            self.summaries.insert(index, summary);
        }
    }

    /// Keyboard first: start in the summary of the first on-screen PR with drafts (else the lowest).
    fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = &self.plan else { return };
        let (lo, hi) = self.shown;
        let with_drafts = (lo..=hi.min(plan.stack.prs.len() - 1)).find(|i| !self.planned_for(*i).is_empty());
        let index = with_drafts.unwrap_or(lo);
        if let Some(summary) = self.summaries.get(&index) {
            summary.update(cx, |s, cx| s.focus(window, cx));
            self.focus_pr(index);
        }
    }

    fn planned_for(&self, index: usize) -> &[Planned] {
        self.plan.as_ref().and_then(|p| p.prs.iter().find(|pr| pr.pr_index == index)).map_or(&[], |pr| pr.planned.as_slice())
    }

    /// GitHub lets the author of a PR only comment on it.
    fn own(&self, index: usize) -> bool {
        self.plan.as_ref().is_some_and(|p| p.stack.prs.get(index).is_some_and(|pr| pr.author == p.viewer))
    }

    fn blocked(&self, index: usize) -> bool {
        self.planned_for(index).iter().any(|p| matches!(p.plan, Plan::Outdated))
    }

    fn summary_text(&self, index: usize, cx: &App) -> String {
        self.summaries.get(&index).map(|s| s.read(cx).value().trim().to_string()).unwrap_or_default()
    }

    fn has_review_work(&self, index: usize, cx: &App) -> bool {
        !self.planned_for(index).is_empty() || self.verdicts.get(&index).copied().unwrap_or(Verdict::Comment) != Verdict::Comment || !self.summary_text(index, cx).is_empty()
    }

    /// Merging was asked for and can go ahead (a commit title is needed unless rebasing).
    fn wants_merge(&self, index: usize, cx: &App) -> bool {
        self.merges.get(&index).is_some_and(|m| {
            m.info.as_ref().is_some_and(|info| merge_blocked(info).is_none()) && (m.method == MergeMethod::Rebase || !m.title.read(cx).value().trim().is_empty())
        })
    }

    fn has_work(&self, index: usize, cx: &App) -> bool {
        self.has_review_work(index, cx) || self.wants_merge(index, cx)
    }

    /// A field of PR `index` got the focus: keys act on that PR, and its section scrolls into view.
    fn focus_pr(&mut self, index: usize) {
        self.focused = Some(index);
        self.body_scroll.scroll_to_item(index);
    }

    /// Turns merging a PR on (asking GitHub how it would merge) or off.
    fn toggle_merge(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.merges.remove(&index).is_some() {
            cx.notify();
            return;
        }
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Commit title"));
        let body = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true).auto_grow(2, 6).placeholder("Commit message (optional)"));
        self._subscriptions.push(cx.subscribe_in(&title, window, move |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift: false, secondary: false } => this.submit(vec![index], window, cx),
            InputEvent::Focus => this.focus_pr(index),
            _ => cx.notify(),
        }));
        self._subscriptions.push(cx.subscribe_in(&body, window, move |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift: false, secondary: false } => this.submit(vec![index], window, cx),
            InputEvent::Focus => this.focus_pr(index),
            _ => cx.notify(),
        }));
        self.merges.insert(index, Merge { info: None, error: None, method: MergeMethod::Squash, title, body });
        let task = self.backend.merge_info(self.stack_id.clone(), index);
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(merge) = this.merges.get_mut(&index) else { return };
                    match result {
                        Ok(info) => {
                            merge.method = info.methods.first().copied().unwrap_or(MergeMethod::Squash);
                            let (t, b) = (info.title.clone(), info.body.clone());
                            merge.title.update(cx, |s, cx| s.set_value(t, window, cx));
                            merge.body.update(cx, |s, cx| s.set_value(b, window, cx));
                            merge.info = Some(info);
                        }
                        Err(err) => merge.error = Some(err),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    /// Submits the review of each PR in `indices` that has something to send.
    fn submit(&mut self, indices: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = &self.plan else { return };
        if self.submitting {
            return;
        }
        self.submitting = true;
        let fresh = plan.stack.clone();
        // Work this submit leaves behind: other PRs' drafts, verdicts or summaries, or PRs held back.
        let mut done = !(0..fresh.prs.len()).any(|i| self.has_work(i, cx) && (!indices.contains(&i) || self.blocked(i)));
        struct Job {
            index: usize,
            number: u64,
            review: Option<(Verdict, Option<String>)>,
            merge: Option<MergeRequest>,
        }
        let jobs: Vec<Job> = indices
            .iter()
            .filter(|&&i| self.has_work(i, cx) && !self.blocked(i))
            .map(|&i| Job {
                index: i,
                number: fresh.prs[i].number,
                review: self.has_review_work(i, cx).then(|| {
                    let summary = self.summary_text(i, cx);
                    (self.verdicts.get(&i).copied().unwrap_or(Verdict::Comment), (!summary.is_empty()).then_some(summary))
                }),
                merge: self.wants_merge(i, cx).then(|| {
                    let m = &self.merges[&i];
                    MergeRequest { method: m.method, title: m.title.read(cx).value().to_string(), body: m.body.read(cx).value().to_string() }
                }),
            })
            .collect();
        let backend = self.backend.clone();
        let stack_id = fresh.stack_id.clone();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let mut said: Vec<String> = Vec::new();
            for job in jobs {
                if let Some((verdict, summary)) = job.review {
                    let _ = this.update(cx, |this, cx| {
                        this.results.insert(job.index, Result_::Sending);
                        cx.notify();
                    });
                    match backend.submit_review(stack_id.clone(), job.index, verdict, summary).await {
                        Ok(outcome) => {
                            let incomplete = !outcome.failed.is_empty() || outcome.unknown > 0;
                            let _ = cx.update_window(handle, |_, window, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.results.insert(job.index, Result_::Done(outcome));
                                    // Sent: the verdict and summary don't go out again with a later submit.
                                    this.verdicts.remove(&job.index);
                                    if let Some(s) = this.summaries.get(&job.index) {
                                        s.update(cx, |s, cx| s.set_value(String::new(), window, cx));
                                    }
                                    cx.notify();
                                });
                            });
                            if !said.iter().any(|s| s == "Review submitted") {
                                said.insert(0, "Review submitted".into());
                            }
                            if incomplete {
                                // Not merged on top of a review that didn't fully go out.
                                done = false;
                                continue;
                            }
                        }
                        Err(err) => {
                            let _ = this.update(cx, |this, cx| {
                                this.results.insert(job.index, Result_::Error(err));
                                cx.notify();
                            });
                            done = false;
                            continue;
                        }
                    }
                }
                let Some(request) = job.merge else { continue };
                let _ = this.update(cx, |this, cx| {
                    this.merge_results.insert(job.index, MergeResult::Sending);
                    cx.notify();
                });
                match backend.merge_pr(stack_id.clone(), job.index, request).await {
                    Ok(outcome) => {
                        let _ = this.update(cx, |this, cx| {
                            this.merge_results.insert(job.index, MergeResult::Done(outcome));
                            this.merges.remove(&job.index);
                            cx.notify();
                        });
                        said.push(match outcome {
                            MergeOutcome::Merged => format!("#{} merged", job.number),
                            MergeOutcome::AutoMergeEnabled => format!("Auto-merge enabled for #{}", job.number),
                        });
                    }
                    Err(err) => {
                        let _ = this.update(cx, |this, cx| {
                            this.merge_results.insert(job.index, MergeResult::Error(err));
                            cx.notify();
                        });
                        done = false;
                    }
                }
            }
            let message = if said.is_empty() { "Review submitted".to_string() } else { said.join(" · ") };
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.submitting = false;
                    cx.emit(SheetEvent::Submitted { fresh: fresh.clone(), done, message });
                    if !done {
                        // What's left to send for the other PRs (posted drafts drop out).
                        this.load(false, window, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn submit_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = &self.plan else { return };
        let all = (0..plan.stack.prs.len()).collect();
        self.submit(all, window, cx);
    }

    fn set_verdict(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        if self.plan.is_none() || self.submitting {
            return;
        }
        let index = self.focused.or_else(|| self.plan.as_ref().and_then(|p| p.prs.first().map(|pr| pr.pr_index))).unwrap_or(0);
        if verdict == Verdict::Comment || !self.own(index) {
            self.verdicts.insert(index, verdict);
            cx.notify();
        }
    }

    pub fn close_from_backdrop(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.submitting {
            cx.emit(SheetEvent::Close);
        }
    }

    /// The fields Tab moves through: every PR's summary and merge fields, in order.
    fn fields(&self, cx: &App) -> Vec<FocusHandle> {
        let mut out = Vec::new();
        let count = self.plan.as_ref().map_or(0, |p| p.stack.prs.len());
        for i in 0..count {
            if let Some(s) = self.summaries.get(&i) {
                out.push(s.focus_handle(cx));
            }
            if let Some(m) = self.merges.get(&i) {
                if m.info.as_ref().is_some_and(|info| merge_blocked(info).is_none()) && m.method != MergeMethod::Rebase {
                    out.push(m.title.focus_handle(cx));
                    out.push(m.body.focus_handle(cx));
                }
            }
        }
        out
    }

    fn move_field(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(cx);
        if fields.is_empty() {
            return;
        }
        let at = fields.iter().position(|f| f.contains_focused(window, cx)).unwrap_or(fields.len() - 1);
        let next = if back { (at + fields.len() - 1) % fields.len() } else { (at + 1) % fields.len() };
        window.focus(&fields[next], cx);
    }

    fn resolve_outdated(&mut self, id: String, as_file: bool, window: &mut Window, cx: &mut Context<Self>) {
        let task = if as_file { self.backend.update_draft(id, None, Some(true)) } else {
            let delete = self.backend.delete_draft(id);
            let handle = window.window_handle();
            cx.spawn(async move |this, cx| {
                let _ = delete.await;
                let _ = cx.update_window(handle, |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.load(false, window, cx));
                });
            })
            .detach();
            return;
        };
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.load(false, window, cx));
            });
        })
        .detach();
    }

    /// Images pasted into a summary go in as Markdown (uploaded when submitting).
    fn paste_into_summary(&mut self, index: usize, item: &ClipboardItem, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let images: Vec<_> = item
            .entries()
            .iter()
            .filter_map(|e| match e {
                ClipboardEntry::Image(i) => Some((crate::diff::inserts::image_type(i.format), i.bytes.clone())),
                _ => None,
            })
            .collect();
        if images.is_empty() {
            return false;
        }
        let tasks: Vec<_> = images.into_iter().map(|(t, b)| self.backend.save_image_data(t, b)).collect();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let mut md = Vec::new();
            for t in tasks {
                match t.await {
                    Ok(m) => md.push(m),
                    Err(e) => {
                        let _ = this.update(cx, |this, cx| {
                            this.results.insert(index, Result_::Error(format!("Couldn't paste the image: {e}")));
                            cx.notify();
                        });
                    }
                }
            }
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if let Some(s) = this.summaries.get(&index) {
                        let text = md.join("\n");
                        s.update(cx, |s, cx| s.insert(text, window, cx));
                    }
                });
            });
        })
        .detach();
        true
    }

    fn render_pr(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let plan = self.plan.as_ref().unwrap();
        let pr = plan.stack.prs[index].clone();
        let planned = self.planned_for(index).to_vec();
        let own = self.own(index);
        let verdict = self.verdicts.get(&index).copied().unwrap_or(Verdict::Comment);
        let verdict_label = match verdict {
            Verdict::Comment => "Comment",
            Verdict::Approve => "Approve",
            Verdict::RequestChanges => "Request changes",
        };
        let can_submit = !self.submitting && self.has_work(index, cx) && !self.blocked(index);
        let verdict_menu = self.menus.get(&format!("verdict-{index}")).filter(|_| !own && !self.submitting);
        let merge_on = self.merges.contains_key(&index);
        let summary = self.summaries.get(&index).cloned();
        let weak = cx.entity().downgrade();
        let verdict_tip = if own {
            "Your own PR: GitHub only lets you comment".to_string()
        } else {
            format!("{} Comment · {} Approve · {} Request changes", key_label("1"), key_label("2"), key_label("3"))
        };
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .p(u(12.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_size(u(13.))
                    .child(div().text_color(p.fg_muted).child(format!("#{}", pr.number)))
                    .child(div().flex_1().min_w(px(0.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(pr.title.clone()))
                    .child(div().text_size(u(12.)).text_color(p.fg_muted).child(format!("{} item{}", planned.len(), if planned.len() == 1 { "" } else { "s" })))
                    .child(
                        div()
                            .id(("verdict", index))
                            .relative()
                            .px(u(8.))
                            .py(u(3.))
                            .rounded(u(4.))
                            .border_1()
                            .border_color(p.border)
                            .text_size(u(12.))
                            .when(own || self.submitting, |d| d.opacity(0.6))
                            .when(!own && !self.submitting, |d| d.cursor_pointer())
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(verdict_tip.clone()).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if own || this.submitting {
                                    return;
                                }
                                let verdicts = [Verdict::Comment, Verdict::Approve, Verdict::RequestChanges];
                                let selected = verdicts.iter().position(|v| *v == verdict);
                                this.menus.toggle(
                                    format!("verdict-{index}"),
                                    vec!["Comment".into(), "Approve".into(), "Request changes".into()],
                                    selected,
                                    |this: &mut Self| &mut this.menus,
                                    move |this, i, _, _| {
                                        this.verdicts.insert(index, verdicts[i]);
                                    },
                                    window,
                                    cx,
                                );
                                cx.notify();
                            }))
                            .child(format!("{verdict_label} ▾"))
                            .when_some(verdict_menu, |d, menu| d.child(deferred(anchored().child(div().mt(u(22.)).child(menu))))),
                    )
                    .child(
                        div()
                            .id(("merge-toggle", index))
                            .flex()
                            .items_center()
                            .gap(u(4.))
                            .text_size(u(12.))
                            .map(|d| if self.submitting { d.opacity(0.6) } else { d.cursor_pointer() })
                            .tooltip({
                                let tip = format!("Merge #{} after submitting ({})", pr.number, key_label("⇧M"));
                                move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.submitting {
                                    this.toggle_merge(index, window, cx)
                                }
                            }))
                            .child(div().size(u(12.)).rounded(u(3.)).border_1().border_color(p.fg_muted).when(merge_on, |d| d.bg(p.accent)))
                            .child("Merge"),
                    )
                    .child(
                        button(format!("Submit #{}", pr.number), false, cx)
                            .id(("submit-pr", index))
                            .when(!can_submit, |d| d.opacity(0.5).cursor_default())
                            .tooltip({
                                let tip = format!("Submit only #{} (↵ in its summary)", pr.number);
                                move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if can_submit {
                                    this.submit(vec![index], window, cx)
                                }
                            })),
                    ),
            )
            .children(summary.map(|s| {
                Textarea::new(&s).disabled(self.submitting).on_paste(move |item, window, cx| weak.update(cx, |this, cx| this.paste_into_summary(index, item, window, cx)).unwrap_or(false))
            }))
            .children(self.render_merge(index, pr.number, cx))
            .child(div().flex().flex_col().gap(u(4.)).children(planned.iter().map(|planned| {
                let draft = planned.draft.clone();
                let outdated = matches!(planned.plan, Plan::Outdated);
                let body = if draft.kind == DraftKind::Resolve { "resolve thread".to_string() } else { draft.body.clone() };
                let (file_id, discard_id) = (draft.id.clone(), draft.id.clone());
                div()
                    .flex()
                    .items_start()
                    .gap(u(8.))
                    .text_size(u(12.))
                    .child(div().flex_none().w(u(180.)).text_color(if outdated { p.pr(1) } else { p.fg_muted }).child(plan_label(&planned.plan, &draft)))
                    .child(div().flex_1().min_w(px(0.)).max_h(u(60.)).overflow_hidden().child(body))
                    .when(outdated, |d| {
                        d.child(button("Post as file comment", false, cx).id(SharedString::from(format!("as-file-{file_id}"))).on_click(cx.listener(
                            move |this, _, window, cx| this.resolve_outdated(file_id.clone(), true, window, cx),
                        )))
                        .child(button("Discard", false, cx).id(SharedString::from(format!("discard-{discard_id}"))).on_click(cx.listener(
                            move |this, _, window, cx| this.resolve_outdated(discard_id.clone(), false, window, cx),
                        )))
                    })
                    .when(draft.status == DraftStatus::Failed, |d| d.child(div().text_color(p.del_fg).child(draft.error.clone().unwrap_or_default())))
            })))
            .when(self.blocked(index), |d| d.child(note("Decide on the outdated comments before this PR can be submitted.", &p)))
            .children(self.results.get(&index).map(|r| match r {
                Result_::Sending => note("Sending…", &p),
                Result_::Error(m) => div().text_size(u(12.)).text_color(p.del_fg).child(m.clone()).into_any_element(),
                Result_::Done(outcome) => {
                    if !outcome.failed.is_empty() {
                        let reasons: Vec<String> = outcome.failed.iter().map(|f| f.1.clone()).collect();
                        div().text_size(u(12.)).text_color(p.del_fg).child(format!("{} rejected: {}", outcome.failed.len(), reasons.join("; "))).into_any_element()
                    } else if outcome.unknown > 0 {
                        div().text_size(u(12.)).text_color(p.add_fg).child(format!("Sent; {} not confirmed yet — checked before the next submit", outcome.unknown)).into_any_element()
                    } else {
                        div().text_size(u(12.)).text_color(p.add_fg).child("Posted ✓").into_any_element()
                    }
                }
            }))
            .children(self.merge_results.get(&index).map(|r| match r {
                MergeResult::Sending => note("Merging…", &p),
                MergeResult::Error(m) => div().text_size(u(12.)).text_color(p.del_fg).child(format!("Not merged: {m}")).into_any_element(),
                MergeResult::Done(MergeOutcome::Merged) => div().text_size(u(12.)).text_color(p.add_fg).child("Merged ✓").into_any_element(),
                MergeResult::Done(MergeOutcome::AutoMergeEnabled) => {
                    div().text_size(u(12.)).text_color(p.add_fg).child("Auto-merge enabled ✓ — merges once it's green").into_any_element()
                }
            }))
            .into_any_element()
    }

    /// Where and how the PR merges, with its commit message to edit.
    fn render_merge(&mut self, index: usize, number: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx).clone();
        let merge = self.merges.get(&index)?;
        if let Some(error) = &merge.error {
            return Some(div().text_size(u(12.)).text_color(p.del_fg).child(format!("Can't check how #{number} merges: {error}")).into_any_element());
        }
        let Some(info) = merge.info.clone() else { return Some(note(&format!("Checking how #{number} merges…"), &p)) };
        let blocked = merge_blocked(&info);
        let elsewhere = info.base_ref != info.default_branch;
        let method = merge.method;
        let (title, body) = (merge.title.clone(), merge.body.clone());
        let method_menu = self.menus.get(&format!("method-{index}")).filter(|_| !self.submitting);
        let status = blocked.map(String::from).unwrap_or_else(|| merge_status(&info));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(u(6.))
                .p(u(8.))
                .rounded(u(6.))
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(u(6.))
                        .text_size(u(12.))
                        .child("Merge into")
                        .child(div().font_family(crate::ui::mono_family(cx)).text_color(if elsewhere { p.pr(1) } else { p.fg }).child(info.base_ref.clone()))
                        .when(elsewhere, |d| d.child(status_badge(format!("not {}", info.default_branch), BadgeKind::Warn, cx)))
                        .when(blocked.is_none(), |d| {
                            d.child(
                                div()
                                    .id(("method", index))
                                    .relative()
                                    .px(u(8.))
                                    .py(u(2.))
                                    .rounded(u(4.))
                                    .border_1()
                                    .border_color(p.border)
                                    .map(|d| if self.submitting { d.opacity(0.6) } else { d.cursor_pointer() })
                                    .child(format!("{} ▾", method_label(method)))
                                    .on_click({
                                        let methods = info.methods.clone();
                                        cx.listener(move |this, _, window, cx| {
                                            if this.submitting {
                                                return;
                                            }
                                            let methods = methods.clone();
                                            let labels = methods.iter().map(|m| SharedString::from(method_label(*m))).collect();
                                            let selected = methods.iter().position(|m| *m == method);
                                            this.menus.toggle(
                                                format!("method-{index}"),
                                                labels,
                                                selected,
                                                |this: &mut Self| &mut this.menus,
                                                move |this, i, _, _| {
                                                    if let (Some(merge), Some(m)) = (this.merges.get_mut(&index), methods.get(i)) {
                                                        merge.method = *m;
                                                    }
                                                },
                                                window,
                                                cx,
                                            );
                                            cx.notify();
                                        })
                                    })
                                    .when_some(method_menu, |d, menu| d.child(deferred(anchored().child(div().mt(u(20.)).child(menu))))),
                            )
                        })
                        .child(div().text_color(p.fg_muted).child(status)),
                )
                .when(blocked.is_none() && method != MergeMethod::Rebase, |d| d.child(Input::new(&title).disabled(self.submitting)).child(Textarea::new(&body).disabled(self.submitting)))
                .into_any_element(),
        )
    }
}

fn note(text: &str, p: &crate::theme::Palette) -> AnyElement {
    div().text_size(u(12.)).text_color(p.fg_muted).child(text.to_string()).into_any_element()
}

impl Focusable for SubmitSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SubmitSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let count = self.plan.as_ref().map_or(0, |pl| pl.stack.prs.len());
        let prs: Vec<AnyElement> = (0..count).map(|i| self.render_pr(i, cx)).collect();
        let _ = &self.session;
        floating(820., cx)
            .max_h(gpui_kit::relative(0.76))
            .key_context("SubmitSheet")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseSheet, _, cx| {
                if !this.submitting {
                    cx.emit(SheetEvent::Close)
                }
            }))
            .on_action(cx.listener(|this, _: &SubmitAll, window, cx| this.submit_all(window, cx)))
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, _, cx| {
                cx.stop_propagation();
                if !this.submitting {
                    cx.emit(SheetEvent::Close)
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleMerge, window, cx| {
                if this.plan.is_some() && !this.submitting {
                    let index = this.focused.unwrap_or(this.shown.0);
                    this.toggle_merge(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &VerdictComment, _, cx| this.set_verdict(Verdict::Comment, cx)))
            .on_action(cx.listener(|this, _: &VerdictApprove, _, cx| this.set_verdict(Verdict::Approve, cx)))
            .on_action(cx.listener(|this, _: &VerdictChanges, _, cx| this.set_verdict(Verdict::RequestChanges, cx)))
            .on_action(cx.listener(|this, _: &NextField, window, cx| this.move_field(false, window, cx)))
            .on_action(cx.listener(|this, _: &PrevField, window, cx| this.move_field(true, window, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                cx.stop_propagation();
                this.move_field(false, window, cx)
            }))
            .capture_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                cx.stop_propagation();
                this.move_field(true, window, cx)
            }))
            // ⌘Enter submits all from any field (the text boxes take Enter themselves).
            .capture_action(cx.listener(|this, action: &gpui_kit::component::input::Enter, window, cx| {
                if action.secondary {
                    cx.stop_propagation();
                    this.submit_all(window, cx);
                }
            }))
            .font_family(crate::ui::sans_family(cx))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(u(16.))
                    .py(u(10.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().text_size(u(14.)).font_weight(FontWeight::SEMIBOLD).child("Submit review"))
                    .child(link_button("close", cx).id("sheet-close").on_click(cx.listener(|this, _, _, cx| {
                        if !this.submitting {
                            cx.emit(SheetEvent::Close)
                        }
                    }))),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(10.))
                        .p(u(12.))
                        .text_size(u(12.))
                        .text_color(p.del_fg)
                        .child(format!("Can't reach GitHub to prepare the review: {error}. Your drafts are safe locally."))
                        .child(button("Retry", false, cx).id("sheet-retry").on_click(cx.listener(|this, _, window, cx| this.load(true, window, cx)))),
                )
            })
            .when(self.plan.is_none() && self.error.is_none(), |d| d.child(div().p(u(16.)).text_size(u(13.)).text_color(p.fg_muted).child("Checking the stack on GitHub…")))
            .when(self.plan.is_some(), |d| {
                d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .child(div().id("sheet-body").flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&self.body_scroll).children(prs))
                        .child(gpui_kit::base::Scrollbar::vertical(&self.body_scroll)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(10.))
                        .px(u(16.))
                        .py(u(10.))
                        .border_t_1()
                        .border_color(p.border)
                        .child(div().flex_1().min_w(px(0.)).whitespace_normal().text_size(u(11.)).text_color(p.fg_muted).child(format!(
                            "{} comment · {} approve · {} request changes · {} merge · ⇥ next field · ↵ submit PR · {} all",
                            key_label("1"),
                            key_label("2"),
                            key_label("3"),
                            key_label("⇧M"),
                            key_label("↵")
                        )))
                        .child(button("Submit all", true, cx).id("submit-all").flex_none().when(self.submitting, |d| d.opacity(0.6).cursor_default()).tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(format!("Submit all ({})", key_label("↵"))).build(window, cx)).on_click(cx.listener(|this, _, window, cx| this.submit_all(window, cx)))),
                )
            })
    }
}
