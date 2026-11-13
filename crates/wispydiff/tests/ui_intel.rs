//! Ports intel.e2e.ts through the production Root, keymap, text hit-testing and real IO.
mod ui_c_support;
use gpui_kit::{
    point, px, AppContext, InputEvent, Modifiers, MouseButton, NavigationDirection, ScrollDelta,
    ScrollWheelEvent, TestAppContext, VisualTestContext,
};
use ui_c_support::*;
use wispydiff::{
    backend::GrepEvent,
    diff::{layout::Mode, seg_text, UnifiedRow},
};

fn secondary() -> Modifiers {
    Modifiers {
        control: true,
        ..Default::default()
    }
}
fn click_word(ui: &Ui, cx: &mut TestAppContext, word: &str) {
    let (file, offset, column) = ui.diff(cx).read_with(cx, |d, _| {
        d.view
            .summary
            .files
            .iter()
            .enumerate()
            .find_map(|(file, _)| {
                (0..d.segment(file).rows).find_map(|o| match d.unified_at(file, o) {
                    UnifiedRow::Row(row) => {
                        let text = seg_text(&row.s);
                        text.find(word)
                            .map(|at| (file, o, text[..at].chars().count() as f32 + 1.))
                    }
                    _ => None,
                })
            })
            .unwrap()
    });
    ui.reveal_row(cx, file, offset);
    let at = ui.code_position(cx, file, offset, column);
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_move(at, None, secondary());
    v.simulate_click(at, secondary());
    ui.frame(cx);
}
fn wheel(ui: &Ui, cx: &mut TestAppContext, delta: f32) {
    cx.update_window(ui.window, |_, w, cx| {
        w.dispatch_event(
            ScrollWheelEvent {
                position: point(px(1250.), px(500.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    ui.frame(cx);
}
async fn computed(ui: &Ui, cx: &mut TestAppContext, name: &str) -> wispy_core::symbols::Usages {
    let d = ui.diff(cx);
    let (backend, id) = d.read_with(cx, |d, cx| {
        (d.session.read(cx).backend.clone(), d.view_id.clone())
    });
    backend.usages(id, name.into()).await.unwrap()
}
async fn click_last_unchanged(ui: &Ui, cx: &mut TestAppContext) -> wispy_core::symbols::Hit {
    click_word(ui, cx, "compute");
    ui.wait(cx, "usages panel", |cx| {
        ui.diff(cx).read(cx).code_panel().is_some()
    })
    .await;
    let hits = computed(ui, cx, "compute").await;
    assert!(!hits.changed.is_empty());
    assert!(!hits.unchanged.is_empty());
    let hit = hits.unchanged.last().unwrap().clone();
    ui.diff(cx).update(cx, |d, cx| {
        d.list.scroll_to(gpui_kit::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    ui.frame(cx);
    // The unvirtualized hit panel ends with the last hit and 12px padding (the whole-repo switch
    // is at the top). A native wheel reaches its end; the final unchanged hit is right above it.
    let start = std::time::Instant::now();
    loop {
        wheel(ui, cx, -1_000_000.);
        ui.click(cx, point(px(1200.), px(872.)));
        if ui.diff(cx).read_with(cx, |d, _| {
            d.flash.is_some_and(|f| f.file == hit.file as usize)
        }) {
            break;
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "last unchanged result was not clickable"
        );
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    hit
}

// e2e: shows usages ... ⌘-click ... definitions, changed and unchanged code
#[gpui_kit::test]
async fn secondary_click_symbol_sections_and_unchanged_jump(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 1).await;
    let name = ui.diff(cx).read_with(cx, |d, _| {
        d.view
            .rows
            .iter()
            .find_map(|row| {
                let t = seg_text(&row.s);
                t.split_once("public function ")
                    .and_then(|(_, s)| s.split_once('('))
                    .map(|(name, _)| name.to_string())
            })
            .unwrap()
    });
    click_word(&ui, cx, &name);
    ui.wait(cx, "definition usages panel", |cx| {
        ui.diff(cx).read(cx).code_panel().is_some()
    })
    .await;
    let definitions = computed(&ui, cx, &name).await;
    assert!(!definitions.definitions.is_empty());
    let hit = click_last_unchanged(&ui, cx).await;
    ui.diff(cx).read_with(cx, |d, _| {
        let flash = d.flash.expect("jump flashes target");
        assert_eq!(flash.file, hit.file as usize);
        if flash.mode == Mode::Split {
            let row = &d.test_split_c(flash.file).unwrap()[flash.from as usize - 1];
            assert_eq!(row.n, Some(hit.line));
            assert!(seg_text(&row.ns).contains(hit.text.trim()));
            assert_eq!(d.segment(flash.file).mode, Mode::Split);
        } else {
            assert!(matches!(
                d.unified_at(flash.file, flash.from),
                UnifiedRow::Row(r) if r.n == Some(hit.line)
            ));
        }
        let item = d
            .built
            .item_of_row(d.segment(flash.file).start + flash.from);
        let bounds = d.list.bounds_for_item(item).unwrap();
        let viewport = d.list.viewport_bounds();
        assert!(bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom());
    });
    ui.close(cx).await;
}

// e2e: goes back ... and forward again (mouse buttons, keys)
#[gpui_kit::test]
async fn jump_history_mouse_and_keyboard_roundtrip(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 1).await;
    let diff = ui.diff(cx);
    let before = diff.read_with(cx, |d, _| {
        (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item,
        )
    });
    click_last_unchanged(&ui, cx).await;
    let after = diff.read_with(cx, |d, _| {
        (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item,
        )
    });
    assert_ne!(before, after);
    let at = point(px(700.), px(400.));
    ui.click_button(cx, at, MouseButton::Navigate(NavigationDirection::Back));
    assert_eq!(
        diff.read_with(cx, |d, _| (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item
        )),
        before
    );
    ui.click_button(cx, at, MouseButton::Navigate(NavigationDirection::Forward));
    assert_eq!(
        diff.read_with(cx, |d, _| (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item
        )),
        after
    );
    ui.click_button(cx, at, MouseButton::Navigate(NavigationDirection::Forward));
    assert_eq!(
        diff.read_with(cx, |d, _| (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item
        )),
        after
    );
    ui.keys(cx, "ctrl-[");
    assert_eq!(
        diff.read_with(cx, |d, _| (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item
        )),
        before
    );
    ui.keys(cx, "alt-right");
    assert_eq!(
        diff.read_with(cx, |d, _| (
            d.list.logical_scroll_top().item_ix,
            d.list.logical_scroll_top().offset_in_item
        )),
        after
    );
    ui.close(cx).await;
}

// CodePanel's result data and individual hit bounds are private. Submit the real UI query,
// read the real service result, then activate its public Jump event (hit_row's click boundary).
async fn search_outside(f: &Fixture, ui: &Ui, cx: &mut TestAppContext) {
    ui.keys(cx, "/");
    ui.wait(cx, "search panel", |cx| {
        ui.diff(cx).read(cx).code_panel().is_some()
    })
    .await;
    ui.input(cx, "endblock");
    ui.keys(cx, "enter");
    let (stack, hi) = ui.diff(cx).read_with(cx, |d, _| (d.stack_id.clone(), d.hi));
    let hits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let received = hits.clone();
    f.backend
        .grep(stack, hi, "endblock".into(), true, move |event| {
            if let GrepEvent::Hits { hits } = event {
                received.lock().unwrap().extend(hits);
            }
        })
        .await
        .unwrap();
    let files = ui.paths(cx);
    let hit = hits
        .lock()
        .unwrap()
        .iter()
        .find(|hit| !files.contains(&hit.path))
        .expect("outside template")
        .clone();
    let panel = ui
        .diff(cx)
        .read_with(cx, |d, _| d.code_panel().unwrap().clone());
    panel.update(cx, |_, cx| {
        cx.emit(wispydiff::diff::code_panel::CodePanelEvent::Jump {
            path: hit.path.clone(),
            line: hit.line,
            file: None,
        })
    });
    ui.wait(cx, "read-only file loaded", |cx| {
        ui.diff(cx)
            .read(cx)
            .file_view
            .as_ref()
            .is_some_and(|v| v.lines.is_some())
    })
    .await;
    ui.diff(cx).read_with(cx, |d, _| {
        let view = d.file_view.as_ref().unwrap();
        assert_eq!(view.path, hit.path);
        assert_eq!((view.line, view.end), (hit.line, hit.line));
        assert!(!view.filtered);
        assert!(view.error.is_none());
    });
}

// e2e: searches the whole repo and opens files outside the diff read-only (load/close portion).
#[gpui_kit::test]
async fn repository_search_outside_file_readonly_and_close_button(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 1).await;
    search_outside(&f, &ui, cx).await;
    let top = ui
        .diff(cx)
        .read_with(cx, |d, _| d.list.viewport_bounds().top());
    // The modal is 1100px wide, centered, 90px below the diff's top; header padding is 14x8.
    ui.click(cx, point(px(1240.), top + px(109.)));
    assert!(ui.diff(cx).read_with(cx, |d, _| d.file_view.is_none()));
    ui.click(cx, point(px(1420.), top + px(24.)));
    assert!(ui.diff(cx).read_with(cx, |d, _| d.code_panel().is_none()));
    ui.close(cx).await;
}

// e2e: Escape closes the read-only view, then the code panel.
#[gpui_kit::test]
async fn repository_file_view_escape_preserves_search_panel(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 1).await;
    search_outside(&f, &ui, cx).await;
    ui.keys(cx, "escape");
    assert!(ui.diff(cx).read_with(cx, |d, _| d.file_view.is_none()));
    assert!(ui.diff(cx).read_with(cx, |d, _| d.code_panel().is_some()));
    ui.keys(cx, "escape");
    assert!(ui.diff(cx).read_with(cx, |d, _| d.code_panel().is_none()));
    ui.close(cx).await;
}

// e2e: Back closes the outside-file view; Forward reopens its highlighted line.
#[gpui_kit::test]
async fn repository_file_view_mouse_history(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 1).await;
    search_outside(&f, &ui, cx).await;
    let at = point(px(700.), px(100.));
    ui.click_button(cx, at, MouseButton::Navigate(NavigationDirection::Back));
    assert!(ui.diff(cx).read_with(cx, |d, _| d.file_view.is_none()));
    ui.click_button(cx, at, MouseButton::Navigate(NavigationDirection::Forward));
    ui.wait(cx, "forward reopens file", |cx| {
        ui.diff(cx)
            .read(cx)
            .file_view
            .as_ref()
            .is_some_and(|v| v.lines.is_some())
    })
    .await;
    ui.close(cx).await;
}
