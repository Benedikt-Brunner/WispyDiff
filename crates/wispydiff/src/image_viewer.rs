//! Clicking an image in rich text (descriptions, comments, drafts) shows it over the whole window,
//! as big as the window allows; one smaller than the window can go back to its own size. An animated GIF / WebP plays there with controls: play / pause,
//! frame steps and a scrubber. The frames are the ones GPUI already decoded for the inline image;
//! the viewer paints the frame it chooses instead of letting `img` run the animation.

use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

use gpui_kit::prelude::*;
use gpui_kit::{
    canvas, div, point, px, size, svg, AnyElement, App, Bounds, ClickEvent, Context, Corners, EventEmitter, FocusHandle,
    Focusable, Hsla, Image, ImgResourceLoader, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Render,
    RenderImage, Resource, SharedString, SharedUri, Task, Window,
};

use crate::ui::{kbd, palette, u};

gpui_kit::actions!(image_viewer, [TogglePlay, StepBack, StepForward, Restart, ToggleSize, Close]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("ImageViewer");
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, ctx),
        KeyBinding::new("k", TogglePlay, ctx),
        KeyBinding::new("left", StepBack, ctx),
        KeyBinding::new(",", StepBack, ctx),
        KeyBinding::new("right", StepForward, ctx),
        KeyBinding::new(".", StepForward, ctx),
        KeyBinding::new("home", Restart, ctx),
        KeyBinding::new("0", Restart, ctx),
        KeyBinding::new("f", ToggleSize, ctx),
        KeyBinding::new("escape", Close, ctx),
    ]);
}

/// The image a click asked for.
#[derive(Clone, Debug)]
pub enum ViewedImage {
    /// An `http(s)` image, as the rich text loaded it.
    Remote(SharedString),
    /// A pasted image in a draft (not uploaded yet).
    Local(Arc<Image>),
}

impl ViewedImage {
    pub fn url(&self) -> Option<&str> {
        match self {
            ViewedImage::Remote(url) => Some(url),
            ViewedImage::Local(_) => None,
        }
    }
}

pub enum ViewerEvent {
    Closed,
}

enum Load {
    Loading,
    Failed,
    Ready(Arc<RenderImage>),
}

pub struct ImageViewer {
    image: ViewedImage,
    focus: FocusHandle,
    on_open_url: crate::markdown::OpenUrl,
    /// The decoded frames, as the last render found them.
    loaded: Option<Arc<RenderImage>>,
    frame: usize,
    /// 0 until the image has loaded.
    frames: usize,
    playing: bool,
    /// Shows the next frame when its time is up (dropping it stops the animation).
    tick: Option<Task<()>>,
    /// At its own size rather than filling the window (only for an image smaller than the window).
    original: bool,
    /// Filling the window enlarges the image, as last painted (offering its own size then).
    enlarges: Rc<Cell<bool>>,
    /// Where the image and the scrubber were painted last frame (for clicks).
    painted: Rc<Cell<Bounds<Pixels>>>,
    track: Rc<Cell<Bounds<Pixels>>>,
    /// Dragging the scrubber; whether it was playing before.
    scrubbing: Option<bool>,
}

impl EventEmitter<ViewerEvent> for ImageViewer {}

/// How long a frame shows. As in browsers, GIF delays of 10 ms or less mean 100 ms.
fn frame_delay(data: &RenderImage, frame: usize) -> Duration {
    let delay = Duration::from(data.delay(frame));
    if delay <= Duration::from_millis(10) {
        Duration::from_millis(100)
    } else {
        delay
    }
}

impl ImageViewer {
    pub fn new(image: ViewedImage, on_open_url: crate::markdown::OpenUrl, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        ImageViewer {
            image,
            focus,
            on_open_url,
            loaded: None,
            frame: 0,
            frames: 0,
            playing: true,
            tick: None,
            original: false,
            enlarges: Rc::default(),
            painted: Rc::default(),
            track: Rc::default(),
            scrubbing: None,
        }
    }

    // ---------- for tests ----------

    pub fn image(&self) -> &ViewedImage {
        &self.image
    }

    /// The frame shown, of how many (0 until loaded).
    pub fn frame(&self) -> (usize, usize) {
        (self.frame, self.frames)
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Shown at its own size (rather than filling the window).
    pub fn original_size(&self) -> bool {
        self.original
    }

    /// Filling the window enlarges it, so its own size is on offer.
    pub fn enlarges(&self) -> bool {
        self.enlarges.get()
    }

    /// The scrubber's bounds as last painted.
    pub fn track_bounds(&self) -> Bounds<Pixels> {
        self.track.get()
    }

    /// The image's bounds as last painted.
    pub fn image_bounds(&self) -> Bounds<Pixels> {
        self.painted.get()
    }

    // ---------- playback ----------

    fn load(&self, window: &mut Window, cx: &mut App) -> Load {
        let data = match &self.image {
            ViewedImage::Remote(url) => match window.use_asset::<ImgResourceLoader>(&Resource::Uri(SharedUri::from(url.to_string())), cx) {
                None => return Load::Loading,
                Some(Err(_)) => return Load::Failed,
                Some(Ok(data)) => data,
            },
            ViewedImage::Local(image) => match image.clone().use_render_image(window, cx) {
                None => return Load::Loading,
                Some(data) => data,
            },
        };
        if data.frame_count() == 0 {
            Load::Failed
        } else {
            Load::Ready(data)
        }
    }

    /// Shows `frame` for its whole delay (restarting the clock).
    fn show(&mut self, frame: usize, cx: &mut Context<Self>) {
        self.frame = frame;
        self.tick = None;
        cx.notify();
    }

    fn schedule(&mut self, data: &Arc<RenderImage>, cx: &mut Context<Self>) {
        if !self.playing || self.frames < 2 || self.tick.is_some() || self.scrubbing.is_some() {
            return;
        }
        let delay = frame_delay(data, self.frame);
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                this.tick = None;
                if this.frames > 0 {
                    this.frame = (this.frame + 1) % this.frames;
                }
                cx.notify();
            });
        }));
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.frames < 2 {
            return;
        }
        self.playing = !self.playing;
        self.tick = None;
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.frames < 2 {
            return;
        }
        self.playing = false;
        let n = self.frames as isize;
        self.show(((self.frame as isize + delta).rem_euclid(n)) as usize, cx);
    }

    /// The frame showing at `fraction` of the animation's running time.
    fn frame_at(data: &RenderImage, frames: usize, fraction: f32) -> usize {
        let total: Duration = (0..frames).map(|i| frame_delay(data, i)).sum();
        let at = total.mul_f32(fraction.clamp(0., 1.));
        let mut start = Duration::ZERO;
        for i in 0..frames {
            start += frame_delay(data, i);
            if start > at {
                return i;
            }
        }
        frames - 1
    }

    fn seek_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(data) = self.loaded.clone() else { return };
        let track = self.track.get();
        if track.size.width <= px(0.) {
            return;
        }
        let fraction = (x - track.left()) / track.size.width;
        let frame = Self::frame_at(&data, self.frames, fraction);
        self.show(frame, cx);
    }

    pub fn toggle_size(&mut self, cx: &mut Context<Self>) {
        if self.enlarges.get() {
            self.original = !self.original;
            cx.notify();
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.tick = None;
        cx.emit(ViewerEvent::Closed);
    }

    fn stage_click(&mut self, event: &ClickEvent, cx: &mut Context<Self>) {
        // A click on an animation plays / pauses it; anywhere else closes the viewer.
        if self.frames > 1 && self.painted.get().contains(&event.position()) {
            self.toggle_play(cx);
        } else {
            self.close(cx);
        }
    }
}

impl Focusable for ImageViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn icon_button(id: &'static str, icon: &'static str, label: &'static str, color: Hsla) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .role(gpui_kit::Role::Button)
        .aria_label(label)
        .p(u(6.))
        .rounded(u(6.))
        .cursor_pointer()
        .hover(|d| d.bg(gpui_kit::white().opacity(0.12)))
        .child(svg().path(icon).size(u(16.)).text_color(color))
}

fn seconds(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f32())
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let light = gpui_kit::white().opacity(0.9);
        let muted = gpui_kit::white().opacity(0.6);
        let load = self.load(window, cx);
        self.loaded = match &load {
            Load::Ready(data) => Some(data.clone()),
            _ => None,
        };
        if let Load::Ready(data) = &load {
            self.frames = data.frame_count();
            self.frame = self.frame.min(self.frames - 1);
            self.schedule(data, cx);
        }

        let stage: AnyElement = match &load {
            Load::Ready(data) => {
                let (data, frame, painted, enlarges) = (data.clone(), self.frame, self.painted.clone(), self.enlarges.clone());
                let (original, viewer) = (self.original, cx.entity().downgrade());
                let natural = data.size(frame);
                let natural = size(px(natural.width.0 as f32), px(natural.height.0 as f32));
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let fill = (bounds.size.width / natural.width).min(bounds.size.height / natural.height);
                        // Whether its own size is on offer shows in the next frame (the window's size is only known now).
                        if (fill > 1.) != enlarges.get() {
                            enlarges.set(fill > 1.);
                            cx.defer(move |cx| {
                                let _ = viewer.update(cx, |_, cx| cx.notify());
                            });
                        }
                        let scale = if original { fill.min(1.) } else { fill };
                        let shown = size(natural.width * scale, natural.height * scale);
                        let at = point(bounds.center().x - shown.width / 2., bounds.center().y - shown.height / 2.);
                        let shown = Bounds::new(at, shown);
                        painted.set(shown);
                        let _ = window.paint_image(shown, shown, Corners::default(), data, frame, false);
                    },
                )
                .size_full()
                .into_any_element()
            }
            Load::Loading => div().text_color(muted).child("Loading…").into_any_element(),
            Load::Failed => div().text_color(muted).child("The image couldn't be loaded.").into_any_element(),
        };

        let controls = match &load {
            Load::Ready(data) if self.frames > 1 => {
                let frames = self.frames;
                let total: Duration = (0..frames).map(|i| frame_delay(data, i)).sum();
                let elapsed: Duration = (0..self.frame).map(|i| frame_delay(data, i)).sum();
                let fraction = if total.is_zero() { 0. } else { elapsed.as_secs_f32() / total.as_secs_f32() };
                let track = self.track.clone();
                let (play_icon, play_label) = if self.playing { ("icons/pause.svg", "Pause") } else { ("icons/play.svg", "Play") };
                Some(
                    div()
                        .id("image-viewer-controls")
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .w(u(640.))
                        .max_w(gpui_kit::relative(0.9))
                        .px(u(10.))
                        .py(u(6.))
                        .rounded(u(10.))
                        .bg(gpui_kit::black().opacity(0.6))
                        .text_size(u(12.))
                        .text_color(light)
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(icon_button("image-viewer-play", play_icon, play_label, light).on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_play(cx);
                        })))
                        .child(icon_button("image-viewer-back", "icons/step-back.svg", "Previous frame", light).on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.step(-1, cx);
                        })))
                        .child(icon_button("image-viewer-forward", "icons/step-forward.svg", "Next frame", light).on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.step(1, cx);
                        })))
                        .child(
                            div()
                                .id("image-viewer-track")
                                .flex_1()
                                .h(u(20.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.scrubbing = Some(this.playing);
                                        this.playing = false;
                                        this.seek_to(event.position.x, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .h(u(4.))
                                        .rounded_full()
                                        .bg(gpui_kit::white().opacity(0.25))
                                        .child(canvas(move |bounds, _, _| track.set(bounds), |_, _, _, _| {}).absolute().inset_0())
                                        .child(div().absolute().left_0().top_0().bottom_0().w(gpui_kit::relative(fraction)).rounded_full().bg(p.accent))
                                        .child(
                                            div()
                                                .absolute()
                                                .top(u(-4.))
                                                .left(gpui_kit::relative(fraction))
                                                .ml(u(-6.))
                                                .size(u(12.))
                                                .rounded_full()
                                                .bg(light),
                                        ),
                                ),
                        )
                        .child(div().flex_none().text_color(muted).child(format!("{} / {}", seconds(elapsed), seconds(total))))
                        .child(div().flex_none().text_color(muted).child(format!("{} / {frames}", self.frame + 1))),
                )
            }
            _ => None,
        };

        let url = self.image.url().map(str::to_string);
        let open_url = self.on_open_url.clone();
        div()
            .id("image-viewer")
            .key_context("ImageViewer")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(gpui_kit::black().opacity(0.88))
            .occlude()
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| this.toggle_play(cx)))
            .on_action(cx.listener(|this, _: &StepBack, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &StepForward, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &Restart, _, cx| {
                if this.frames > 1 {
                    this.show(0, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSize, _, cx| this.toggle_size(cx)))
            .on_action(cx.listener(|this, _: &Close, _, cx| this.close(cx)))
            // A drag that started on the scrubber goes on seeking wherever the mouse is.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.scrubbing.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.seek_to(event.position.x, cx);
                    } else {
                        this.playing = this.scrubbing.take().unwrap_or(false);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(playing) = this.scrubbing.take() {
                        this.playing = playing;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(u(12.))
                    .px(u(12.))
                    .py(u(8.))
                    .text_size(u(12.))
                    .text_color(muted)
                    .when(self.enlarges.get(), |d| {
                        let label = if self.original { "Fill the window" } else { "Original size" };
                        d.child(
                            div()
                                .id("image-viewer-size")
                                .flex()
                                .items_center()
                                .gap(u(4.))
                                .cursor_pointer()
                                .text_color(light)
                                .hover(|d| d.underline())
                                .child(label)
                                .child(kbd("F", cx))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_size(cx);
                                })),
                        )
                    })
                    .child(div().flex().items_center().gap(u(4.)).child(kbd("Esc", cx)).child("close"))
                    .when_some(url, |d, url| {
                        d.child(
                            div()
                                .id("image-viewer-browser")
                                .cursor_pointer()
                                .text_color(light)
                                .hover(|d| d.underline())
                                .child("Open in browser")
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    open_url(&url, window, cx);
                                }),
                        )
                    })
                    .child(icon_button("image-viewer-close", "icons/x.svg", "Close", light).on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                    }))),
            )
            .child(
                div()
                    .id("image-viewer-stage")
                    .flex_1()
                    .min_h(px(0.))
                    .mx(u(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| this.stage_click(event, cx)))
                    .child(stage),
            )
            .child(div().flex().justify_center().pt(u(8.)).pb(u(16.)).children(controls))
    }
}
