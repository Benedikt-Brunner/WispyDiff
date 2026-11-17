//! The PR description modal: `i` / double-click in the diff, `i` / right-click in the inbox.
mod comments_support;
use comments_support::*;
use gpui_kit::{point, px, AppContext as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext};
use wispydiff::backend::Backend;

fn described(ui: &Ui, cx: &mut TestAppContext) -> Option<Vec<(String, String)>> {
    ui.app.read_with(cx, |a, cx| a.described_prs(cx)).map(|prs| prs.into_iter().map(|p| (p.label, p.body_html)).collect())
}

fn labels(ui: &Ui, cx: &mut TestAppContext) -> Vec<String> {
    described(ui, cx).expect("the description is open").into_iter().map(|(label, _)| label).collect()
}

const STACK: [&str; 4] = ["wispy/fixture#1", "wispy/fixture#2", "wispy/fixture#3", "wispy/fixture#4"];

/// The PR at the top of the modal's viewport, once it has laid out.
fn top_pr(ui: &Ui, cx: &mut TestAppContext) -> usize {
    ui.frame(cx);
    ui.frame(cx);
    ui.app.read_with(cx, |a, cx| {
        let view = a.description_view().expect("the description is open").read(cx);
        let (scrolled, max) = view.scrolled();
        assert!(max > px(0.), "the stack's descriptions overflow the modal");
        assert!(scrolled <= max);
        view.top_pr().unwrap()
    })
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
async fn i_shows_the_stacks_descriptions_scrolled_to_the_pr_in_view(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    ui.open(cx, 2).await;

    ui.press(cx, "i");
    let shown = described(&ui, cx).expect("i opens the description");
    assert_eq!(shown.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), STACK, "every PR of the stack, bottom first");
    assert!(shown[1].1.contains("<strong>2</strong>"), "GitHub's rendering: {}", shown[1].1);
    assert_eq!(top_pr(&ui, cx), 1, "#2 starts at the top");

    // ↓ scrolls it rather than the diff; i (or Esc) closes it again.
    ui.press(cx, "down");
    ui.press(cx, "i");
    assert!(described(&ui, cx).is_none());
    ui.press(cx, "i");
    ui.press(cx, "escape");
    assert!(described(&ui, cx).is_none());

    // A range scrolls to its lowest PR.
    ui.press(cx, "]");
    ui.wait(cx, |cx| ui.session.read(cx).shown.as_ref().is_some_and(|r| (r.range.lo, r.range.hi) == (2, 2))).await;
    ui.press(cx, "}");
    ui.wait(cx, |cx| ui.session.read(cx).shown.as_ref().is_some_and(|r| (r.range.lo, r.range.hi) == (2, 3))).await;
    ui.press(cx, "i");
    assert_eq!(labels(&ui, cx), STACK);
    assert_eq!(top_pr(&ui, cx), 2, "#3 starts at the top");
    ui.press(cx, "escape");

    // Double-clicking a PR in the stack bar scrolls to that one.
    let y = ui.diff(cx).read_with(cx, |d, _| d.list.viewport_bounds().top()) - px(20.);
    double_click(&ui, cx, point(px(172.), y));
    assert_eq!(labels(&ui, cx), STACK);
    assert_eq!(top_pr(&ui, cx), 0, "#1 starts at the top");
    ui.press(cx, "escape");

    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn the_description_can_be_selected_and_copied(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    ui.open(cx, 2).await;
    ui.press(cx, "i");
    assert!(described(&ui, cx).is_some());

    // Drag from the left of the body's first line to the right of its last (the modal sits
    // centred below the top, 820 wide, the body below the PR's heading).
    ui.frame(cx);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    let (from, to) = (point(px(335.), px(245.)), point(px(1100.), px(275.)));
    v.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    v.simulate_mouse_move(point(px(700.), px(260.)), Some(MouseButton::Left), Modifiers::default());
    v.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    v.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    ui.frame(cx);
    assert!(described(&ui, cx).is_some(), "selecting doesn't close it");
    let selected = cx.update_window(ui.window, |_, window, cx| gpui_kit::base::TextSelection::selected_text(window, cx)).unwrap();
    assert!(selected.contains("of the fixture stack"), "selected: {selected:?}");

    ui.press(cx, secondary("c"));
    let copied = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    assert!(copied.contains("of the fixture stack"), "copied: {copied:?}");

    // A click outside still closes it.
    click(&ui, cx, point(px(40.), px(850.)));
    assert!(described(&ui, cx).is_none());

    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn the_inbox_shows_a_description_on_i_or_a_right_click(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(fixture.data.path().into(), fixture.api(), "fixture-token".into());
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;

    // A PR of a stack: the stack's descriptions, scrolled to that PR.
    inbox_select(&ui, cx, 3);
    ui.press(cx, "i");
    let shown = described(&ui, cx).expect("i opens the description");
    assert_eq!(shown.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), STACK);
    assert!(shown[2].1.contains("Fixes #103"), "{}", shown[2].1);
    assert_eq!(top_pr(&ui, cx), 2);
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
