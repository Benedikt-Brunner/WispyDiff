//! Images in rich text open in the image viewer; an animated one plays there with controls.
mod comments_support;
use std::io::Cursor;

use comments_support::*;
use gpui_kit::http_client::{AsyncBody, FakeHttpClient, Response};
use gpui_kit::{point, px, AppContext as _, Bounds, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, ImageBuffer, Rgba};
use wispydiff::backend::Backend;
use wispydiff::description::DescribedPr;
use wispydiff::image_viewer::ViewedImage;

const GIF: &str = "https://images.example/walkthrough.gif";

/// A 600×300 GIF of three frames, 100 / 200 / 300 ms.
fn animated_gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(Cursor::new(&mut bytes));
        encoder.set_repeat(Repeat::Infinite).unwrap();
        for (i, ms) in [100, 200, 300].into_iter().enumerate() {
            let buffer = ImageBuffer::from_pixel(600, 300, Rgba([60 * i as u8, 120, 200, 255]));
            encoder.encode_frame(Frame::from_parts(buffer, 0, 0, Delay::from_numer_denom_ms(ms, 1))).unwrap();
        }
    }
    bytes
}

fn serve_gif(cx: &mut TestAppContext) {
    let gif = animated_gif();
    let client = FakeHttpClient::create(move |request| {
        let gif = gif.clone();
        async move {
            Ok(if request.uri().to_string().starts_with("https://images.example/") {
                Response::builder().status(200).body(AsyncBody::from(gif)).unwrap()
            } else {
                Response::builder().status(404).body(AsyncBody::default()).unwrap()
            })
        }
    });
    cx.update(|cx| cx.set_http_client(client));
}

/// A description as GitHub renders one with an image: linked to itself.
fn describe(ui: &Ui, cx: &mut TestAppContext) {
    let pr = DescribedPr {
        label: "wispy/fixture#1".into(),
        title: "Images".into(),
        author: "octocat".into(),
        url: String::new(),
        body_html: format!(r#"<p><a target="_blank" rel="noopener noreferrer" href="{GIF}"><img src="{GIF}" alt="Walkthrough" style="max-width: 100%;"></a></p>"#),
        color: None,
        auto_merge: false,
        branches: String::new(),
        stats: None,
    };
    cx.update_window(ui.window, |_, window, cx| ui.app.update(cx, |app, cx| app.show_description(vec![pr], 0, window, cx))).unwrap();
    ui.frame(cx);
}

fn viewer<R>(ui: &Ui, cx: &mut TestAppContext, f: impl FnOnce(&wispydiff::image_viewer::ImageViewer) -> R) -> R {
    ui.app.read_with(cx, |app, cx| f(app.image_viewer().expect("the viewer is open").read(cx)))
}

fn shown(ui: &Ui, cx: &mut TestAppContext) -> (usize, usize) {
    viewer(ui, cx, |v| v.frame())
}

fn bounds(ui: &Ui, cx: &mut TestAppContext, f: impl FnOnce(&wispydiff::image_viewer::ImageViewer) -> Bounds<Pixels>) -> Bounds<Pixels> {
    ui.frame(cx);
    viewer(ui, cx, f)
}

#[gpui_kit::test]
async fn an_image_opens_in_the_viewer_and_an_animation_plays_there_under_control(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    serve_gif(cx);
    ui.home(cx).await;

    describe(&ui, cx);
    assert_eq!(ui.app.read_with(cx, |a, _| a.overlay_kind()), Some("description"));
    // The image loads inline first: the body's first block, at the modal's left edge.
    ui.wait(cx, |cx| cx.has_asset::<gpui_kit::ImgResourceLoader>(&gpui_kit::Resource::Uri(GIF.into()))).await;
    for _ in 0..5 {
        ui.frame(cx);
        cx.run_until_parked();
    }
    click(&ui, cx, point(px(480.), px(330.)));
    let image = viewer(&ui, cx, |v| v.image().clone());
    assert!(matches!(&image, ViewedImage::Remote(url) if url.as_ref() == GIF), "{image:?}");
    assert_eq!(ui.app.read_with(cx, |a, _| a.overlay_kind()), Some("description"), "over the description");

    // Loaded: all three frames, playing from the first.
    ui.wait(cx, |cx| ui.app.read(cx).image_viewer().is_some_and(|v| v.read(cx).frame().1 == 3)).await;
    assert_eq!(shown(&ui, cx), (0, 3));
    assert!(viewer(&ui, cx, |v| v.playing()));
    // As big as the window (1440×900) allows: the full width, less the margins, at its 2:1.
    ui.frame(cx);
    let image_bounds = bounds(&ui, cx, |v| v.image_bounds());
    assert!(image_bounds.size.width > px(1350.), "{image_bounds:?}");
    assert!((image_bounds.size.width / image_bounds.size.height - 2.).abs() < 0.01, "{image_bounds:?}");
    // That enlarges it, so its own size is on offer (F, or the button), and back.
    assert!(viewer(&ui, cx, |v| v.enlarges() && !v.original_size()));
    ui.press(cx, "f");
    let own = bounds(&ui, cx, |v| v.image_bounds());
    assert_eq!((own.size.width, own.size.height), (px(600.), px(300.)));
    assert_eq!(own.center(), image_bounds.center(), "still centred");
    ui.press(cx, "f");
    assert_eq!(bounds(&ui, cx, |v| v.image_bounds()), image_bounds);

    // Each frame shows for its own delay.
    cx.executor().advance_clock(std::time::Duration::from_millis(110));
    ui.frame(cx);
    assert_eq!(shown(&ui, cx).0, 1);
    cx.executor().advance_clock(std::time::Duration::from_millis(210));
    ui.frame(cx);
    assert_eq!(shown(&ui, cx).0, 2);

    // Space pauses; ← / → step a frame (wrapping).
    ui.press(cx, "space");
    assert!(!viewer(&ui, cx, |v| v.playing()));
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    ui.frame(cx);
    assert_eq!(shown(&ui, cx).0, 2, "paused");
    ui.press(cx, "right");
    assert_eq!(shown(&ui, cx).0, 0);
    ui.press(cx, "left");
    assert_eq!(shown(&ui, cx).0, 2);

    // Clicking the image plays it again.
    click(&ui, cx, image_bounds.center());
    assert!(viewer(&ui, cx, |v| v.playing()));
    ui.press(cx, "space");

    // The scrubber seeks by time: 100 + 200 + 300 ms, so its middle (300 ms) is the last frame's start.
    let track = bounds(&ui, cx, |v| v.track_bounds());
    assert!(track.size.width > px(100.), "{track:?}");
    click(&ui, cx, point(track.left() + track.size.width * 0.1, track.center().y));
    assert_eq!(shown(&ui, cx).0, 0);
    click(&ui, cx, point(track.left() + track.size.width * 0.3, track.center().y));
    assert_eq!(shown(&ui, cx).0, 1);
    // Dragging goes on seeking; letting go leaves it as it was (paused).
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_down(point(track.left() + px(2.), track.center().y), MouseButton::Left, Modifiers::default());
    v.simulate_mouse_move(point(track.left() + track.size.width * 0.9, track.center().y - px(80.)), Some(MouseButton::Left), Modifiers::default());
    assert_eq!(shown(&ui, cx).0, 2);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_up(point(track.left() + track.size.width * 0.9, track.center().y - px(80.)), MouseButton::Left, Modifiers::default());
    ui.frame(cx);
    assert!(!viewer(&ui, cx, |v| v.playing()));
    assert!(ui.app.read_with(cx, |a, _| a.image_viewer().is_some()), "a drag doesn't close it");

    // Esc goes back to the description, which keeps the keyboard; a click beside the image closes it too.
    ui.press(cx, "escape");
    assert!(ui.app.read_with(cx, |a, _| a.image_viewer().is_none()));
    assert_eq!(ui.app.read_with(cx, |a, _| a.overlay_kind()), Some("description"));
    click(&ui, cx, point(px(480.), px(330.)));
    assert!(ui.app.read_with(cx, |a, _| a.image_viewer().is_some()));
    let beside = bounds(&ui, cx, |v| v.image_bounds());
    click(&ui, cx, point(beside.center().x, beside.top() - px(10.)));
    assert!(ui.app.read_with(cx, |a, _| a.image_viewer().is_none()));
    ui.press(cx, "escape");
    assert_eq!(ui.app.read_with(cx, |a, _| a.overlay_kind()), None, "Esc then closes the description");

    drop(backend);
    ui.close(cx).await;
}

/// Every image of a description opens in the viewer: GitHub's tables, paragraphs, a cell beside text.
#[gpui_kit::test]
async fn every_image_opens_wherever_it_is(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    serve_gif(cx);
    ui.home(cx).await;
    let img = |n: &str| format!(r#"<a target="_blank" rel="noopener noreferrer" href="https://images.example/{n}"><img src="https://images.example/{n}" alt="{n}" style="max-width: 100%;"></a>"#);
    let body = format!(
        "<markdown-accessiblity-table><table><thead><tr><th>Before</th><th>After</th></tr></thead><tbody><tr><td>{}</td><td>{}</td></tr><tr><td>text</td><td>{}</td></tr></tbody></table></markdown-accessiblity-table><p>{}</p>",
        img("c"), img("d"), img("e"), img("a")
    );
    let pr = DescribedPr { label: "wispy/fixture#1".into(), title: "Images".into(), author: "octocat".into(), url: String::new(), body_html: body, color: None, auto_merge: false, branches: String::new(), stats: None };
    let open = |ui: &Ui, cx: &mut TestAppContext| {
        cx.update_window(ui.window, |_, window, cx| ui.app.update(cx, |app, cx| app.show_description(vec![pr.clone()], 0, window, cx))).unwrap();
        for _ in 0..5 {
            ui.frame(cx);
            cx.run_until_parked();
        }
    };
    open(&ui, cx);
    // Click down the modal; note which image each click opens.
    let mut opened = std::collections::BTreeSet::new();
    for y in (240..800).step_by(40) {
        for x in [400, 900] {
            click(&ui, cx, point(px(x as f32), px(y as f32)));
            if let Some(ViewedImage::Remote(url)) = ui.app.read_with(cx, |a, cx| a.image_viewer().map(|v| v.read(cx).image().clone())) {
                opened.insert(url.rsplit('/').next().unwrap().to_string());
                ui.press(cx, "escape");
            }
            if ui.app.read_with(cx, |a, _| a.overlay_kind()).is_none() {
                open(&ui, cx);
            }
        }
    }
    assert_eq!(opened.into_iter().collect::<Vec<_>>(), ["a", "c", "d", "e"]);

    drop(backend);
    ui.close(cx).await;
}
