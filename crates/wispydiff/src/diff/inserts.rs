//! What sits between the rows: the comment box (composer), draft cards and review threads (was
//! `Inserts.tsx`).

use std::sync::Arc;

use gpui_kit::component::input::{Enter, IndentInline, InputEvent, OutdentInline, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, AnyElement, App, ClipboardEntry, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, KeyBinding, Render, SharedString, Subscription, Window,
};
use wispy_core::drafts::{DraftKind, DraftStatus};
use wispy_core::github::{ReviewThread, ThreadComment};
use wispy_core::service::ShownDraft;

use super::{CommentKind, DiffViewer};
use crate::backend::Backend;
use crate::logic::comments::line_label;
use crate::logic::indent::{newline, reindent, Edit};
use crate::ui::{button, key_label, link_button, palette, status_badge, u, BadgeKind};

gpui_kit::actions!(composer, [Suggest, Cancel, SaveNow]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Composer");
    cx.bind_keys([
        KeyBinding::new("secondary-g", Suggest, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("secondary-enter", SaveNow, ctx),
    ]);
}

/// Why GitHub would show the thread folded away, or None for an open thread.
pub fn settled_reason(thread: &ReviewThread) -> Option<&'static str> {
    if thread.resolved {
        Some("Resolved")
    } else if thread.outdated {
        Some("Outdated")
    } else if thread.comments.first().is_some_and(|c| c.minimized) {
        Some("Hidden on GitHub")
    } else {
        None
    }
}

/// A hidden thread's gutter icon tooltip: the first comment.
pub fn thread_preview(thread: &ReviewThread) -> String {
    let Some(first) = thread.comments.first() else { return "Hidden comment".into() };
    let text = strip_html_comments(&first.body).split_whitespace().collect::<Vec<_>>().join(" ");
    let more = if thread.comments.len() > 1 { format!(" (+{} more)", thread.comments.len() - 1) } else { String::new() };
    let reason = settled_reason(thread).map(|r| format!("{r} · ")).unwrap_or_default();
    let text = if text.chars().count() > 120 { format!("{}…", text.chars().take(120).collect::<String>()) } else { text };
    format!("{reason}{}: {text}{more}", first.author)
}

fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

pub enum ComposerEvent {
    Save(String),
    Cancel,
    /// Its height may have changed (typing, a pasted image's note).
    Resized,
}

/// A comment box: Enter saves, Shift+Enter breaks the line; inside a ```suggestion block Enter
/// adds a line indented like the caret's (⌘Enter still saves) and Tab / Shift+Tab re-indent.
pub struct Composer {
    title: String,
    save_label: &'static str,
    /// GitHub won't take a line comment here: it'll go out as a file comment.
    fallback: Option<bool>,
    /// The commented lines, offered as a suggestion to edit (None: not possible).
    suggestion: Option<String>,
    /// For pasting images and the file's indentation: (backend, stack, PR, path).
    source: Option<(Arc<Backend>, String, usize, String)>,
    unit: Option<String>,
    paste_error: Option<String>,
    pub input: Entity<TextareaState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: String,
        initial: String,
        save_label: &'static str,
        fallback: Option<bool>,
        suggestion: Option<String>,
        source: Option<(Arc<Backend>, String, usize, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let can_suggest = suggestion.is_some();
        let initial_has_suggestion = initial.contains("```suggestion");
        let placeholder = format!(
            "Leave a comment · ↵ to save · ⇧↵ for a new line{} · paste images · Esc to cancel",
            if can_suggest { format!(" · {} to suggest a change", key_label("G")) } else { String::new() }
        );
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(3, 20).placeholder(placeholder);
            state.set_value(initial, window, cx);
            state
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscriptions = vec![cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                if this.unit.is_none() && this.text(cx).contains("```suggestion") {
                    this.load_unit(cx);
                }
                cx.emit(ComposerEvent::Resized);
            }
        })];
        let has_suggestion = initial_has_suggestion;
        let mut composer = Composer {
            title,
            save_label,
            fallback,
            suggestion,
            source,
            unit: None,
            paste_error: None,
            input,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        // Editing a draft that already holds a suggestion: its file's indentation right away.
        if has_suggestion {
            composer.load_unit(cx);
        }
        composer
    }

    pub fn set_fallback(&mut self, fallback: Option<bool>, cx: &mut Context<Self>) {
        self.fallback = fallback;
        cx.notify();
    }

    fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// One level of the file's indentation, asked for once a suggestion is there.
    fn load_unit(&mut self, cx: &mut Context<Self>) {
        self.unit = Some("    ".into());
        let Some((backend, stack, pr, path)) = &self.source else { return };
        let task = backend.indent_unit(stack.clone(), *pr, path.clone());
        cx.spawn(async move |this, cx| {
            if let Ok(unit) = task.await {
                let _ = this.update(cx, |this, _| this.unit = Some(unit));
            }
        })
        .detach();
    }

    /// A file comment can't carry a suggestion.
    fn can_suggest(&self) -> bool {
        self.suggestion.is_some() && self.fallback != Some(true)
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx).trim().to_string();
        if !text.is_empty() {
            cx.emit(ComposerEvent::Save(text));
        }
    }

    /// Types an edit in (undoable) and selects what it says.
    fn apply(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            state.set_selected_range(edit.from..edit.to, cx);
            state.replace(edit.insert, window, cx);
            state.set_selected_range(edit.start..edit.end, cx);
        });
    }

    /// Inserts a ```suggestion block with the lines at the caret, and leaves the caret at their end.
    fn suggest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(suggestion) = self.suggestion.clone().filter(|_| self.can_suggest()) else { return };
        let text = self.text(cx);
        let range = self.input.read(cx).selected_range();
        let (before, after) = (&text[..range.start], &text[range.end..]);
        let block = format!("{}```suggestion\n{suggestion}\n```", if !before.is_empty() && !before.ends_with('\n') { "\n" } else { "" });
        let tail = if !after.is_empty() && !after.starts_with('\n') { "\n" } else { "" };
        let caret = before.len() + block.len() - "\n```".len();
        let insert = format!("{block}{tail}");
        self.apply(Edit { from: range.start, to: range.end, insert, start: caret, end: caret }, window, cx);
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        // Inside a suggestion, Enter and Shift+Enter add a line indented like the caret's.
        if !action.secondary {
            let text = self.text(cx);
            let range = self.input.read(cx).selected_range();
            if let Some(edit) = newline(&text, range.start, range.end) {
                // Captured before the text box: it mustn't also insert its own line break.
                cx.stop_propagation();
                self.apply(edit, window, cx);
                return;
            }
        }
        if action.shift {
            return;
        }
        cx.stop_propagation();
        self.save(cx);
    }

    fn on_indent(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.text(cx);
        let range = self.input.read(cx).selected_range();
        let unit = self.unit.clone().unwrap_or_else(|| "    ".into());
        match reindent(&text, range.start, range.end, &unit, outdent) {
            Some(edit) => {
                self.apply(edit, window, cx);
                true
            }
            None => false,
        }
    }

    /// Pasted images go in as Markdown (kept locally, uploaded when submitting).
    fn on_paste(&mut self, item: &ClipboardItem, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let images: Vec<(String, Vec<u8>)> = item
            .entries()
            .iter()
            .filter_map(|e| match e {
                ClipboardEntry::Image(image) => Some((image_type(image.format), image.bytes.clone())),
                _ => None,
            })
            .collect();
        if images.is_empty() {
            return false;
        }
        let Some((backend, ..)) = &self.source else { return false };
        let tasks: Vec<_> = images.into_iter().map(|(ty, bytes)| backend.save_image_data(ty, bytes)).collect();
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let mut markdown = Vec::new();
            let mut error = None;
            for task in tasks {
                match task.await {
                    Ok(md) => markdown.push(md),
                    Err(err) => error = Some(err),
                }
            }
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.paste_error = error;
                    if !markdown.is_empty() {
                        let text = markdown.join("\n");
                        this.input.update(cx, |state, cx| state.insert(text, window, cx));
                    }
                    cx.emit(ComposerEvent::Resized);
                    cx.notify();
                });
            });
        })
        .detach();
        true
    }
}

pub fn image_type(format: gpui_kit::ImageFormat) -> String {
    use gpui_kit::ImageFormat::*;
    match format {
        Png => "image/png",
        Jpeg => "image/jpeg",
        Gif => "image/gif",
        Webp => "image/webp",
        Svg => "image/svg+xml",
        Bmp => "image/bmp",
        Tiff => "image/tiff",
        _ => "application/octet-stream",
    }
    .to_string()
}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let empty = self.text(cx).trim().is_empty();
        let weak = cx.entity().downgrade();
        div()
            .key_context("Composer")
            .track_focus(&self.focus)
            .capture_action(cx.listener(Self::on_enter))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.on_indent(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                if this.on_indent(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &Suggest, window, cx| this.suggest(window, cx)))
            .on_action(cx.listener(|this, _: &SaveNow, _, cx| this.save(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(ComposerEvent::Cancel)))
            .capture_action(cx.listener(|_, _: &gpui_kit::component::input::Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(ComposerEvent::Cancel)
            }))
            .flex()
            .flex_col()
            .gap(u(8.))
            .p(u(10.))
            .rounded(u(6.))
            .border_1()
            .border_color(p.floating_border.unwrap_or(p.border))
            .bg(p.bg_elevated)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_size(u(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
                    .when(self.fallback == Some(true), |d| {
                        d.child(
                            div()
                                .id("fallback-badge")
                                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("GitHub only takes line comments inside the PR's diff").build(window, cx))
                                .child(status_badge("will post as file comment", BadgeKind::Warn, cx)),
                        )
                    }),
            )
            .child(Textarea::new(&self.input).on_paste(move |item, window, cx| {
                weak.update(cx, |this, cx| this.on_paste(item, window, cx)).unwrap_or(false)
            }))
            .when_some(self.paste_error.clone(), |d, e| d.child(div().text_size(u(12.)).text_color(p.del_fg).child(format!("Couldn't paste the image: {e}"))))
            .child(
                div()
                    .flex()
                    .gap(u(8.))
                    .child(
                        button(self.save_label, true, cx)
                            .id("composer-save")
                            .when(empty, |d| d.opacity(0.5).cursor_default())
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(button("Cancel", false, cx).id("composer-cancel").on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Cancel))))
                    .when(self.can_suggest(), |d| {
                        d.child(
                            button("± Suggest change", false, cx)
                                .id("composer-suggest")
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(format!("Suggest a change to these lines ({})", key_label("G"))).build(window, cx)
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.suggest(window, cx))),
                        )
                    }),
            )
    }
}

// ---------- cards in the diff ----------

impl DiffViewer {
    pub fn render_insert(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let Some(item) = self.items_by_key.get(key).cloned() else { return div().into_any_element() };
        let indent = super::rows::gutter_width(self, cx) * 2. + super::u_px(16. + 8., cx);
        let content = match item.kind {
            CommentKind::Composer => match &self.composer {
                Some((_, composer)) => composer.clone().into_any_element(),
                None => div().into_any_element(),
            },
            CommentKind::Draft(shown) => self.draft_card(&shown, window, cx),
            CommentKind::Thread { pr_index, thread } => self.thread_card(pr_index, &thread, window, cx),
        };
        div()
            .w_full()
            .py(u(6.))
            .pl(indent)
            .pr(u(24.))
            .bg(p.bg)
            .font_family(crate::ui::sans_family(cx))
            .text_size(u(13.))
            .line_height(u(18.))
            .child(div().max_w(u(860.)).child(content))
            .into_any_element()
    }

    fn markdown_options(&self, cx: &App) -> crate::markdown::RenderOptions {
        let mut opts = crate::ui::markdown_options(&self.backend, cx);
        opts.image = Some(crate::ui::image_resolver());
        opts
    }

    fn draft_card(&mut self, shown: &ShownDraft, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let draft = &shown.draft;
        let id = draft.id.clone();
        let where_ = if draft.kind == DraftKind::File {
            "file".to_string()
        } else {
            let a = line_label(shown.start_line, shown.line);
            if a.is_empty() { line_label(draft.start_line, draft.line) } else { a }
        };
        let label = format!("#{} · {where_}", self.prs[shown.pr_index as usize].number);
        if let Some(editor) = self.draft_edits.get(&id) {
            return editor.clone().into_any_element();
        }
        let outdated = draft.kind == DraftKind::Line && shown.line.is_none();
        let opts = self.markdown_options(cx);
        let body = crate::markdown::markdown(SharedString::from(format!("draft-{id}")), &draft.body, true, &opts);
        let (edit_id, delete_id) = (id.clone(), id.clone());
        let (pr, path) = (shown.pr_index as usize, shown.path.clone().or_else(|| draft.path.clone()).unwrap_or_default());
        let initial = draft.body.clone();
        card(&p)
            .when(draft.status == DraftStatus::Failed, |d| d.border_color(p.del_fg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_size(u(12.))
                    .child(status_badge("Draft", BadgeKind::Plain, cx))
                    .when(draft.assistant, |d| d.child(status_badge("by the assistant", BadgeKind::Plain, cx)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(label.clone()))
                    .when(outdated, |d| d.child(status_badge("outdated — lines changed since", BadgeKind::Warn, cx)))
                    .when(draft.status == DraftStatus::Posting, |d| d.child(status_badge("sending…", BadgeKind::Plain, cx))),
            )
            .child(body)
            .when(draft.status == DraftStatus::Failed, |d| {
                d.child(div().text_size(u(12.)).text_color(p.del_fg).child(format!("GitHub rejected this: {}", draft.error.clone().unwrap_or_default())))
            })
            .child(
                div()
                    .flex()
                    .gap(u(8.))
                    .child(button("Edit", false, cx).id(SharedString::from(format!("edit-{id}"))).on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_draft_open(edit_id.clone(), label.clone(), initial.clone(), pr, path.clone(), window, cx)
                    })))
                    .child(
                        button("Delete", false, cx)
                            .id(SharedString::from(format!("delete-{id}")))
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_draft(delete_id.clone(), cx))),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn edit_draft_open(&mut self, id: String, label: String, initial: String, pr: usize, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let source = Some((self.backend.clone(), self.stack_id.clone(), pr, path));
        let composer = cx.new(|cx| Composer::new(format!("Edit draft · {label}"), initial, "Save", None, None, source, window, cx));
        let key = format!("draft:{id}");
        let draft_id = id.clone();
        cx.subscribe_in(&composer, window, move |this, _, event: &ComposerEvent, window, cx| match event {
            ComposerEvent::Save(body) => {
                this.draft_edits.remove(&id);
                this.edit_draft(id.clone(), body.clone(), cx);
                window.focus(&this.focus_handle_diff(), cx);
                this.remeasure_insert_key(&key, cx);
            }
            ComposerEvent::Cancel => {
                this.draft_edits.remove(&id);
                window.focus(&this.focus_handle_diff(), cx);
                this.remeasure_insert_key(&key, cx);
            }
            ComposerEvent::Resized => this.remeasure_insert_key(&key, cx),
        })
        .detach();
        self.draft_edits.insert(draft_id.clone(), composer);
        self.remeasure_insert_key(&format!("draft:{draft_id}"), cx);
    }

    fn thread_card(&mut self, pr_index: usize, thread: &ReviewThread, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let s = self.session.read(cx);
        let pending: Vec<ShownDraft> = s.drafts.iter().filter(|d| d.draft.thread_id.as_deref() == Some(thread.id.as_str())).cloned().collect();
        let replies: Vec<ShownDraft> = pending.iter().filter(|d| d.draft.kind == DraftKind::Reply).cloned().collect();
        let resolving = pending.iter().find(|d| d.draft.kind == DraftKind::Resolve).cloned();
        let number = self.prs[pr_index].number;
        let where_ = if thread.file_level {
            "file".to_string()
        } else if thread.line.is_some() {
            line_label(thread.start_line, thread.line)
        } else {
            format!("was {}", line_label(None, thread.original_line))
        };
        let label = format!("#{number} · {} {where_}", thread.path.rsplit('/').next().unwrap_or_default());
        let id = thread.id.clone();
        let opts = self.markdown_options(cx);
        let comments: Vec<AnyElement> = thread.comments.iter().map(|c| self.thread_comment(c, &opts, cx)).collect();
        let replies_el: Vec<AnyElement> = replies
            .iter()
            .map(|r| {
                let draft_id = r.draft.id.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    .pl(u(10.))
                    .border_l_2()
                    .border_color(p.accent.opacity(0.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .child(status_badge("Draft reply", BadgeKind::Plain, cx))
                            .child(link_button("discard", cx).id(SharedString::from(format!("discard-{draft_id}"))).on_click(cx.listener(
                                move |this, _, _, cx| this.delete_draft(draft_id.clone(), cx),
                            ))),
                    )
                    .child(crate::markdown::markdown(SharedString::from(format!("reply-{}", r.draft.id)), &r.draft.body, true, &opts))
                    .into_any_element()
            })
            .collect();
        let reply_box = self.thread_replies.get(&id).cloned();
        let hide_id = id.clone();
        let actions = match reply_box {
            Some(composer) => composer.into_any_element(),
            None => {
                let (reply_id, resolve_id) = (id.clone(), id.clone());
                let (path, reply_to) = (thread.path.clone(), thread.comments.first().map(|c| c.database_id));
                let resolve_path = thread.path.clone();
                div()
                    .flex()
                    .gap(u(8.))
                    .child(button("Reply", false, cx).id(SharedString::from(format!("reply-{id}"))).on_click(cx.listener(move |this, _, window, cx| {
                        this.open_reply(reply_id.clone(), pr_index, path.clone(), reply_to, window, cx)
                    })))
                    .when(!thread.resolved, |d| match resolving {
                        Some(r) => {
                            let draft_id = r.draft.id.clone();
                            d.child(
                                button("Resolves on submit · undo", false, cx)
                                    .id(SharedString::from(format!("unresolve-{id}")))
                                    .on_click(cx.listener(move |this, _, _, cx| this.delete_draft(draft_id.clone(), cx))),
                            )
                        }
                        None => d.child(button("Resolve", false, cx).id(SharedString::from(format!("resolve-{id}"))).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.save_draft(
                                    wispy_core::service::NewDraft {
                                        pr_index,
                                        kind: DraftKind::Resolve,
                                        path: Some(resolve_path.clone()),
                                        side: None,
                                        line: None,
                                        start_line: None,
                                        body: String::new(),
                                        thread_id: Some(resolve_id.clone()),
                                        reply_to: None,
                                        assistant: false,
                                    },
                                    cx,
                                )
                            },
                        ))),
                    })
                    .into_any_element()
            }
        };
        let _ = window;
        card(&p)
            .when(thread.resolved, |d| d.opacity(0.85))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_size(u(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                    .when(thread.resolved, |d| d.child(status_badge("resolved", BadgeKind::Ok, cx)))
                    .when(thread.outdated, |d| d.child(status_badge("outdated", BadgeKind::Plain, cx)))
                    .child(div().flex_1())
                    .child(
                        link_button("hide", cx)
                            .id(SharedString::from(format!("hide-{id}")))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Hide this thread (a comment icon in the gutter brings it back)").build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.hide_thread(&hide_id, cx)
                            })),
                    ),
            )
            .children(comments)
            .children(replies_el)
            .child(actions)
            .into_any_element()
    }

    /// One comment of a thread; a minimized one stays folded until clicked, like on GitHub.
    fn thread_comment(&self, comment: &ThreadComment, opts: &crate::markdown::RenderOptions, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx).clone();
        let folded = comment.minimized && !self.unfolded.contains(&comment.id);
        let id = comment.id.clone();
        let body = if folded {
            None
        } else if comment.body_html.is_empty() {
            let text = strip_wispy_markers(&comment.body);
            Some(div().whitespace_normal().child(text).into_any_element())
        } else {
            Some(crate::markdown::html(SharedString::from(format!("comment-{}", comment.id)), &comment.body_html, opts))
        };
        div()
            .flex()
            .flex_col()
            .gap(u(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .text_size(u(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(comment.author.clone()))
                    .child(div().text_color(p.fg_muted).child(format!("· {}", comment.created_at.get(..10).unwrap_or_default())))
                    .when(folded, |d| {
                        d.child(link_button("hidden on GitHub · show", cx).id(SharedString::from(format!("unfold-{id}"))).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.unfolded.insert(id.clone());
                                this.remeasure_all(cx);
                            },
                        )))
                    }),
            )
            .children(body)
            .into_any_element()
    }

    fn open_reply(&mut self, thread_id: String, pr_index: usize, path: String, reply_to: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let source = Some((self.backend.clone(), self.stack_id.clone(), pr_index, path.clone()));
        let composer = cx.new(|cx| Composer::new("Reply".into(), String::new(), "Save reply", None, None, source, window, cx));
        let key = format!("thread:{thread_id}");
        let id = thread_id.clone();
        cx.subscribe_in(&composer, window, move |this, _, event: &ComposerEvent, window, cx| match event {
            ComposerEvent::Save(body) => {
                this.thread_replies.remove(&id);
                this.save_draft(
                    wispy_core::service::NewDraft {
                        pr_index,
                        kind: DraftKind::Reply,
                        path: Some(path.clone()),
                        side: None,
                        line: None,
                        start_line: None,
                        body: body.clone(),
                        thread_id: Some(id.clone()),
                        reply_to,
                        assistant: false,
                    },
                    cx,
                );
                window.focus(&this.focus_handle_diff(), cx);
                this.remeasure_insert_key(&key, cx);
            }
            ComposerEvent::Cancel => {
                this.thread_replies.remove(&id);
                window.focus(&this.focus_handle_diff(), cx);
                this.remeasure_insert_key(&key, cx);
            }
            ComposerEvent::Resized => this.remeasure_insert_key(&key, cx),
        })
        .detach();
        self.thread_replies.insert(thread_id.clone(), composer);
        self.remeasure_insert_key(&format!("thread:{thread_id}"), cx);
    }

    /// Every row's height may have changed.
    pub fn remeasure_all(&self, cx: &mut Context<Self>) {
        self.list.remeasure();
        cx.notify();
    }
}

/// The app's own markers are left out of comments shown as text.
fn strip_wispy_markers(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find("<!-- wispydiff:") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

fn card(p: &crate::theme::Palette) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(u(10.))
        .p(u(10.))
        .rounded(u(6.))
        .border_1()
        .border_color(p.floating_border.unwrap_or(p.border))
        .bg(p.bg_elevated)
        .min_w(px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markers_and_comments() {
        assert_eq!(strip_wispy_markers("hi <!-- wispydiff:abc --> there"), "hi  there");
        assert_eq!(strip_html_comments("a<!-- x -->b"), "ab");
    }
}
