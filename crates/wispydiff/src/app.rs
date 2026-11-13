//! The window's root (was `App.tsx`): the title bar holding the header, the stack bar, the inbox or
//! the diff, and the overlays (palette, theme picker, shortcuts, newer-changes warning, submit
//! sheet), the toast, and the keys that work everywhere.

use std::time::Duration;

use gpui_kit::component::TitleBar;
use gpui_kit::prelude::*;
use gpui_kit::{
    anchored, canvas, deferred, div, px, App, Context, DispatchPhase, Entity, FocusHandle, Focusable, FontWeight,
    KeyBinding, Render, ScrollDelta, ScrollWheelEvent, SharedString, Subscription, Task, Window,
};

use crate::diff::DiffViewer;
use crate::inbox::{Inbox, InboxEvent};
use crate::overlays::{NewerChangesWarning, OverlayEvent, ShortcutsHelp, ThemePicker};
use crate::palette::{Command, CommandPalette, PaletteEvent};
use crate::session::{pr_label, BaseMode, Later, Session, SessionEvent};
use crate::settings;
use crate::submit_sheet::{SheetEvent, SubmitSheet};
use crate::theme::ThemeId;
use crate::ui::{button, logo_mark, palette, u, Looks};

gpui_kit::actions!(
    wispy,
    [
        OpenPalette,
        OpenThemes,
        ShowShortcuts,
        GoInbox,
        ToggleFiles,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleWhitespace,
        MarkReviewed,
        OpenSubmit,
        ToggleSince,
        NextPr,
        PrevPr,
        RangeDown,
        RangeUp,
        ExtendDown,
        ExtendUp,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let root = Some("Wispy");
    let typing_ok = Some("Wispy && !Input");
    let stack = Some("Stack && !Input");
    cx.bind_keys([
        KeyBinding::new("secondary-k", OpenPalette, root),
        KeyBinding::new("secondary-t", OpenThemes, root),
        KeyBinding::new("secondary-/", ShowShortcuts, root),
        KeyBinding::new("?", ShowShortcuts, typing_ok),
        KeyBinding::new("shift-/", ShowShortcuts, typing_ok),
        KeyBinding::new("secondary-i", GoInbox, root),
        KeyBinding::new("secondary-b", ToggleFiles, root),
        KeyBinding::new("secondary-=", ZoomIn, root),
        KeyBinding::new("secondary-+", ZoomIn, root),
        KeyBinding::new("secondary-shift-=", ZoomIn, root),
        KeyBinding::new("secondary--", ZoomOut, root),
        KeyBinding::new("secondary-_", ZoomOut, root),
        KeyBinding::new("secondary-0", ZoomReset, root),
        KeyBinding::new("w", ToggleWhitespace, stack),
        KeyBinding::new("shift-m", MarkReviewed, stack),
        KeyBinding::new("r", OpenSubmit, stack),
        KeyBinding::new("d", ToggleSince, stack),
        KeyBinding::new("tab", NextPr, stack),
        KeyBinding::new("shift-tab", PrevPr, stack),
        KeyBinding::new("[", RangeDown, stack),
        KeyBinding::new("]", RangeUp, stack),
        KeyBinding::new("{", ExtendDown, stack),
        KeyBinding::new("}", ExtendUp, stack),
        KeyBinding::new("shift-[", ExtendDown, stack),
        KeyBinding::new("shift-]", ExtendUp, stack),
    ]);
}

/// Zoom steps, like a browser's.
const LEVELS: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
/// Wheel distance (px) per zoom step, so a trackpad doesn't race through the levels.
const WHEEL_STEP: f32 = 50.;

pub fn saved_zoom(cx: &App) -> f32 {
    let allowed: Vec<String> = LEVELS.iter().map(|l| l.to_string()).collect();
    let allowed: Vec<&str> = allowed.iter().map(String::as_str).collect();
    settings::pref(cx, "zoom", &allowed, "1").parse().unwrap_or(1.0)
}

enum Overlay {
    Theme(Entity<ThemePicker>),
    Shortcuts(Entity<ShortcutsHelp>),
    Newer(Entity<NewerChangesWarning>, Later),
}

pub struct WispyApp {
    pub session: Entity<Session>,
    inbox: Entity<Inbox>,
    pub diff: Option<Entity<DiffViewer>>,
    /// The stack the diff viewer shows (a new stack gets a new viewer, as React keyed it).
    diff_stack: Option<String>,
    palette: Option<Entity<CommandPalette>>,
    overlay: Option<Overlay>,
    pub sheet: Option<Entity<SubmitSheet>>,
    toast: Option<(SharedString, Task<()>)>,
    menus: crate::menu::MenuSlot,
    was_home: bool,
    wheel_zoom: f32,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl WispyApp {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inbox = cx.new(Inbox::new);
        let subscriptions = vec![
            cx.observe_in(&session, window, |this, session, window, cx| this.session_changed(session, window, cx)),
            cx.subscribe_in(&session, window, |this, _, event: &SessionEvent, window, cx| match event {
                SessionEvent::Toast(message) => this.say(message.clone(), cx),
                SessionEvent::Opened => {
                    this.palette = None;
                    this.focus_content(window, cx);
                }
                SessionEvent::ViewedWarning { path, mark } => {
                    let warning = cx.new(|cx| NewerChangesWarning::new(path.clone(), window, cx));
                    this.subscribe_overlay(&warning, window, cx);
                    this.overlay = Some(Overlay::Newer(warning, mark.clone()));
                    cx.notify();
                }
            }),
            cx.subscribe_in(&inbox, window, |this, _, event: &InboxEvent, _, cx| match event {
                InboxEvent::Open(label) => this.session.update(cx, |s, cx| s.open(label.clone(), cx)),
                InboxEvent::Refresh => this.session.update(cx, |s, _| s.refresh_inbox()),
            }),
            // Every focus of the window refreshes the inbox and checks the open PR.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.session.update(cx, |s, _| s.refresh_inbox());
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                let dark = matches!(window.appearance(), gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark);
                Looks::set_system_dark(dark, cx);
            }),
        ];
        let focus = cx.focus_handle();
        let app = WispyApp {
            session,
            inbox,
            diff: None,
            diff_stack: None,
            palette: None,
            overlay: None,
            sheet: None,
            toast: None,
            menus: Default::default(),
            was_home: true,
            wheel_zoom: 0.,
            focus,
            _subscriptions: subscriptions,
        };
        app.focus_content(window, cx);
        app
    }

    fn session_changed(&mut self, session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) {
        let s = session.read(cx);
        let stack_id = s.stack.as_ref().map(|st| st.stack_id.clone());
        let (entries, online, busy, error, home) = (s.inbox.clone(), s.online, s.busy.clone(), s.error.clone(), s.home);
        self.inbox.update(cx, |inbox, cx| {
            inbox.set_entries(entries, cx);
            inbox.set_online(online, cx);
        });
        if let Some(palette) = &self.palette {
            palette.update(cx, |p, cx| {
                p.busy = busy;
                p.error = error;
                cx.notify();
            });
        }
        // A new stack gets a new viewer, and so does coming back from the inbox (as the React
        // app remounted it): per-file choices start over.
        let back_from_home = self.was_home && !home;
        self.was_home = home;
        if stack_id != self.diff_stack || (back_from_home && stack_id.is_some()) {
            self.diff_stack = stack_id.clone();
            self.diff = stack_id.map(|_| {
                let session = self.session.clone();
                cx.new(|cx| DiffViewer::new(session, window, cx))
            });
            if self.palette.is_none() && self.overlay.is_none() {
                self.focus_content(window, cx);
            }
        }
        cx.notify();
    }

    /// The inbox or the diff takes the keyboard.
    fn focus_content(&self, window: &mut Window, cx: &mut App) {
        let home = self.session.read(cx).home;
        match (&self.diff, home) {
            (Some(diff), false) => window.focus(&diff.focus_handle(cx), cx),
            _ => window.focus(&self.inbox.focus_handle(cx), cx),
        }
    }

    fn say(&mut self, message: String, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(3500)).await;
            let _ = this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            });
        });
        self.toast = Some((message.into(), task));
        cx.notify();
    }

    fn subscribe_overlay<T: gpui_kit::EventEmitter<OverlayEvent>>(&mut self, entity: &Entity<T>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(entity, window, |this, _, event: &OverlayEvent, window, cx| {
            match event {
                OverlayEvent::Closed(theme) => {
                    if let Some(theme) = theme {
                        settings::set_pref(cx, "theme", theme.id());
                    }
                    this.overlay = None;
                }
                OverlayEvent::LoadNewer => {
                    this.overlay = None;
                    this.session.update(cx, |s, cx| s.load_newer(cx));
                }
                OverlayEvent::MarkAnyway => {
                    if let Some(Overlay::Newer(_, mark)) = this.overlay.take() {
                        this.session.update(cx, |s, _| s.dismiss_newer_warnings());
                        mark(window, cx);
                    }
                }
            }
            this.focus_content(window, cx);
            cx.notify();
        })
        .detach();
    }

    // ---------- for tests ----------

    pub fn palette_view(&self) -> Option<&Entity<CommandPalette>> {
        self.palette.as_ref()
    }

    /// "theme", "shortcuts" or "newer" while that overlay is open.
    pub fn overlay_kind(&self) -> Option<&'static str> {
        match &self.overlay {
            Some(Overlay::Theme(_)) => Some("theme"),
            Some(Overlay::Shortcuts(_)) => Some("shortcuts"),
            Some(Overlay::Newer(..)) => Some("newer"),
            None => None,
        }
    }

    pub fn toast_text(&self) -> Option<String> {
        self.toast.as_ref().map(|t| t.0.to_string())
    }

    pub fn inbox_view(&self) -> &Entity<Inbox> {
        &self.inbox
    }

    // ---------- actions ----------

    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, _| s.error = None);
        if let Some(Overlay::Theme(picker)) = self.overlay.take() {
            picker.update(cx, |p, cx| p.cancel(cx));
        }
        self.overlay = None;
        if let Some(palette) = &self.palette {
            palette.update(cx, |p, cx| p.focus_input(window, cx));
            return;
        }
        let commands = self.commands(cx);
        let recents = settings::recent_prs(cx);
        // What it can suggest at once: the inbox's PRs and the open stack's.
        let s = self.session.read(cx);
        let mut known: Vec<crate::palette::Suggestion> = s
            .inbox
            .iter()
            .flat_map(|e| &e.group.prs)
            .map(|pr| crate::palette::Suggestion { label: format!("{}#{}", pr.repo, pr.number), title: pr.title.clone(), source: "inbox" })
            .collect();
        if let Some(stack) = &s.stack {
            known.extend(stack.prs.iter().map(|pr| crate::palette::Suggestion { label: pr_label(pr), title: pr.title.clone(), source: "stack" }));
        }
        let backend = s.backend.clone();
        let palette = cx.new(|cx| CommandPalette::new(commands, recents, window, cx).with_suggestions(known, Some(backend)));
        cx.subscribe_in(&palette, window, |this, _, event: &PaletteEvent, window, cx| match event {
            PaletteEvent::Open(input) => this.session.update(cx, |s, cx| s.open(input.clone(), cx)),
            PaletteEvent::Close => {
                this.palette = None;
                this.focus_content(window, cx);
                cx.notify();
            }
        })
        .detach();
        self.palette = Some(palette);
        cx.notify();
    }

    fn open_themes(&mut self, _: &OpenThemes, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Theme(_)) = &self.overlay {
            // ⌘T again: the picker stays as it is.
            return;
        }
        self.palette = None;
        let picker = cx.new(|cx| ThemePicker::new(window, cx));
        self.subscribe_overlay(&picker, window, cx);
        self.overlay = Some(Overlay::Theme(picker));
        cx.notify();
    }

    fn show_shortcuts(&mut self, _: &ShowShortcuts, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Some(Overlay::Shortcuts(_))) {
            self.overlay = None;
            self.focus_content(window, cx);
            cx.notify();
            return;
        }
        self.palette = None;
        let help = cx.new(|cx| ShortcutsHelp::new(window, cx));
        self.subscribe_overlay(&help, window, cx);
        self.overlay = Some(Overlay::Shortcuts(help));
        cx.notify();
    }

    fn blocked(&self) -> bool {
        self.overlay.is_some()
    }

    fn go_inbox(&mut self, _: &GoInbox, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked() {
            return;
        }
        self.session.update(cx, |s, cx| s.go_home(cx));
        self.palette = None;
        self.focus_content(window, cx);
    }

    fn toggle_files(&mut self, _: &ToggleFiles, _: &mut Window, cx: &mut Context<Self>) {
        if self.blocked() {
            return;
        }
        self.session.update(cx, |s, cx| s.toggle_files(cx));
    }

    fn zoom(&mut self, step: i32, cx: &mut Context<Self>) {
        let level = crate::ui::zoom(cx);
        let index = LEVELS.iter().position(|l| (l - level).abs() < 0.001).unwrap_or(5) as i32;
        let next = if step == 0 { 1.0 } else { LEVELS[(index + step).clamp(0, LEVELS.len() as i32 - 1) as usize] };
        Looks::set_zoom(next, cx);
        settings::set_pref(cx, "zoom", &next.to_string());
        self.say(format!("Zoom {}%", (next * 100.).round()), cx);
    }

    fn on_wheel_capture(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) -> bool {
        if !event.modifiers.secondary() {
            return false;
        }
        let dy = match event.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * WHEEL_STEP,
        };
        self.wheel_zoom += dy;
        if self.wheel_zoom.abs() >= WHEEL_STEP {
            let step = if self.wheel_zoom > 0. { 1 } else { -1 };
            self.wheel_zoom = 0.;
            self.zoom(step, cx);
        }
        true
    }

    fn stack_action(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Session, &mut Context<Session>)) {
        if self.blocked() || self.palette.is_some() || self.sheet.is_some() {
            return;
        }
        if !self.session.read(cx).showing_diff() {
            return;
        }
        self.session.update(cx, f);
    }

    fn open_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.session.read(cx);
        let Some(stack) = &s.stack else { return };
        let shown = s.shown.as_ref().map(|r| (r.range.lo, r.range.hi)).unwrap_or((0, stack.prs.len() - 1));
        let (id, session) = (stack.stack_id.clone(), self.session.clone());
        let sheet = cx.new(|cx| SubmitSheet::new(session, id, shown, window, cx));
        cx.subscribe_in(&sheet, window, |this, _, event: &SheetEvent, window, cx| match event {
            SheetEvent::Close => {
                this.sheet = None;
                this.focus_content(window, cx);
                cx.notify();
            }
            SheetEvent::Submitted { fresh, done, message } => {
                if *done {
                    this.sheet = None;
                    this.focus_content(window, cx);
                }
                let (fresh, done, message) = (fresh.clone(), *done, message.clone());
                this.session.update(cx, |s, cx| s.after_submit(fresh, done, message, cx));
                cx.notify();
            }
        })
        .detach();
        self.palette = None;
        self.sheet = Some(sheet);
        cx.notify();
    }

    /// The palette's `>` commands.
    fn commands(&self, cx: &App) -> Vec<Command> {
        let s = self.session.read(cx);
        let session = self.session.clone();
        let mut list = Vec::new();
        let with = |f: fn(&mut Session, &mut Context<Session>)| {
            let session = session.clone();
            move |_: &str, _: &mut Window, cx: &mut App| session.update(cx, f)
        };
        if let Some(shown) = &s.shown {
            let label = if shown.ignore_whitespace { "Show whitespace changes" } else { "Hide whitespace changes" };
            list.push(Command::new("whitespace", label, with(|s, cx| s.toggle_whitespace(cx))));
        }
        list.push(Command::new("inbox", "Go to inbox", with(|s, cx| s.go_home(cx))));
        list.push(Command::new("theme", "Change theme…", |_, window, cx| window.dispatch_action(Box::new(OpenThemes), cx)));
        list.push(Command::new("shortcuts", "Keyboard shortcuts", |_, window, cx| window.dispatch_action(Box::new(ShowShortcuts), cx)));
        if s.stack.is_some() {
            list.push(Command::new("submit", "Submit review…", |_, window, cx| window.dispatch_action(Box::new(OpenSubmit), cx)));
        }
        if let Some(shown) = &s.shown {
            list.push(Command::new("reviewed", "Mark as reviewed (checkpoint)", with(|s, cx| s.mark_reviewed(cx))));
            let label = if shown.since.is_some() { "Show the full diff" } else { "Show changes since last review" };
            list.push(Command::new("since", label, with(|s, cx| s.toggle_since(cx))));
        }
        let split = s.default_mode == BaseMode::Split;
        list.push(Command::new(
            "mode",
            if split { "Show all files unified" } else { "Show all files side by side" },
            with(if split { |s, cx| s.set_default_mode(BaseMode::Unified, cx) } else { |s, cx| s.set_default_mode(BaseMode::Split, cx) }),
        ));
        let wrap = s.wrap;
        list.push(Command::new(
            "wrap",
            if wrap { "Don't wrap long lines" } else { "Wrap long lines" },
            with(if wrap { |s, cx| s.set_wrap(false, cx) } else { |s, cx| s.set_wrap(true, cx) }),
        ));
        if s.shown.is_some() && s.stack.is_some() {
            let (keys, hidden) = s.range_keys();
            let session = session.clone();
            list.push(Command::new("comments", if hidden { "Show comments" } else { "Hide comments" }, move |_, _, cx| {
                session.update(cx, |s, cx| s.set_comments_hidden(&keys, !hidden, cx))
            }));
        }
        if s.repo().is_some() {
            let patterns = s.ignore_patterns.clone();
            {
                let (session, patterns) = (session.clone(), patterns.clone());
                list.push(
                    Command::new("ignore", "Ignore files matching", move |pattern, _, cx| {
                        let pattern = pattern.trim();
                        if pattern.is_empty() {
                            return;
                        }
                        let mut next = patterns.clone();
                        if !next.iter().any(|p| p == pattern) {
                            next.push(pattern.to_string());
                        }
                        session.update(cx, |s, cx| s.update_ignore(next, cx));
                    })
                    .with_keyword("ignore"),
                );
            }
            for pattern in &patterns {
                let (session, all, pattern) = (session.clone(), patterns.clone(), pattern.clone());
                let label = format!("Stop ignoring {pattern}");
                list.push(Command::new(&format!("unignore:{pattern}"), label, move |_, _, cx| {
                    let next = all.iter().filter(|p| **p != pattern).cloned().collect();
                    session.update(cx, |s, cx| s.update_ignore(next, cx));
                }));
            }
        }
        list
    }

    // ---------- rendering ----------

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let s = self.session.read(cx);
        let showing = s.showing_diff();
        let home_button = div()
            .id("home")
            .flex()
            .items_center()
            .px(u(6.))
            .h_full()
            .cursor_pointer()
            .text_color(p.fg_muted)
            .hover(|d| d.text_color(p.fg))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(format!("Inbox ({})", crate::ui::key_label("I"))).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.go_inbox(&GoInbox, window, cx)))
            .child(logo_mark(18., p.fg_muted));
        let mut header = div().flex().items_center().gap(u(10.)).flex_1().min_w(px(0.)).h_full().child(home_button);
        if showing {
            header = header.child(self.render_range_header(cx));
        } else {
            header = header.child(div().flex_1());
        }
        let s = self.session.read(cx);
        let drafts = s.drafts.len();
        let newer = s.newer.is_some();
        header
            .when(showing, |d| {
                let label = if drafts > 0 { format!("Submit review · {drafts}") } else { "Submit review".into() };
                d.child(
                    button(label, true, cx)
                        .id("review-button")
                        .flex_none()
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Submit review (r)").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.open_submit(window, cx))),
                )
            })
            .when(newer, |d| {
                d.child(
                    div()
                        .id("update-banner")
                        .flex_none()
                        .px(u(10.))
                        .py(u(3.))
                        .rounded(u(6.))
                        .bg(p.pr(1).opacity(0.15))
                        .text_color(p.pr(1))
                        .text_size(u(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .child("New commits pushed · load latest")
                        .on_click(cx.listener(|this, _, _, cx| this.session.update(cx, |s, cx| s.load_newer(cx)))),
                )
            })
    }

    fn render_range_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let s = self.session.read(cx);
        let (stack, shown) = (s.stack.as_ref().unwrap(), s.shown.as_ref().unwrap());
        let (lo, hi) = (shown.range.lo, shown.range.hi);
        let (bottom, top) = (&stack.prs[lo], &stack.prs[hi]);
        let summary = &shown.view.summary;
        let title = if lo == hi { top.title.clone() } else { format!("#{}–#{} · {} PRs", bottom.number, top.number, hi - lo + 1) };
        let meta = if lo == hi {
            format!("{} · {} · {} ← {} · {} files ·", pr_label(top), top.author, bottom.base_ref, top.head_ref, summary.files.len())
        } else {
            format!("{} · {} ← {} · {} files ·", top.base_repo, bottom.base_ref, top.head_ref, summary.files.len())
        };
        let since = shown.since.clone();
        let checkpoints = s.checkpoints.clone();
        let since_menu = self.menus.get("since-picker");
        // One row, as before: the title, then the meta on the same baseline; both give way.
        div()
            .flex()
            .items_baseline()
            .gap(u(12.))
            .min_w(px(0.))
            .flex_1()
            .overflow_hidden()
            .child(div().flex_shrink(1.).min_w(px(0.)).text_size(u(13.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(title))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .items_baseline()
                    .gap(u(4.))
                    .text_size(u(12.))
                    .text_color(p.fg_muted)
                    .whitespace_nowrap()
                    .child(div().flex_shrink(1.).min_w(px(0.)).overflow_hidden().text_ellipsis().child(meta))
                    .child(div().flex_none().text_color(p.add_fg).child(format!("+{}", summary.additions)))
                    .child(div().flex_none().text_color(p.del_fg).child(format!("−{}", summary.deletions)))
                    .when(shown.ignore_whitespace, |d| d.child(div().text_color(p.pr(1)).child("· whitespace hidden")))
                    .when_some(since, |d, since| {
                        let label = checkpoints
                            .iter()
                            .find(|c| c.id == since.checkpoint_id)
                            .map(checkpoint_label)
                            .unwrap_or_else(|| local_time(since.created_at));
                        d.child(div().text_color(p.accent).child("· changes since"))
                            .child(
                                div()
                                    .id("since-picker")
                                    .px(u(4.))
                                    .rounded(u(4.))
                                    .border_1()
                                    .border_color(p.border)
                                    .text_color(p.fg)
                                    .cursor_pointer()
                                    .child(format!("{label} ▾"))
                                    .on_click({
                                        let ids: Vec<String> = checkpoints.iter().map(|c| c.id.clone()).collect();
                                        let labels: Vec<SharedString> = checkpoints.iter().map(|c| checkpoint_label(c).into()).collect();
                                        let selected = ids.iter().position(|id| *id == since.checkpoint_id);
                                        cx.listener(move |this, _, window, cx| {
                                            let ids = ids.clone();
                                            this.menus.toggle(
                                                "since-picker",
                                                labels.clone(),
                                                selected,
                                                |this: &mut Self| &mut this.menus,
                                                move |this, i, _, cx| {
                                                    let id = ids.get(i).cloned();
                                                    this.session.update(cx, |s, cx| s.show_since(id, cx));
                                                },
                                                window,
                                                cx,
                                            );
                                            cx.notify();
                                        })
                                    })
                                    .when_some(since_menu, |d, menu| d.child(deferred(anchored().child(div().mt(u(18.)).child(menu))))),
                            )
                            .when(since.conflicts, |d| d.child(div().text_color(p.pr(1)).child("· rebase conflicted, showing raw changes")))
                    }),
            )
    }

    fn render_stack_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = palette(cx).clone();
        let s = self.session.read(cx);
        let (stack, shown) = (s.stack.as_ref()?, s.shown.as_ref()?);
        if stack.prs.len() < 2 || !s.showing_diff() {
            return None;
        }
        let (lo, hi) = (shown.range.lo, shown.range.hi);
        let mono = crate::ui::mono_family(cx);
        let chips = stack.prs.iter().enumerate().map(|(i, pr)| {
            let selected = i >= lo && i <= hi;
            let color = p.pr(i as u8);
            let warn = stack.needs_rebase.get(i).copied().unwrap_or(false);
            let ready = s.ready(i, i);
            let tip = format!(
                "{}\n{} ← {}{}\nShift-click to select a range",
                pr.title,
                pr.base_ref,
                pr.head_ref,
                if warn { "\n⚠ Not based on the latest head of the PR below" } else { "" }
            );
            // A selected range reads as one band: the chips inside it share their top and bottom
            // borders, only its ends are closed and rounded.
            let (first, last) = (selected && i == lo, selected && i == hi);
            div()
                .id(("stack-chip", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(u(6.))
                .h(u(24.))
                .px(u(10.))
                .max_w(u(260.))
                .text_size(u(12.))
                .text_color(if selected { p.fg } else { p.fg_muted })
                .hover(|d| d.text_color(p.fg))
                .cursor_pointer()
                .map(|d| {
                    if selected {
                        d.bg(color.opacity(0.14))
                            .border_color(color.opacity(0.45))
                            .border_t_1()
                            .border_b_1()
                            .when(first, |d| d.border_l_1().rounded_l(u(6.)))
                            .when(last, |d| d.border_r_1().rounded_r(u(6.)))
                    } else {
                        d.border_1().border_color(gpui_kit::transparent_black()).rounded(u(6.))
                    }
                })
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    let extend = event.modifiers().shift;
                    this.session.update(cx, |s, cx| s.click_chip(i, extend, cx));
                }))
                .child(div().size(u(8.)).flex_none().rounded_full().bg(color))
                .child(div().flex_none().font_family(mono.clone()).text_size(u(11.)).child(format!("#{}", pr.number)))
                .child(div().min_w(px(0.)).whitespace_nowrap().text_ellipsis().overflow_hidden().child(pr.title.clone()))
                .when(warn, |d| d.child(div().flex_none().text_color(p.pr(1)).child("⚠")))
                .when(!ready, |d| d.child(div().size(u(5.)).flex_none().rounded_full().bg(p.fg_muted.opacity(0.5))))
        });
        Some(
            // Scrolls sideways when the stack is wider than the window.
            div()
                .id("stack-bar")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(2.))
                .h(u(34.))
                .px(u(14.))
                .overflow_x_scroll()
                .border_b_1()
                .border_color(p.border)
                .bg(p.bg_sidebar)
                .child(
                    div()
                        .flex_none()
                        .pr(u(8.))
                        .font_family(mono.clone())
                        .text_size(u(11.))
                        .text_color(p.fg_muted)
                        .child(format!("{} ›", stack.prs[0].base_ref)),
                )
                .children(chips),
        )
    }
}

fn checkpoint_label(c: &wispy_core::progress::Checkpoint) -> String {
    format!("{} ({})", local_time(c.created_at), if c.source == "submit" { "submitted" } else { "marked" })
}

/// A Unix time as local "YYYY-MM-DD HH:MM".
pub fn local_time(seconds: i64) -> String {
    use std::process::Command;
    // No date crate in the tree: `date` formats in the local zone on macOS and Linux.
    let arg = if cfg!(target_os = "macos") { format!("-r{seconds}") } else { format!("-d@{seconds}") };
    Command::new("date")
        .args([arg.as_str(), "+%Y-%m-%d %H:%M"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| seconds.to_string())
}

impl Focusable for WispyApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WispyApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let showing = self.session.read(cx).showing_diff();
        let header = self.render_header(cx);
        let stack_bar = self.render_stack_bar(cx);
        let content = match (&self.diff, showing) {
            (Some(diff), true) => div().key_context("Stack").flex_1().min_h(px(0.)).child(diff.clone()).into_any_element(),
            _ => div().flex_1().min_h(px(0.)).child(self.inbox.clone()).into_any_element(),
        };
        let this = cx.entity().downgrade();
        let wheel = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let handled = this.update(cx, |this, cx| this.on_wheel_capture(event, cx)).unwrap_or(false);
                    if handled {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let overlay: Option<gpui_kit::AnyElement> = if let Some(palette_view) = &self.palette {
            let palette_view = palette_view.clone();
            Some(crate::ui::backdrop(
                palette_view,
                {
                    let this = cx.entity().downgrade();
                    move |window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.palette = None;
                            this.focus_content(window, cx);
                            cx.notify();
                        });
                    }
                },
                cx,
            ))
        } else if let Some(sheet) = &self.sheet {
            let sheet = sheet.clone();
            let close = {
                let sheet = sheet.clone();
                move |window: &mut Window, cx: &mut App| sheet.update(cx, |s, cx| s.close_from_backdrop(window, cx))
            };
            Some(crate::ui::backdrop(sheet, close, cx))
        } else {
            match &self.overlay {
                Some(Overlay::Theme(picker)) => {
                    let close = {
                        let picker = picker.clone();
                        move |_: &mut Window, cx: &mut App| picker.update(cx, |p, cx| p.cancel(cx))
                    };
                    Some(crate::ui::backdrop(picker.clone(), close, cx))
                }
                Some(Overlay::Shortcuts(help)) => {
                    let this = cx.entity().downgrade();
                    Some(crate::ui::backdrop(
                        help.clone(),
                        move |window, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.overlay = None;
                                this.focus_content(window, cx);
                                cx.notify();
                            });
                        },
                        cx,
                    ))
                }
                Some(Overlay::Newer(warning, _)) => {
                    let warning_view = warning.clone();
                    let close = move |_: &mut Window, cx: &mut App| warning_view.update(cx, |_, cx| cx.emit(OverlayEvent::MarkAnyway));
                    Some(crate::ui::backdrop(warning.clone(), close, cx))
                }
                None => None,
            }
        };

        div()
            .key_context("Wispy")
            .track_focus(&self.focus)
            // A click on the title bar or stack bar leaves the keyboard with the inbox / diff.
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.focus.is_focused(window) && this.palette.is_none() && this.overlay.is_none() && this.sheet.is_none() {
                        this.focus_content(window, cx);
                    }
                }),
            )
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::open_themes))
            .on_action(cx.listener(Self::show_shortcuts))
            .on_action(cx.listener(Self::go_inbox))
            .on_action(cx.listener(Self::toggle_files))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(-1, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom(0, cx)))
            .on_action(cx.listener(|this, _: &ToggleWhitespace, _, cx| this.stack_action(cx, |s, cx| s.toggle_whitespace(cx))))
            .on_action(cx.listener(|this, _: &MarkReviewed, _, cx| this.stack_action(cx, |s, cx| s.mark_reviewed(cx))))
            .on_action(cx.listener(|this, _: &ToggleSince, _, cx| this.stack_action(cx, |s, cx| s.toggle_since(cx))))
            .on_action(cx.listener(|this, _: &NextPr, _, cx| this.stack_action(cx, |s, cx| s.cycle_pr(false, cx))))
            .on_action(cx.listener(|this, _: &PrevPr, _, cx| this.stack_action(cx, |s, cx| s.cycle_pr(true, cx))))
            .on_action(cx.listener(|this, _: &RangeDown, _, cx| this.stack_action(cx, |s, cx| s.step_range('[', cx))))
            .on_action(cx.listener(|this, _: &RangeUp, _, cx| this.stack_action(cx, |s, cx| s.step_range(']', cx))))
            .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.stack_action(cx, |s, cx| s.step_range('{', cx))))
            .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.stack_action(cx, |s, cx| s.step_range('}', cx))))
            .on_action(cx.listener(|this, _: &OpenSubmit, window, cx| {
                if this.blocked() || this.palette.is_some() || this.sheet.is_some() {
                    return;
                }
                if this.session.read(cx).stack.is_some() {
                    this.open_submit(window, cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.fg)
            .font_family(crate::ui::sans_family(cx))
            .child(TitleBar::new().child(header))
            .children(stack_bar)
            .child(content)
            .child(wheel)
            .children(overlay)
            .when_some(self.toast.as_ref().map(|t| t.0.clone()), |d, message| {
                d.child(
                    div().absolute().bottom(u(20.)).left_0().right_0().flex().justify_center().child(
                        div()
                            .px(u(14.))
                            .py(u(8.))
                            .rounded(u(8.))
                            .bg(p.fg)
                            .text_color(p.bg)
                            .text_size(u(12.))
                            .when(p.shadows, |d| d.shadow_md())
                            .when_some(p.floating_border, |d, b| d.border_1().border_color(b))
                            .child(message),
                    ),
                )
            })
            .children(gpui_kit::component::Root::render_notification_layer(window, cx))
    }
}

pub fn theme_from_prefs(cx: &App) -> ThemeId {
    let ids: Vec<&str> = ThemeId::all().iter().map(|t| t.id()).collect();
    ThemeId::from_id(&settings::pref(cx, "theme", &ids, "system")).unwrap_or(ThemeId::System)
}
