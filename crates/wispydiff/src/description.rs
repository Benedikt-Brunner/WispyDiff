//! `i` / double-click on a PR: its description as GitHub renders it (`bodyHTML`), in a modal.
//! A stack shows every PR's description, bottom first, scrolled to the one asked for.

use std::{cell::Cell, rc::Rc};

use gpui_kit::prelude::*;
use gpui_kit::{div, px, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Render, ScrollHandle, SharedString, Window};

use crate::markdown::RenderOptions;
use crate::overlays::{Cancel, Next, OverlayEvent, Prev};
use crate::ui::{link_button, palette, panel, u};

/// How far ↑/↓ scroll the description.
const SCROLL_STEP: f32 = 60.;

/// One PR as the modal shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct DescribedPr {
    /// `owner/repo#N`.
    pub label: String,
    pub title: String,
    pub author: String,
    pub url: String,
    /// GitHub's rendering of the description; empty when there is none.
    pub body_html: String,
    /// The PR's colour in the open stack.
    pub color: Option<u8>,
}

pub struct DescriptionView {
    prs: Vec<DescribedPr>,
    opts: RenderOptions,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The current press started inside the panel (the backdrop leaves it open then).
    pressed_inside: Rc<Cell<bool>>,
    /// The PR to keep at the top until the reader scrolls: the descriptions get their heights
    /// over the first frames, so a single scroll would land short.
    pin_top: Option<usize>,
}

impl EventEmitter<OverlayEvent> for DescriptionView {}

impl DescriptionView {
    /// Opens scrolled so that `prs[first]` starts at the top (as far as the content allows).
    pub fn new(prs: Vec<DescribedPr>, first: usize, opts: RenderOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        DescriptionView { prs, opts, scroll: ScrollHandle::new(), focus, pressed_inside: Rc::default(), pin_top: (first > 0).then_some(first) }
    }

    /// Whether the press the backdrop sees started inside the panel (and forgets it). The panel
    /// lets presses through, so its text can be selected.
    pub fn take_pressed_inside(&self) -> bool {
        self.pressed_inside.take()
    }

    /// The PR whose description is at the top of the modal's viewport.
    pub fn top_pr(&self) -> Option<usize> {
        // Child bounds are where the children are laid out before scrolling.
        let top = self.scroll.bounds().top() - self.scroll.offset().y;
        (0..self.prs.len()).find(|&i| self.scroll.bounds_for_item(i).is_some_and(|b| b.bottom() > top + px(1.)))
    }

    /// How far the modal is scrolled, and how far it could be.
    pub fn scrolled(&self) -> (gpui_kit::Pixels, gpui_kit::Pixels) {
        (-self.scroll.offset().y, self.scroll.max_offset().y)
    }

    pub fn prs(&self) -> &[DescribedPr] {
        &self.prs
    }

    fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        self.pin_top = None;
        let offset = self.scroll.offset();
        let max = self.scroll.max_offset().y;
        let y = (offset.y - px(dy) * crate::ui::zoom(cx)).clamp(-max, px(0.));
        self.scroll.set_offset(gpui_kit::point(offset.x, y));
        cx.notify();
    }
}

impl Focusable for DescriptionView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DescriptionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let mono = crate::ui::mono_family(cx);
        let heading = if self.prs.len() == 1 { "Description".to_string() } else { format!("Descriptions · {} PRs", self.prs.len()) };
        let sections = self.prs.iter().enumerate().map(|(i, pr)| {
            let url = pr.url.clone();
            let backend_open = self.opts.on_open_url.clone();
            let body = if pr.body_html.trim().is_empty() {
                div().text_size(u(13.)).italic().text_color(p.fg_muted).child("No description provided.").into_any_element()
            } else {
                crate::markdown::html(SharedString::from(format!("pr-description-{}", pr.label)), &pr.body_html, &self.opts)
            };
            div()
                .flex()
                .flex_col()
                .gap(u(8.))
                .when(i > 0, |d| d.pt(u(16.)).border_t_1().border_color(p.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .when_some(pr.color, |d, color| d.child(div().size(u(8.)).flex_none().rounded_full().bg(p.pr(color))))
                        .child(div().flex_1().min_w(px(0.)).font_family(mono.clone()).text_size(u(12.)).text_color(p.fg_muted).child(pr.label.clone()))
                        .when(!url.is_empty(), |d| {
                            d.child(
                                link_button("open on GitHub", cx)
                                    .id(SharedString::from(format!("description-open-{}", pr.label)))
                                    .flex_none()
                                    .on_click(move |_, window, cx| backend_open(&url, window, cx)),
                            )
                        }),
                )
                .child(div().text_size(u(15.)).font_weight(FontWeight::SEMIBOLD).child(pr.title.clone()))
                .child(div().text_size(u(12.)).text_color(p.fg_muted).child(format!("by {}", pr.author)))
                .child(div().text_size(u(13.)).line_height(u(20.)).child(body))
        });
        if let Some(first) = self.pin_top {
            self.scroll.scroll_to_top_of_item(first);
        }
        // Room below the last description, so that any PR's can scroll to the top (from the last
        // frame's layout; the body's padding stays out of it).
        let last = self.prs.len().saturating_sub(1);
        let room = match (self.scroll.bounds_for_item(last), self.scroll.bounds().size.height) {
            (Some(item), viewport) if viewport > px(0.) => (viewport - item.size.height - crate::ui::upx(32., cx)).max(px(0.)),
            _ => px(0.),
        };
        let pressed_inside = self.pressed_inside.clone();
        panel(820., cx)
            .max_h(gpui_kit::relative(0.8))
            .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, _| pressed_inside.set(true))
            .key_context("Overlay Description")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(OverlayEvent::Closed(None))))
            .on_action(cx.listener(|this, _: &Next, _, cx| this.scroll_by(SCROLL_STEP, cx)))
            .on_action(cx.listener(|this, _: &Prev, _, cx| this.scroll_by(-SCROLL_STEP, cx)))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(u(16.))
                    .py(u(10.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().text_size(u(14.)).font_weight(FontWeight::SEMIBOLD).child(heading))
                    .child(link_button("close", cx).id("description-close").on_click(cx.listener(|_, _, _, cx| cx.emit(OverlayEvent::Closed(None))))),
            )
            .child(
                div()
                    .id("description-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll_wheel(cx.listener(|this, _, _, _| this.pin_top = None))
                    .px(u(20.))
                    .py(u(16.))
                    .flex()
                    .flex_col()
                    .gap(u(16.))
                    .children(sections)
                    .child(div().flex_none().h(room)),
            )
    }
}
