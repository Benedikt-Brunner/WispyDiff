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
            let body = w
                .painted_quads()
                .into_iter()
                .find(|q| {
                    (q.bounds.size.width.0 / scale - 1100.).abs() < 1.
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
