//! Overlays: the theme picker (⌘T), the shortcuts list (? / ⌘/), the newer-changes warning.

use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, Render, SharedString, Window,
};

use crate::theme::ThemeId;
use crate::ui::{button, click_label, floating, kbd, key_label, link_button, palette, u, Looks};

gpui_kit::actions!(overlay, [Next, Prev, Confirm, Cancel]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Overlay");
    cx.bind_keys([
        KeyBinding::new("down", Next, ctx),
        KeyBinding::new("tab", Next, ctx),
        KeyBinding::new("up", Prev, ctx),
        KeyBinding::new("shift-tab", Prev, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("escape", Cancel, ctx),
    ]);
}

pub enum OverlayEvent {
    /// Closed; with the theme that was chosen, if any.
    Closed(Option<ThemeId>),
    /// The newer-changes warning: load the newer version.
    LoadNewer,
    /// The newer-changes warning: mark viewed anyway.
    MarkAnyway,
}

// ---------- theme picker ----------

/// ⌘T: pick a theme. Moving through the list previews it; Enter keeps it, Escape goes back.
pub struct ThemePicker {
    initial: ThemeId,
    selected: usize,
    focus: FocusHandle,
}

impl EventEmitter<OverlayEvent> for ThemePicker {}

impl ThemePicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = cx.global::<Looks>().theme;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        ThemePicker { initial, selected: ThemeId::all().iter().position(|t| *t == initial).unwrap_or(0), focus }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        Looks::preview(Some(ThemeId::all()[index]), cx);
        cx.notify();
    }

    fn next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        self.select((self.selected + 1) % ThemeId::all().len(), cx);
    }

    fn prev(&mut self, _: &Prev, _: &mut Window, cx: &mut Context<Self>) {
        let n = ThemeId::all().len();
        self.select((self.selected + n - 1) % n, cx);
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.choose(self.selected, cx);
    }

    pub fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let theme = ThemeId::all()[index];
        Looks::set_theme(theme, cx);
        cx.emit(OverlayEvent::Closed(Some(theme)));
    }

    /// Keeps the theme being previewed (⌘T again).
    pub fn choose_current(&mut self, cx: &mut Context<Self>) {
        self.choose(self.selected, cx);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        Looks::preview(None, cx);
        cx.emit(OverlayEvent::Closed(None));
    }

    fn on_cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel(cx);
    }
}

impl Focusable for ThemePicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ThemePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        floating(360., cx)
            .key_context("Overlay")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::prev))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::on_cancel))
            .child(overlay_title("Theme", cx))
            .child(div().py(u(4.)).children(ThemeId::all().iter().enumerate().map(|(i, theme)| {
                div()
                    .id(SharedString::from(format!("theme-{}", theme.id())))
                    .flex()
                    .justify_between()
                    .px(u(14.))
                    .py(u(6.))
                    .text_size(u(13.))
                    .when(i == self.selected, |d| d.bg(p.selection))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.selected != i {
                            this.select(i, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(i, cx)))
                    .child(theme.name())
                    .when(*theme == self.initial, |d| d.child(div().text_size(u(11.)).text_color(p.fg_muted).child("current")))
            })))
    }
}

fn overlay_title(title: &'static str, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .px(u(14.))
        .pt(u(12.))
        .pb(u(6.))
        .text_size(u(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.fg_muted)
        .child(title.to_uppercase())
}

// ---------- newer-changes warning ----------

/// Marking a file viewed while newer commits are waiting and it changed in them: load the newer
/// version (Enter; the file stays unviewed) or mark it anyway (Esc; no more warnings for that version).
pub struct NewerChangesWarning {
    path: String,
    focus: FocusHandle,
}

impl EventEmitter<OverlayEvent> for NewerChangesWarning {}

impl NewerChangesWarning {
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        NewerChangesWarning { path, focus }
    }
}

impl Focusable for NewerChangesWarning {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for NewerChangesWarning {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let name = self.path.rsplit('/').next().unwrap_or_default().to_string();
        floating(440., cx)
            .key_context("Overlay")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(OverlayEvent::LoadNewer)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(OverlayEvent::MarkAnyway)))
            .child(overlay_title("Newer changes", cx))
            .child(
                div()
                    .px(u(14.))
                    .text_size(u(13.))
                    .flex()
                    .flex_wrap()
                    .gap(u(4.))
                    .child(div().font_family(crate::ui::mono_family(cx)).text_color(p.fg).child(name))
                    .child("changed in commits pushed since this version was loaded."),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(u(8.))
                    .p(u(14.))
                    .child(
                        button("Mark viewed anyway", false, cx)
                            .id("newer-dismiss")
                            .child(kbd("Esc", cx))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(OverlayEvent::MarkAnyway))),
                    )
                    .child(
                        button("Load latest", true, cx)
                            .id("newer-load")
                            .child(kbd("↵", cx))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(OverlayEvent::LoadNewer))),
                    ),
            )
    }
}

// ---------- shortcuts ----------

fn shortcuts() -> Vec<(&'static str, Vec<(Vec<String>, String)>)> {
    let k = |s: &str| key_label(s);
    let s = |s: &str| s.to_string();
    vec![
        (
            "Anywhere",
            vec![
                (vec![k("K")], s("Open a PR (URL or owner/repo#123) · type > for commands")),
                (vec![k("I")], s("Inbox")),
                (vec![k("T")], s("Theme")),
                (vec![s("?"), k("/")], s("This list")),
                (vec![k("+"), k("-"), k("0")], format!("Zoom in / out / reset (or {})", k("scroll"))),
            ],
        ),
        (
            "Inbox",
            vec![
                (vec![s("j"), s("k")], s("Move down / up")),
                (vec![s("↵")], s("Open (a collapsed stack: expand)")),
                (vec![s("Space"), s("e")], s("Collapse / expand the stack")),
                (vec![s("r")], s("Refresh")),
            ],
        ),
        (
            "Moving around",
            vec![
                (vec![s("↓"), s("↑")], s("Scroll")),
                (vec![s("⇧↓"), s("⇧↑")], s("Next / previous file")),
                (vec![s("n"), s("p")], s("Next / previous file")),
                (vec![s("⇧⇧")], s("Find a file by name")),
                (
                    vec![k("["), k("]")],
                    s("Back / forward to where a jump left (also Alt+← / Alt+→ and the mouse's back / forward buttons)"),
                ),
                (vec![k("B")], s("Show / hide the file list")),
                (vec![s("t")], s("File list as a directory tree / flat list")),
                (vec![s("f")], s("Filter files by extension (↑/↓, Space to toggle)")),
                (vec![s("x")], s("Hide the current file's extension")),
                (vec![s("F")], s("Switch the file filter off / on")),
            ],
        ),
        (
            "Stack",
            vec![
                (vec![s("⇥"), s("⇧⇥")], s("Next / previous PR")),
                (vec![s("]"), s("[")], s("Move the selection up / down the stack")),
                (vec![s("}"), s("{")], s("Extend the selection up / down")),
                (vec![s("w")], s("Show / hide whitespace changes")),
                (vec![s("M")], s("Mark as reviewed (local checkpoint)")),
                (vec![s("d")], s("Changes since the last checkpoint")),
            ],
        ),
        (
            "Files",
            vec![
                (vec![s("s")], s("Side by side for this file")),
                (vec![s("S")], s("Side by side for all files")),
                (vec![s("z")], s("Wrap long lines")),
                (vec![s("Space"), s("e")], s("Collapse / expand the file")),
                (vec![s("E")], s("Show / hide all unchanged lines between the hunks (unified)")),
                (vec![s("v")], s("Mark viewed and go to the next file")),
                (vec![s("V")], s("Undo the last viewed mark and go back to that file")),
            ],
        ),
        (
            "Review",
            vec![
                (vec![s("c")], s("Comment on the line under the mouse (or the file)")),
                (vec![s("h")], s("Hide / show the selected PRs' comment threads")),
                (vec![s("↵")], s("Save the comment (⇧↵: new line)")),
                (vec![k("G")], s("Suggest a change to the commented lines")),
                (vec![s("Tab"), s("⇧Tab")], s("Indent / outdent the selected lines of a suggestion")),
                (vec![s("↵")], format!("New line in a suggestion, indented like the last ({} saves)", k("↵"))),
                (vec![s(click_label()), s("u")], s("Usages of the name under the mouse")),
                (vec![s("/")], s("Search the repo")),
                (vec![s("a")], s("Ask the assistant (about the selected lines, if any)")),
                (vec![s("r")], s("Submit review…")),
                (
                    vec![k("1"), k("2"), k("3")],
                    s("In the submit sheet: comment / approve / request changes (your own PRs: comment only)"),
                ),
                (vec![s("⇥"), s("⇧⇥")], s("In the submit sheet: next / previous PR's summary")),
                (vec![s("↵"), k("↵")], s("In the submit sheet: submit this PR / submit all")),
                (vec![s("Esc")], s("Close the assistant / panel / comment box")),
            ],
        ),
    ]
}

/// `?` / ⌘/: every keyboard shortcut.
pub struct ShortcutsHelp {
    focus: FocusHandle,
}

impl EventEmitter<OverlayEvent> for ShortcutsHelp {}

impl ShortcutsHelp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        ShortcutsHelp { focus }
    }
}

impl Focusable for ShortcutsHelp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ShortcutsHelp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        // Grows with the window (its groups flow into more columns).
        floating(860., cx)
            .w(gpui_kit::relative(0.6))
            .min_w(u(560.))
            .max_h(gpui_kit::relative(0.8))
            .key_context("Overlay Shortcuts")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(OverlayEvent::Closed(None))))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(u(16.))
                    .py(u(10.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().text_size(u(14.)).font_weight(FontWeight::SEMIBOLD).child("Keyboard shortcuts"))
                    .child(
                        link_button("close", cx).id("shortcuts-close").on_click(cx.listener(|_, _, _, cx| cx.emit(OverlayEvent::Closed(None)))),
                    ),
            )
            .child(
                div()
                    .id("shortcuts-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p(u(16.))
                    .flex()
                    .flex_wrap()
                    .gap(u(20.))
                    .children(shortcuts().into_iter().map(|(group, entries)| {
                        div()
                            .flex_1()
                            .min_w(u(340.))
                            .flex()
                            .flex_col()
                            .gap(u(4.))
                            .child(div().text_size(u(11.)).font_weight(FontWeight::SEMIBOLD).text_color(p.fg_muted).child(group.to_uppercase()))
                            .children(entries.into_iter().map(|(keys, what)| {
                                div()
                                    .flex()
                                    .gap(u(10.))
                                    .items_start()
                                    .child(div().w(u(130.)).flex_none().flex().flex_wrap().gap(u(3.)).children(keys.into_iter().map(|k| kbd(k, cx))))
                                    .child(div().flex_1().min_w(px(0.)).text_size(u(12.)).child(what))
                            }))
                    })),
            )
    }
}
