//! Pointer selection through the production root, including the read-only head-file overlay.
mod ui_c_support;
use gpui_kit::{
    point, px, AppContext, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
    VisualTestContext,
};
use ui_c_support::*;
use wispydiff::diff::{seg_text, UnifiedRow};

fn copy(ui: &Ui, cx: &mut TestAppContext) -> String {
    ui.keys(
        cx,
        if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        },
    );
    cx.update(|cx| {
        cx.read_from_clipboard()
            .expect("copied selection")
            .text()
            .unwrap()
    })
}

#[gpui_kit::test]
async fn diff_double_click_selects_word_and_shift_click_extends_same_column(
    cx: &mut TestAppContext,
) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 2).await;
    let (file, offset, text, next, at) = ui.diff(cx).read_with(cx, |d, _| {
        d.built
            .segments
            .iter()
            .find_map(|s| {
                (1..s.rows.saturating_sub(1)).find_map(|o| {
                    let (UnifiedRow::Row(row), UnifiedRow::Row(next)) =
                        (d.unified_at(s.file, o), d.unified_at(s.file, o + 1))
                    else {
                        return None;
                    };
                    let text = seg_text(&row.s);
                    let at = text.find("function")?;
                    let next = seg_text(&next.s);
                    (next.len() >= 8).then_some((s.file, o, text, next, at))
                })
            })
            .expect("fixture has a function followed by code")
    });
    ui.reveal_row(cx, file, offset);
    let at_word = ui.code_position(cx, file, offset, at as f32 + 2.);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_event(MouseDownEvent {
        position: at_word,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    v.simulate_event(MouseUpEvent {
        position: at_word,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    ui.frame(cx);
    let original = ui.diff(cx).read_with(cx, |d, _| {
        d.text_selection
            .clone()
            .expect("word selected after mouse up")
    });
    assert_eq!(original.text, "function");
    assert_eq!(
        (original.anchor.byte, original.head.byte),
        (at, at + "function".len())
    );
    assert!(!original.active);
    assert_eq!(copy(&ui, cx), "function");

    let end = ui.code_position(cx, file, offset + 1, 8.);
    VisualTestContext::from_window(ui.window, cx).simulate_click(
        end,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    ui.frame(cx);
    let expected = format!("{}\n{}", &text[at..], &next[..8]);
    ui.diff(cx).read_with(cx, |d, _| {
        let selection = d.text_selection.as_ref().unwrap();
        assert_eq!(selection.anchor, original.anchor);
        assert_eq!(
            (selection.head.offset, selection.head.byte),
            (offset + 1, 8)
        );
        assert_eq!(selection.side, original.side);
        assert_eq!(selection.text, expected);
        assert!(!selection.active);
    });
    assert_eq!(copy(&ui, cx), expected);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn file_view_drag_copies_original_tabs_and_shift_click_extends(cx: &mut TestAppContext) {
    let f = Fixture::new();
    // Add an unchanged tab-indented file to both sides of the standalone PR, entirely in
    // this test's disposable origin. It is present at the head but absent from the diff.
    const PATH: &str = "selection.txt";
    const CONTENT: &str = "header\n\talpha = 1;\n\t\tbeta = 2;\n\tgamma = 3;\n";
    f.write(PATH, CONTENT);
    f.git(&["add", PATH]);
    f.git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.test",
        "commit",
        "--quiet",
        "-m",
        "unchanged selection fixture",
    ]);
    f.git(&["checkout", "--quiet", "solo"]);
    f.git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.test",
        "merge",
        "--quiet",
        "--no-edit",
        "main",
    ]);
    f.git(&["update-ref", "refs/pull/5/head", "solo"]);
    f.git(&["checkout", "--quiet", "main"]);

    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    assert!(!ui.paths(cx).iter().any(|p| p == PATH));
    let diff = ui.diff(cx);
    cx.update_window(ui.window, |_, w, cx| {
        diff.update(cx, |d, cx| {
            d.open_file_view(PATH.into(), 1, 1, false, w, cx)
        })
    })
    .unwrap();
    ui.wait(cx, "head-file contents", |cx| {
        diff.read(cx)
            .file_view
            .as_ref()
            .is_some_and(|v| v.lines.is_some())
    })
    .await;
    let (body, cw) = cx
        .update_window(ui.window, |_, w, cx| {
            let scale = w.scale_factor();
            // The modal fills the window but for a 24px margin.
            let width = f32::from(w.viewport_size().width) - 48.;
            let body = w
                .painted_quads()
                .into_iter()
                .find(|q| {
                    (q.bounds.size.width.0 / scale - width).abs() < 1.
                        && q.bounds.size.height.0 / scale > 400.
                })
                .expect("painted file-view body");
            (
                point(
                    px(body.bounds.left().0 / scale),
                    px(body.bounds.top().0 / scale),
                ),
                diff.update(cx, |d, cx| d.char_width(w, cx)),
            )
        })
        .unwrap();
    // Modal border + three-digit gutter + text padding. Header: 13px * 1.5 + 16px
    // padding + border. Rows are 20px, with the first line at the top of this view.
    let position = |line: usize, column: f32| {
        point(
            body.x + px(1. + 3. * 7.5 + 16. + 8.) + cw * column,
            body.y + px(1. + 36.5 + 20. * line as f32 + 10.),
        )
    };
    let (from, to) = (position(1, 0.), position(2, 20.));
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_down(from, MouseButton::Left, Default::default());
    v.simulate_mouse_move(to, MouseButton::Left, Default::default());
    v.simulate_mouse_up(to, MouseButton::Left, Default::default());
    ui.frame(cx);
    let expected = "\talpha = 1;\n\t\tbeta";
    let anchor = diff.read_with(cx, |d, _| {
        let view = d.file_view.as_ref().unwrap();
        assert!(!view.filtered);
        let selection = view.selection.expect("file text selected");
        assert!(!selection.active);
        assert_eq!(selection.anchor, (1, 0));
        assert_eq!(selection.head, (2, 20));
        assert_eq!(view.selected_text().as_deref(), Some(expected));
        selection.anchor
    });
    assert_eq!(copy(&ui, cx), expected);

    VisualTestContext::from_window(ui.window, cx).simulate_click(
        position(3, 13.),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    ui.frame(cx);
    let expected = "\talpha = 1;\n\t\tbeta = 2;\n\tgamma";
    diff.read_with(cx, |d, _| {
        let view = d.file_view.as_ref().unwrap();
        let selection = view.selection.unwrap();
        assert_eq!(selection.anchor, anchor);
        assert_eq!(selection.head, (3, 13));
        assert!(!selection.active);
        assert_eq!(view.selected_text().as_deref(), Some(expected));
    });
    assert_eq!(copy(&ui, cx), expected);
    drop(diff);
    ui.close(cx).await;
}

/// A line far longer than the window: text scrolled into view sideways selects like any other
/// (double click: a word, triple click: the whole line), and dragging past the code's right
/// edge scrolls it, the selection following.
#[gpui_kit::test]
async fn long_lines_select_after_scrolling_sideways_and_drag_scrolls_them(cx: &mut TestAppContext) {
    let f = Fixture::new();
    const PATH: &str = "long.txt";
    let line: String = (0..120).map(|i| format!("word{i:03}")).collect::<Vec<_>>().join(" ");
    f.git(&["checkout", "--quiet", "solo"]);
    f.write(PATH, &format!("{line}\n"));
    f.git(&["add", PATH]);
    f.git(&["-c", "user.name=Test", "-c", "user.email=test@example.test", "commit", "--quiet", "-m", "a long line"]);
    f.git(&["update-ref", "refs/pull/5/head", "solo"]);
    f.git(&["checkout", "--quiet", "main"]);

    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    let diff = ui.diff(cx);
    let (file, offset) = diff.read_with(cx, |d, _| {
        let file = d.view.summary.files.iter().position(|f| f.path == PATH).expect("long file in the diff");
        let offset = (0..d.segment(file).rows)
            .find(|&o| matches!(d.unified_at(file, o), UnifiedRow::Row(r) if seg_text(&r.s) == line))
            .expect("the long line's row");
        (file, offset)
    });
    ui.reveal_row(cx, file, offset);
    let cw = cx.update_window(ui.window, |_, w, cx| diff.update(cx, |d, cx| d.char_width(w, cx))).unwrap();
    let viewport = diff.read_with(cx, |d, _| d.list.viewport_bounds());

    // Scroll sideways with the wheel, far past where the row's own box used to end.
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_event(gpui_kit::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui_kit::ScrollDelta::Pixels(point(-(cw * 400.), px(0.))),
        ..Default::default()
    });
    ui.frame(cx);
    let scrolled = diff.read_with(cx, |d, _| d.scroll_x);
    assert!(scrolled > cw * 300., "scrolled sideways: {scrolled:?}");
    let shifted = (scrolled / cw).round() as usize;

    // A double click on a word now in view: column 20 of the cell is that much further on.
    let at = ui.code_position(cx, file, offset, 20.5);
    for clicks in [2, 3] {
        let mut v = VisualTestContext::from_window(ui.window, cx);
        v.simulate_event(MouseDownEvent { position: at, button: MouseButton::Left, click_count: clicks, ..Default::default() });
        v.simulate_event(MouseUpEvent { position: at, button: MouseButton::Left, click_count: clicks, ..Default::default() });
        ui.frame(cx);
        let selection = diff.read_with(cx, |d, _| d.text_selection.clone().expect("selected after scrolling sideways"));
        if clicks == 2 {
            let column = shifted + 20;
            let start = line[..column].rfind(' ').map_or(0, |i| i + 1);
            let end = line[column..].find(' ').map_or(line.len(), |i| column + i);
            assert_eq!(selection.text, line[start..end], "the word under the pointer");
        } else {
            assert_eq!(selection.text, line, "a triple click selects the whole line");
        }
    }

    // Back to the start, then drag from the line's start past the code's right edge.
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_event(gpui_kit::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui_kit::ScrollDelta::Pixels(point(cw * 1000., px(0.))),
        ..Default::default()
    });
    ui.frame(cx);
    assert_eq!(diff.read_with(cx, |d, _| d.scroll_x), px(0.));
    let from = ui.code_position(cx, file, offset, 0.);
    // Inside the edge zone at the right, where it scrolls slowly.
    let past = point(viewport.right() - px(4.), from.y);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_down(from, MouseButton::Left, Default::default());
    v.simulate_mouse_move(past, MouseButton::Left, Default::default());
    ui.frame(cx);
    let visible_columns = ((viewport.right() - from.x) / cw) as usize;
    ui.wait(cx, "dragging past the right edge scrolls sideways", |cx| {
        let d = diff.read(cx);
        d.scroll_x > cw * 20. && d.text_selection.as_ref().is_some_and(|s| s.head.byte > visible_columns + 10)
    })
    .await;
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_up(past, MouseButton::Left, Default::default());
    ui.frame(cx);
    diff.read_with(cx, |d, _| {
        let selection = d.text_selection.as_ref().unwrap();
        assert!(!selection.active);
        assert_eq!(selection.anchor.byte, 0);
        assert!(line.starts_with(&selection.text) && selection.text.len() > visible_columns);
    });
    ui.close(cx).await;
}
