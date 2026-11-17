//! `i` / double-click on a PR: its description as GitHub renders it (`bodyHTML`), in a modal.
//! A range of PRs shows each one's description, bottom of the stack first.

use gpui_kit::prelude::*;
use gpui_kit::{div, px, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Render, ScrollHandle, SharedString, Window};

use crate::markdown::RenderOptions;
use crate::overlays::{Cancel, Next, OverlayEvent, Prev};
use crate::ui::{floating, link_button, palette, u};

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
}

impl EventEmitter<OverlayEvent> for DescriptionView {}

impl DescriptionView {
    pub fn new(prs: Vec<DescribedPr>, opts: RenderOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        DescriptionView { prs, opts, scroll: ScrollHandle::new(), focus }
    }

    pub fn prs(&self) -> &[DescribedPr] {
        &self.prs
    }

    fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
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
                        .child(div().flex_none().font_family(mono.clone()).text_size(u(12.)).text_color(p.fg_muted).child(pr.label.clone()))
                        .child(div().flex_1().min_w(px(0.)).text_size(u(15.)).font_weight(FontWeight::SEMIBOLD).child(pr.title.clone()))
                        .when(!url.is_empty(), |d| {
                            d.child(
                                link_button("open on GitHub", cx)
                                    .id(SharedString::from(format!("description-open-{}", pr.label)))
                                    .flex_none()
                                    .on_click(move |_, window, cx| backend_open(&url, window, cx)),
                            )
                        }),
                )
                .child(div().text_size(u(12.)).text_color(p.fg_muted).child(format!("by {}", pr.author)))
                .child(div().text_size(u(13.)).line_height(u(20.)).child(body))
        });
        floating(820., cx)
            .max_h(gpui_kit::relative(0.8))
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
                    .px(u(20.))
                    .py(u(16.))
                    .flex()
                    .flex_col()
                    .gap(u(16.))
                    .children(sections),
            )
    }
}
