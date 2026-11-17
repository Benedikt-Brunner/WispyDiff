//! The PR description modal: `i` / double-click in the diff, `i` / right-click in the inbox.
mod comments_support;
use comments_support::*;
use gpui_kit::{point, px, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext};
use wispydiff::backend::Backend;

fn described(ui: &Ui, cx: &mut TestAppContext) -> Option<Vec<(String, String)>> {
    ui.app.read_with(cx, |a, cx| a.described_prs(cx)).map(|prs| prs.into_iter().map(|p| (p.label, p.body_html)).collect())
}

fn labels(ui: &Ui, cx: &mut TestAppContext) -> Vec<String> {
    described(ui, cx).expect("the description is open").into_iter().map(|(label, _)| label).collect()
}

fn double_click(ui: &Ui, cx: &mut TestAppContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
    ui.frame(cx);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    for count in 1..=2 {
        let modifiers = Modifiers::default();
        v.simulate_event(MouseDownEvent { button: MouseButton::Left, position: at, modifiers, click_count: count, first_mouse: false });
        v.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers, click_count: count });
    }
    ui.frame(cx);
}

#[gpui_kit::test]
async fn i_shows_the_descriptions_of_the_prs_in_view(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    ui.open(cx, 2).await;

    ui.press(cx, "i");
    let shown = described(&ui, cx).expect("i opens the description");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, "wispy/fixture#2");
    assert!(shown[0].1.contains("<strong>2</strong>"), "GitHub's rendering: {}", shown[0].1);

    // ↓ scrolls it rather than the diff; i (or Esc) closes it again.
    ui.press(cx, "down");
    ui.press(cx, "i");
    assert!(described(&ui, cx).is_none());
    ui.press(cx, "i");
    ui.press(cx, "escape");
    assert!(described(&ui, cx).is_none());

    // A range shows each PR's, bottom first.
    ui.press(cx, "}");
    ui.wait(cx, |cx| ui.session.read(cx).shown.as_ref().is_some_and(|r| (r.range.lo, r.range.hi) == (1, 2))).await;
    ui.press(cx, "i");
    assert_eq!(labels(&ui, cx), ["wispy/fixture#2", "wispy/fixture#3"]);
    ui.press(cx, "escape");

    // Double-clicking a PR in the stack bar shows that PR's alone.
    let y = ui.diff(cx).read_with(cx, |d, _| d.list.viewport_bounds().top()) - px(20.);
    double_click(&ui, cx, point(px(172.), y));
    assert_eq!(labels(&ui, cx), ["wispy/fixture#1"]);
    ui.press(cx, "escape");

    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn the_inbox_shows_a_description_on_i_or_a_right_click(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;

    inbox_select(&ui, cx, 3);
    ui.press(cx, "i");
    let shown = described(&ui, cx).expect("i opens the description");
    assert_eq!(shown[0].0, "wispy/fixture#3");
    assert!(shown[0].1.contains("Fixes #103"), "{}", shown[0].1);
    ui.press(cx, "escape");
    assert!(described(&ui, cx).is_none());

    inbox_select(&ui, cx, 5);
    let row = inbox_selection_bounds(&ui, cx);
    ui.frame(cx);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    v.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    ui.frame(cx);
    assert_eq!(labels(&ui, cx), ["wispy/fixture#5"]);
    // Still in the inbox, which takes the keyboard again once it closes.
    ui.press(cx, "escape");
    assert!(described(&ui, cx).is_none());
    assert!(ui.session.read_with(cx, |s, _| s.home));

    drop(backend);
    ui.close(cx).await;
}
