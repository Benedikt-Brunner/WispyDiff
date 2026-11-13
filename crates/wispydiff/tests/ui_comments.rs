//! Ports comments.e2e.ts against the production Root → WispyApp → DiffViewer.
mod comments_support;
use comments_support::*;
use gpui_kit::test::TestAppContextExt;
use gpui_kit::{
    point, px, AppContext, Entity, ListOffset, MouseButton, TestAppContext, VisualTestContext,
};
use wispy_core::model::row_kind;
use wispydiff::{
    backend::Backend,
    diff::{CommentKind, DiffViewer, UnifiedRow},
    logic::layout::Mode,
};

fn added(diff: &DiffViewer, nth: usize) -> (usize, u32, String) {
    diff.built
        .segments
        .iter()
        .filter(|s| s.mode == Mode::Unified)
        .flat_map(|s| {
            (0..s.rows).filter_map(move |offset| match diff.unified_at(s.file, offset) {
                UnifiedRow::Row(row) if row.k == row_kind::ADDED => {
                    Some((s.file, offset, wispydiff::diff::seg_text(&row.s)))
                }
                _ => None,
            })
        })
        .nth(nth)
        .expect("added line")
}
fn reveal(ui: &Ui, diff: &Entity<DiffViewer>, row: u32, cx: &mut TestAppContext) {
    diff.update(cx, |d, cx| {
        d.scroll_to_row(row, 0, None, cx);
    });
    ui.frame(cx);
}
fn gutter(
    ui: &Ui,
    diff: &Entity<DiffViewer>,
    file: usize,
    from: u32,
    to: u32,
    cx: &mut TestAppContext,
) {
    let row = diff.read_with(cx, |d, _| d.segment(file).start + from);
    reveal(ui, diff, row, cx);
    let (a, b) = diff.read_with(cx, |d, _| {
        let pos = |offset| {
            let item = d.built.item_of_row(d.segment(file).start + offset);
            let bounds = d.list.bounds_for_item(item).expect("painted gutter row");
            assert!(d.list.viewport_bounds().contains(&bounds.center()));
            point(bounds.left() + px(25.), bounds.center().y)
        };
        (pos(from), pos(to))
    });
    let mut v = VisualTestContext::from_window(ui.window, cx);
    v.simulate_mouse_move(a, None, Default::default());
    v.simulate_mouse_down(a, MouseButton::Left, Default::default());
    if a != b {
        v.simulate_mouse_move(b, MouseButton::Left, Default::default());
    }
    v.simulate_mouse_up(b, MouseButton::Left, Default::default());
}
async fn draft(ui: &Ui, diff: &Entity<DiffViewer>, body: &str, cx: &mut TestAppContext) {
    let (file, offset, _) = diff.read_with(cx, |d, _| added(d, 0));
    gutter(ui, diff, file, offset, offset, cx);
    ui.wait(cx, |cx| diff.read(cx).composer.is_some()).await;
    ui.input(cx, body);
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        diff.read(cx).composer.is_none()
            && ui
                .session
                .read(cx)
                .drafts
                .iter()
                .any(|d| d.draft.body == body)
    })
    .await;
}

#[gpui_kit::test]
async fn teammate_thread_is_inline_and_uses_github_html(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    ui.threads(cx).await;
    ui.wait(cx, |cx| {
        diff.read(cx)
            .items_by_key
            .values()
            .any(|i| matches!(i.kind, CommentKind::Thread { .. }))
    })
    .await;
    diff.read_with(cx, |d, _| {
        let thread = d
            .items_by_key
            .values()
            .find_map(|i| match &i.kind {
                CommentKind::Thread { thread, .. } => Some(thread),
                _ => None,
            })
            .unwrap();
        assert_eq!(thread.comments[0].author, "teammate");
        assert_eq!(thread.comments[0].body, "Is this **threshold** right?");
        assert_eq!(
            thread.comments[0].body_html,
            "<p>Is this <strong>threshold</strong> right?</p>"
        );
        assert!(d
            .built
            .inserts
            .iter()
            .any(|i| i.key == format!("thread:{}", thread.id)));
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn drafts_changed_line_from_real_gutter(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    draft(&ui, &diff, "Why this threshold?", cx).await;
    ui.drafts(cx, 1).await;
    ui.session.read_with(cx, |s, _| {
        let d = &s.drafts[0];
        assert_eq!(d.pr_index, 1);
        assert!(d.line.is_some());
        assert!(d.start_line.is_none());
        assert_eq!(d.draft.body, "Why this threshold?");
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn hides_individual_threads_and_toggles_pr_threads_with_h(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    ui.threads(cx).await;
    ui.wait(cx, |cx| !diff.read(cx).items_by_key.is_empty())
        .await;
    let id = ui.session.read_with(cx, |s, _| {
        s.threads
            .iter()
            .flat_map(|p| &p.threads)
            .next()
            .unwrap()
            .id
            .clone()
    });
    diff.update(cx, |d, cx| d.hide_thread(&id, cx));
    ui.wait(cx, |cx| {
        diff.read(cx)
            .marks
            .values()
            .any(|m| m.threads.iter().any(|t| t.id == id))
    })
    .await;
    diff.read_with(cx, |d, _| {
        assert!(!d
            .built
            .inserts
            .iter()
            .any(|i| i.key == format!("thread:{id}")))
    });
    diff.update(cx, |d, cx| d.show_threads(&[id.clone()], cx));
    ui.wait(cx, |cx| diff.read(cx).marks.is_empty()).await;
    ui.press(cx, "h");
    ui.wait(cx, |cx| !diff.read(cx).marks.is_empty()).await;
    assert_eq!(
        backend.prefs().get_list("hiddenCommentPrs"),
        vec!["wispy/fixture#2"]
    );
    ui.press(cx, "h");
    ui.wait(cx, |cx| diff.read(cx).marks.is_empty()).await;
    assert!(backend.prefs().get_list("hiddenCommentPrs").is_empty());
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn collapsed_file_retains_open_thread_tally(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    ui.threads(cx).await;
    let file = diff.read_with(cx, |d, _| {
        d.view
            .summary
            .files
            .iter()
            .position(|f| f.path == "src/Module0/Service0.php")
            .unwrap()
    });
    ui.wait(cx, |cx| {
        diff.read(cx)
            .tallies
            .get(&file)
            .is_some_and(|t| t.get("open") == Some(&1))
    })
    .await;
    diff.update(cx, |d, cx| d.go_to_file(file, true, cx));
    ui.press(cx, "e");
    diff.read_with(cx, |d, _| {
        assert_eq!(d.segment(file).mode, Mode::Collapsed);
        assert_eq!(d.tallies[&file]["open"], 1);
        assert!(!d.built.inserts.iter().any(|p| p.key.starts_with("thread:")));
    });
    diff.update(cx, |d, cx| d.go_to_file(file, true, cx));
    ui.wait(cx, |cx| {
        diff.read(cx)
            .built
            .inserts
            .iter()
            .any(|p| p.key.starts_with("thread:"))
    })
    .await;
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn suggestion_caret_indent_outdent_and_indented_enter(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    let (file, offset, line) = diff.read_with(cx, |d, _| added(d, 1));
    gutter(&ui, &diff, file, offset, offset, cx);
    ui.wait(cx, |cx| {
        diff.read(cx)
            .composer
            .as_ref()
            .is_some_and(|(t, _)| t.fallback == Some(false))
    })
    .await;
    ui.input(cx, "Simpler:");
    ui.press(cx, secondary("g"));
    let state = |cx: &gpui_kit::App| {
        let composer = &diff.read(cx).composer.as_ref().unwrap().1;
        let input = composer.read(cx).input.read(cx);
        (input.value().to_string(), input.selected_range())
    };
    let expected = format!("Simpler:\n```suggestion\n{line}\n```");
    ui.wait(cx, |cx| state(cx).0 == expected).await;
    let caret = expected.len() - 4;
    cx.read(|cx| assert_eq!(state(cx).1, caret..caret));
    ui.press(cx, "tab");
    let indented = format!("Simpler:\n```suggestion\n    {line}\n```");
    cx.read(|cx| {
        assert_eq!(state(cx).0, indented);
        assert_eq!(state(cx).1, caret + 4..caret + 4);
    });
    ui.press(cx, "shift-tab");
    cx.read(|cx| {
        assert_eq!(state(cx).0, expected);
        assert_eq!(state(cx).1, caret..caret);
    });
    ui.press(cx, "tab enter");
    let lead: String = line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let broken = format!("Simpler:\n```suggestion\n    {line}\n    {lead}\n```");
    cx.read(|cx| {
        assert_eq!(state(cx).0, broken);
        assert_eq!(state(cx).1, broken.len() - 4..broken.len() - 4);
    });
    ui.press(cx, "escape");
    ui.wait(cx, |cx| diff.read(cx).composer.is_none()).await;
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn composer_survives_reflow_and_virtualization(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    diff.update(cx, |d, cx| d.set_sidebar_width("files", Some(800.), cx));
    ui.press(cx, "z");
    let (file, offset, _) = diff.read_with(cx, |d, _| added(d, 10));
    gutter(&ui, &diff, file, offset, offset, cx);
    ui.input(cx, "Half-written thought");
    let input = diff.read_with(cx, |d, cx| {
        d.composer.as_ref().unwrap().1.read(cx).input.clone()
    });
    for _ in 0..2 {
        ui.press(cx, secondary("b"));
        assert_eq!(
            input.read_with(cx, |i, _| i.value().to_string()),
            "Half-written thought"
        );
    }
    let before = diff.read_with(cx, |d, _| d.list.logical_scroll_top());
    diff.update(cx, |d, cx| {
        d.list.scroll_to(ListOffset {
            item_ix: d.built.items.len() - 1,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    ui.frame(cx);
    assert_eq!(
        input.read_with(cx, |i, _| i.value().to_string()),
        "Half-written thought"
    );
    diff.update(cx, |d, cx| {
        d.list.scroll_to(before);
        cx.notify();
    });
    ui.frame(cx);
    diff.read_with(cx, |d, cx| {
        assert_eq!(d.composer.as_ref().unwrap().1.read(cx).input, input)
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn range_gutter_drag_shift_enter_then_enter_saves(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    let (file,from) = diff.read_with(cx,|d,_|d.built.segments.iter().find_map(|s| {
        (0..s.rows.saturating_sub(2)).find(|o|(0..3).all(|n| matches!(d.unified_at(s.file,o+n),UnifiedRow::Row(r) if r.k==row_kind::CONTEXT)))
            .map(|o|(s.file,o))
    }).expect("three context lines"));
    gutter(&ui, &diff, file, from, from + 2, cx);
    let (start, end) = diff.read_with(cx, |d, _| {
        let t = &d.composer.as_ref().unwrap().0;
        (t.start_line.unwrap(), t.line.unwrap())
    });
    assert_eq!(end - start, 2);
    ui.input(cx, "These three lines");
    ui.press(cx, "shift-enter");
    assert!(diff.read_with(cx, |d, _| d.composer.is_some()));
    ui.press(cx, "enter");
    ui.drafts(cx, 1).await;
    ui.session.read_with(cx, |s, _| {
        assert_eq!(s.drafts[0].start_line, Some(start));
        assert_eq!(s.drafts[0].line, Some(end));
        assert_eq!(s.drafts[0].draft.body, "These three lines");
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

async fn outside_draft(ui: &Ui, diff: &Entity<DiffViewer>, cx: &mut TestAppContext) {
    let file = diff.read_with(cx, |d, _| {
        d.view
            .summary
            .files
            .iter()
            .position(|f| f.path == "src/Module0/Service0.php")
            .unwrap()
    });
    diff.update(cx, |d, cx| d.go_to_file(file, true, cx));
    ui.press(cx, "s");
    ui.wait(cx, |cx| diff.read(cx).segment(file).mode == Mode::Split)
        .await;
    // Split row 12 is unchanged code well outside the first hunk, as in the e2e.
    ui.wait(cx, |cx| {
        diff.read(cx).view.summary.files[file].split_rows > 20
    })
    .await;
    let row = diff.read_with(cx, |d, _| d.segment(file).start + 12);
    reveal(ui, diff, row, cx);
    // Right-hand gutter is immediately after the 50% divider.
    let position = diff.read_with(cx, |d, _| {
        let b = d.list.bounds_for_item(d.built.item_of_row(row)).unwrap();
        point(b.center().x + px(25.), b.center().y)
    });
    click(ui, cx, position);
    ui.wait(cx, |cx| {
        diff.read(cx)
            .composer
            .as_ref()
            .is_some_and(|(t, _)| t.fallback == Some(true))
    })
    .await;
    ui.input(cx, "Unrelated but worth changing");
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .drafts
            .iter()
            .any(|d| d.draft.body == "Unrelated but worth changing")
    })
    .await;
    ui.press(cx, "s");
}

#[gpui_kit::test]
async fn outside_diff_warns_and_posts_permalink_and_quoted_snippet(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    outside_draft(&ui, &diff, cx).await;
    ui.sheet(cx).await;
    ui.submitted(cx).await;
    ui.drafts(cx, 0).await;
    let comments = fixture.get("/repos/wispy/fixture/pulls/2/comments");
    let body = comments
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["body"].as_str())
        .find(|b| b.contains("Unrelated but worth changing"))
        .unwrap();
    assert!(
        body.contains("https://github.com/wispy/fixture/blob/"),
        "{body}"
    );
    assert!(body.contains("```"), "{body}");
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

fn thread_bounds(
    ui: &Ui,
    diff: &Entity<DiffViewer>,
    id: &str,
    cx: &mut TestAppContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let item = diff.read_with(cx, |d, _| {
        d.built
            .items
            .iter()
            .position(|i| match i {
                wispydiff::diff::layout::Item::Insert(n) => {
                    d.built.inserts[*n as usize].key == format!("thread:{id}")
                }
                _ => false,
            })
            .unwrap()
    });
    diff.update(cx, |d, cx| {
        d.list.scroll_to(ListOffset {
            item_ix: item,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    ui.frame(cx);
    diff.read_with(cx, |d, _| d.list.bounds_for_item(item).unwrap())
}
async fn reply_and_resolve(ui: &Ui, diff: &Entity<DiffViewer>, cx: &mut TestAppContext) -> String {
    ui.threads(cx).await;
    let id = ui.session.read_with(cx, |s, _| {
        s.threads
            .iter()
            .flat_map(|p| &p.threads)
            .find(|t| t.comments.first().is_some_and(|c| c.author == "teammate"))
            .unwrap()
            .id
            .clone()
    });
    let b = thread_bounds(ui, diff, &id, cx);
    let indent = diff.read_with(cx, |d, cx| d.comment_card_indent(cx));
    click(
        ui,
        cx,
        point(b.left() + indent + px(35.), b.bottom() - px(27.)),
    );
    ui.wait(cx, |cx| diff.read(cx).thread_replies.contains_key(&id))
        .await;
    ui.input(cx, "Yes — 304 is the bin limit.");
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .drafts
            .iter()
            .any(|d| d.draft.kind == wispy_core::drafts::DraftKind::Reply)
    })
    .await;
    let b = thread_bounds(ui, diff, &id, cx);
    let indent = diff.read_with(cx, |d, cx| d.comment_card_indent(cx));
    click(
        ui,
        cx,
        point(b.left() + indent + px(110.), b.bottom() - px(27.)),
    );
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .drafts
            .iter()
            .any(|d| d.draft.kind == wispy_core::drafts::DraftKind::Resolve)
    })
    .await;
    id
}

#[gpui_kit::test]
async fn queues_reply_resolve_reopens_and_submits_all_then_reveals_resolved_thread(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    draft(&ui, &diff, "Why this threshold?", cx).await;
    let (file, from) = diff.read_with(cx, |d, _| context_range(d));
    gutter(&ui, &diff, file, from, from + 2, cx);
    ui.input(cx, "These three lines");
    ui.press(cx, "enter");
    ui.drafts(cx, 2).await;
    outside_draft(&ui, &diff, cx).await;
    ui.drafts(cx, 3).await;
    let id = reply_and_resolve(&ui, &diff, cx).await;
    ui.drafts(cx, 5).await;
    drop(diff);
    ui.open(cx, 3).await;
    let diff = ui.open(cx, 2).await;
    ui.drafts(cx, 5).await;
    ui.sheet(cx).await;
    ui.press(cx, secondary("2"));
    ui.submitted(cx).await;
    ui.drafts(cx, 0).await;
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .threads
            .iter()
            .flat_map(|p| &p.threads)
            .any(|t| {
                t.start_line.is_some_and(|start| t.line == Some(start + 2))
                    && t.comments
                        .iter()
                        .any(|c| c.body.contains("These three lines"))
            })
    })
    .await;
    assert_eq!(
        ui.app.read_with(cx, |a, _| a.toast_text()).as_deref(),
        Some("Review submitted")
    );
    ui.wait(cx, |cx| {
        diff.read(cx).marks.values().any(|m| {
            m.settled
                && m.threads
                    .iter()
                    .any(|t| t.id == id && t.preview.starts_with("Resolved · teammate:"))
        })
    })
    .await;
    let comments = fixture.get("/repos/wispy/fixture/pulls/2/comments");
    let bodies: Vec<_> = comments
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert!(bodies.iter().any(|b| b.contains("Why this threshold?")));
    assert!(bodies.iter().any(|b| b.contains("These three lines")));
    assert!(bodies
        .iter()
        .any(|b| b.contains("Unrelated but worth changing")
            && b.contains("https://github.com/wispy/fixture/blob/")));
    assert!(bodies
        .iter()
        .any(|b| b.contains("Yes — 304 is the bin limit.")));
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/2/reviews")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    diff.update(cx, |d, cx| d.show_threads(&[id.clone()], cx));
    ui.wait(cx, |cx| {
        diff.read(cx)
            .built
            .inserts
            .iter()
            .any(|i| i.key == format!("thread:{id}"))
    })
    .await;
    diff.read_with(cx, |d, _| {
        match &d.items_by_key[&format!("thread:{id}")].kind {
            CommentKind::Thread { thread, .. } => {
                assert!(thread.resolved);
                assert_eq!(thread.comments.len(), 2);
            }
            _ => panic!("thread expected"),
        }
    });
    diff.update(cx, |d, cx| d.hide_thread(&id, cx));
    ui.wait(cx, |cx| diff.read(cx).marks.values().any(|m| m.settled))
        .await;
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn multi_pr_range_routes_to_introducing_pr(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    ui.press(cx, "{");
    ui.wait(cx, |cx| {
        let d = diff.read(cx);
        (d.lo, d.hi) == (0, 1)
    })
    .await;
    let (file, offset) = diff.read_with(cx, |d, _| {
        d.built
            .segments
            .iter()
            .find_map(|s| {
                (0..s.rows).find_map(|o| match d.unified_at(s.file, o) {
                    UnifiedRow::Row(r) if r.k == row_kind::ADDED && r.a == Some(0) => {
                        Some((s.file, o))
                    }
                    _ => None,
                })
            })
            .unwrap()
    });
    gutter(&ui, &diff, file, offset, offset, cx);
    assert_eq!(
        diff.read_with(cx, |d, _| d.composer.as_ref().unwrap().0.pr_index),
        0
    );
    ui.press(cx, "escape");
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn submits_one_pr_keeps_other_drafts_and_keyboard_verdicts_respect_own_pr(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 3).await;
    draft(&ui, &diff, "Only this one goes out", cx).await;
    drop(diff);
    let diff = ui.open(cx, 2).await;
    draft(&ui, &diff, "This one stays a draft", cx).await;
    ui.drafts(cx, 2).await;
    ui.sheet(cx).await;
    // First focus is #2; Tab reaches #3 and Enter submits only that summary.
    ui.press(cx, "tab");
    ui.input(cx, "Summary intended for #3");
    ui.press(cx, "enter");
    ui.wait(cx, |_| {
        [1, 2, 3, 4].iter().any(|n| {
            !fixture
                .get(&format!("/repos/wispy/fixture/pulls/{n}/reviews"))
                .as_array()
                .unwrap()
                .is_empty()
        })
    })
    .await;
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/3/reviews")
            .as_array()
            .unwrap()
            .len(),
        1,
        "Tab from #2 should focus #3; reviews received: {:?}",
        [1, 2, 3, 4].map(|n| (
            n,
            fixture.get(&format!("/repos/wispy/fixture/pulls/{n}/reviews"))
        ))
    );
    ui.drafts(cx, 1).await;
    assert!(ui.app.read_with(cx, |a, _| a.sheet.is_some()));
    assert_eq!(
        ui.session
            .read_with(cx, |s, _| s.drafts[0].draft.body.clone()),
        "This one stays a draft"
    );
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/3/reviews")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/2/reviews")
            .as_array()
            .unwrap()
            .len(),
        0
    );
    ui.press(cx, "escape");
    // #2 has drafts, so the sheet starts there again. #1 is yours; its attempted
    // approval must remain COMMENT while #2 keeps the keyboard-selected approval.
    ui.sheet(cx).await;
    ui.press(cx, secondary("3"));
    ui.press(cx, secondary("2"));
    ui.press(cx, "shift-tab");
    ui.press(cx, secondary("2"));
    ui.input(cx, "Own PR comment");
    ui.press(cx, "tab");
    ui.submitted(cx).await;
    ui.drafts(cx, 0).await;
    ui.home(cx).await;
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .inbox
            .iter()
            .flat_map(|e| e.group.prs.iter().zip(&e.progress))
            .any(|(p, s)| p.number == 2 && s.review.as_deref() == Some("APPROVED"))
    })
    .await;
    ui.session.read_with(cx, |s, _| {
        let progress = s
            .inbox
            .iter()
            .flat_map(|e| e.group.prs.iter().zip(&e.progress))
            .find(|(p, _)| p.number == 1)
            .unwrap()
            .1;
        assert_eq!(progress.review.as_deref(), Some("COMMENTED"));
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn clipboard_image_is_local_then_uploaded_on_submit(cx: &mut TestAppContext) {
    use base64::Engine as _;
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    let (file, offset, _) = diff.read_with(cx, |d, _| added(d, 0));
    gutter(&ui, &diff, file, offset, offset, cx);
    ui.input(cx, "Renders like this: ");
    let png=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==").unwrap();
    cx.update(|cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(
            &gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png),
        ))
    });
    ui.press(
        cx,
        if cfg!(target_os = "macos") {
            "cmd-v"
        } else {
            "ctrl-v"
        },
    );
    ui.wait(cx, |cx| {
        diff.read(cx).composer.as_ref().is_some_and(|(_, c)| {
            c.read(cx)
                .input
                .read(cx)
                .value()
                .contains("![image](wispy-image:")
        })
    })
    .await;
    ui.press(cx, "enter");
    ui.drafts(cx, 1).await;
    let body = ui
        .session
        .read_with(cx, |s, _| s.drafts[0].draft.body.clone());
    assert!(body.starts_with("Renders like this: ![image](wispy-image:"));
    let image = body
        .split("wispy-image:")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap()
        .to_string();
    assert!(backend
        .image_data_url(image.clone())
        .await
        .unwrap()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    ui.wait(cx, |_| wispydiff::ui::image_resolver()(&image).is_some())
        .await;
    ui.sheet(cx).await;
    ui.press(cx, secondary("2"));
    ui.submitted(cx).await;
    ui.drafts(cx, 0).await;
    let posted = fixture.get("/repos/wispy/fixture/pulls/2/comments");
    assert!(posted.as_array().unwrap().iter().any(|c| c["body"]
        .as_str()
        .unwrap()
        .contains("![image](https://github.com/user-attachments/assets/fixture-")));
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .threads
            .iter()
            .flat_map(|t| &t.threads)
            .flat_map(|t| &t.comments)
            .any(|c| {
                c.body
                    .contains("https://github.com/user-attachments/assets/fixture-")
            })
    })
    .await;
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

fn copied_field(ui: &Ui, cx: &mut TestAppContext) -> String {
    ui.press(cx, secondary("a"));
    ui.press(cx, secondary("c"));
    cx.read(|cx| {
        cx.read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
async fn merges_green_and_enables_auto_merge_with_edited_commit_message(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    ui.sheet(cx).await;
    let stack = diff.read_with(cx, |d, _| d.stack_id.clone());
    let second = backend.merge_info(stack.clone(), 1).await.unwrap();
    assert_ne!(second.base_ref, "main");
    assert_eq!(second.checks, "PENDING");
    assert_eq!(second.title, "Add bin capacity checks (#2)");
    assert_eq!(second.body, "Fixes #102\nRelated to #7");
    click_merge_toggle(&ui, cx, 1);
    wait_fields(&ui, cx, 2).await;
    let fields = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, true))
        .unwrap();
    click(&ui, cx, fields[0].center());
    assert_eq!(copied_field(&ui, cx), "Add bin capacity checks (#2)");
    click(&ui, cx, fields[1].center());
    assert_eq!(copied_field(&ui, cx), "Fixes #102\nRelated to #7");
    // Focus #1's actual summary, then toggle its merge editor.
    let summaries = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, false))
        .unwrap();
    click(&ui, cx, summaries[0].center());
    let first = backend.merge_info(stack, 0).await.unwrap();
    assert_eq!(first.base_ref, "main");
    assert_eq!(first.merge_state, "CLEAN");
    assert!(first.title.ends_with(" (#1)"));
    click_merge_toggle(&ui, cx, 0);
    wait_fields(&ui, cx, 4).await;
    let fields = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, true))
        .unwrap();
    click(&ui, cx, fields[0].center());
    assert!(copied_field(&ui, cx).ends_with(" (#1)"));
    ui.input(cx, "Ship the first module");
    click(&ui, cx, fields[1].center());
    ui.press(cx, secondary("a"));
    ui.input(cx, "Fixes #101\n\nEdited in the sheet");
    let submit = cx
        .update_window(ui.window, |_, w, cx| {
            let scale = w.scale_factor();
            let accent = gpui_kit::solid_background(wispydiff::ui::palette(cx).accent);
            w.painted_quads()
                .into_iter()
                .filter(|q| {
                    q.background == accent
                        && q.bounds.origin.x.0 / scale > 900.
                        && q.bounds.origin.x.0 / scale < 1100.
                        && q.bounds.origin.y.0 / scale > 100.
                        && q.bounds.size.width.0 / scale > 60.
                        && q.bounds.size.height.0 / scale < 40.
                })
                .map(|q| {
                    point(
                        px(q.bounds.center().x.0 / scale),
                        px(q.bounds.center().y.0 / scale),
                    )
                })
                .next()
                .expect("painted Submit all button")
        })
        .unwrap();
    click(&ui, cx, submit);
    ui.wait(cx, |cx| ui.app.read(cx).sheet.is_none()).await;
    assert_eq!(
        ui.app.read_with(cx, |a, _| a.toast_text()).as_deref(),
        Some("#1 merged · Auto-merge enabled for #2")
    );
    let merges = fixture.get("/__merges");
    let expected = serde_json::json!([
        {"number":1,"auto":false,"method":"SQUASH","title":"Ship the first module","body":"Fixes #101\n\nEdited in the sheet"},
        {"number":2,"auto":true,"method":"SQUASH","title":"Add bin capacity checks (#2)","body":"Fixes #102\nRelated to #7"}
    ]);
    assert_eq!(merges.as_array().unwrap().len(), 2);
    for (actual, expected) in merges
        .as_array()
        .unwrap()
        .iter()
        .zip(expected.as_array().unwrap())
    {
        assert_eq!(actual["head"].as_str().unwrap().len(), 40);
        for key in ["number", "auto", "method", "title", "body"] {
            assert_eq!(actual[key], expected[key], "{key}");
        }
    }
    assert!(fixture
        .get("/repos/wispy/fixture/pulls/1/reviews")
        .as_array()
        .unwrap()
        .is_empty());
    assert!(fixture
        .get("/repos/wispy/fixture/pulls/2/reviews")
        .as_array()
        .unwrap()
        .is_empty());
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

// App buttons have IDs but aren't registered with kit test_support. These are
// the real painted input borders, converted from physical to logical pixels.
// The fixed 820px sheet has 794px summaries and 776px merge fields.
fn painted_fields(
    window: &gpui_kit::Window,
    merge: bool,
) -> Vec<gpui_kit::Bounds<gpui_kit::Pixels>> {
    let scale = window.scale_factor();
    let mut fields: Vec<_> = window
        .painted_quads()
        .into_iter()
        .filter_map(|q| {
            let width = q.bounds.size.width.0 / scale;
            let height = q.bounds.size.height.0 / scale;
            let matches = if merge {
                (775. ..777.).contains(&width) && (31. ..60.).contains(&height)
            } else {
                (793. ..795.).contains(&width) && (57. ..59.).contains(&height)
            };
            matches.then(|| {
                gpui_kit::Bounds::new(
                    point(
                        px(q.bounds.origin.x.0 / scale),
                        px(q.bounds.origin.y.0 / scale),
                    ),
                    gpui_kit::size(px(width), px(height)),
                )
            })
        })
        .collect();
    fields.sort_by(|a, b| a.origin.y.partial_cmp(&b.origin.y).unwrap());
    fields.dedup();
    fields
}

#[gpui_kit::test]
async fn single_pr_submit_by_mouse_keeps_other_prs_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 3).await;
    draft(&ui, &diff, "Only this one goes out", cx).await;
    drop(diff);
    let diff = ui.open(cx, 2).await;
    draft(&ui, &diff, "This one stays a draft", cx).await;
    ui.drafts(cx, 2).await;
    ui.sheet(cx).await;
    let fields = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, false))
        .unwrap();
    assert_eq!(fields.len(), 4);
    click(&ui, cx, fields[2].center());
    ui.press(cx, "enter");
    ui.drafts(cx, 1).await;
    assert!(ui.app.read_with(cx, |a, _| a.sheet.is_some()));
    assert_eq!(
        ui.session
            .read_with(cx, |s, _| s.drafts[0].draft.body.clone()),
        "This one stays a draft"
    );
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/3/reviews")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/2/reviews")
            .as_array()
            .unwrap()
            .len(),
        0
    );
    ui.press(cx, "escape");
    let id = ui
        .session
        .read_with(cx, |s, _| s.drafts[0].draft.id.clone());
    diff.update(cx, |d, cx| d.delete_draft(id, cx));
    ui.drafts(cx, 0).await;
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

fn context_range(diff: &DiffViewer) -> (usize, u32) {
    diff.built.segments.iter().find_map(|s| {
        (0..s.rows.saturating_sub(2)).find(|o|(0..3).all(|n|matches!(diff.unified_at(s.file,o+n),UnifiedRow::Row(r) if r.k==row_kind::CONTEXT)))
            .map(|o|(s.file,o))
    }).expect("three context lines")
}

#[gpui_kit::test]
async fn verdict_shortcuts_set_approval_and_own_pr_remains_comment(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    draft(&ui, &diff, "Sent from the keyboard", cx).await;
    ui.sheet(cx).await;
    ui.press(cx, secondary("3"));
    ui.press(cx, secondary("2"));
    let fields = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, false))
        .unwrap();
    click(&ui, cx, fields[0].center());
    ui.press(cx, secondary("2"));
    ui.input(cx, "Own PR comment");
    click(&ui, cx, fields[1].center());
    ui.submitted(cx).await;
    ui.drafts(cx, 0).await;
    ui.home(cx).await;
    ui.wait(cx, |cx| {
        ui.session
            .read(cx)
            .inbox
            .iter()
            .flat_map(|e| e.group.prs.iter().zip(&e.progress))
            .any(|(p, s)| p.number == 2 && s.review.as_deref() == Some("APPROVED"))
    })
    .await;
    ui.session.read_with(cx, |s, _| {
        assert_eq!(
            s.inbox
                .iter()
                .flat_map(|e| e.group.prs.iter().zip(&e.progress))
                .find(|(p, _)| p.number == 1)
                .unwrap()
                .1
                .review
                .as_deref(),
            Some("COMMENTED")
        )
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

async fn wait_fields(ui: &Ui, cx: &mut TestAppContext, count: usize) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    cx.wait_for(ui.window, TIMEOUT, |w, _| {
        let fields = painted_fields(w, true);
        assert!(
            std::time::Instant::now() < deadline,
            "expected {count} painted merge fields, got {fields:?}"
        );
        fields.len() == count
    })
    .await;
}

fn click_merge_toggle(ui: &Ui, cx: &mut TestAppContext, index: usize) {
    ui.frame(cx);
    let boxes = cx
        .update_window(ui.window, |_, w, _| {
            let scale = w.scale_factor();
            let mut boxes: Vec<_> = w
                .painted_quads()
                .into_iter()
                .filter_map(|q| {
                    let width = q.bounds.size.width.0 / scale;
                    let height = q.bounds.size.height.0 / scale;
                    let x = q.bounds.origin.x.0 / scale;
                    let y = q.bounds.origin.y.0 / scale;
                    ((11.9..12.1).contains(&width) && (11.9..12.1).contains(&height) && x > 900.)
                        .then(|| {
                            gpui_kit::Bounds::new(
                                point(px(x), px(y)),
                                gpui_kit::size(px(width), px(height)),
                            )
                        })
                })
                .collect();
            boxes.sort_by(|a, b| a.origin.y.partial_cmp(&b.origin.y).unwrap());
            boxes.dedup();
            boxes
        })
        .unwrap();
    assert!(boxes.len() > index, "painted merge toggles: {boxes:?}");
    click(ui, cx, boxes[index].center());
}

#[gpui_kit::test]
async fn secondary_enter_from_merge_body_submits(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    ui.sheet(cx).await;
    click_merge_toggle(&ui, cx, 1);
    wait_fields(&ui, cx, 2).await;
    let fields = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, true))
        .unwrap();
    click(&ui, cx, fields[1].center());
    ui.submitted(cx).await;
    let merges = fixture.get("/__merges");
    assert_eq!(merges.as_array().unwrap().len(), 1);
    assert_eq!(merges[0]["number"], 2);
    assert_eq!(merges[0]["auto"], true);
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn offers_suggestion_with_caret_and_indentation(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    let diff = ui.open(cx, 2).await;
    let (file, offset, line) = diff.read_with(cx, |d, _| added(d, 1));
    gutter(&ui, &diff, file, offset, offset, cx);
    ui.wait(cx, |cx| {
        diff.read(cx)
            .composer
            .as_ref()
            .is_some_and(|(t, _)| t.fallback == Some(false))
    })
    .await;
    ui.input(cx, "Simpler:");
    ui.press(cx, secondary("g"));
    let state = |cx: &gpui_kit::App| {
        let composer = &diff.read(cx).composer.as_ref().unwrap().1;
        let input = composer.read(cx).input.read(cx);
        (input.value().to_string(), input.selected_range())
    };
    let expected = format!("Simpler:\n```suggestion\n{line}\n```");
    ui.wait(cx, |cx| state(cx).0 == expected).await;
    let caret = expected.len() - 4;
    cx.read(|cx| assert_eq!(state(cx).1, caret..caret));
    ui.press(cx, "tab");
    let indented = format!("Simpler:\n```suggestion\n    {line}\n```");
    cx.read(|cx| {
        assert_eq!(state(cx).0, indented);
        assert_eq!(state(cx).1, caret + 4..caret + 4);
    });
    ui.press(cx, "shift-tab");
    cx.read(|cx| {
        assert_eq!(state(cx).0, expected);
        assert_eq!(state(cx).1, caret..caret);
    });
    ui.press(cx, "escape");
    ui.wait(cx, |cx| diff.read(cx).composer.is_none()).await;
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

/// The verdict is the first bordered control in the right-hand part of the PR header.
fn verdict_trigger(
    ui: &Ui,
    cx: &mut TestAppContext,
    index: usize,
) -> gpui_kit::Point<gpui_kit::Pixels> {
    ui.frame(cx);
    cx.update_window(ui.window, |_, w, _| {
        let field = painted_fields(w, false)[index];
        let scale = w.scale_factor();
        w.painted_quads()
            .into_iter()
            .filter_map(|q| {
                let b = gpui_kit::Bounds::new(
                    point(px(q.bounds.left().0 / scale), px(q.bounds.top().0 / scale)),
                    gpui_kit::size(
                        px(q.bounds.size.width.0 / scale),
                        px(q.bounds.size.height.0 / scale),
                    ),
                );
                (b.left() > field.center().x
                    && b.top() > field.top() - px(45.)
                    && b.bottom() < field.top()
                    && b.size.width > px(40.)
                    && b.size.width < px(180.)
                    && b.size.height > px(20.)
                    && b.size.height < px(35.))
                .then_some(b)
            })
            .min_by(|a, b| a.left().partial_cmp(&b.left()).unwrap())
            .expect("painted verdict trigger")
            .center()
    })
    .unwrap()
}

#[gpui_kit::test]
async fn verdict_menu_keyboard_pick_escape_tab_and_trigger_toggle(cx: &mut TestAppContext) {
    use gpui_kit::Focusable;
    use wispy_core::github::Verdict;

    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.open(cx, 2).await;
    ui.sheet(cx).await;
    let sheet = ui.app.read_with(cx, |a, _| a.sheet.clone().unwrap());
    let fields = cx
        .update_window(ui.window, |_, w, _| painted_fields(w, false))
        .unwrap();
    click(&ui, cx, fields[1].center());
    let focus = cx
        .update_window(ui.window, |_, w, cx| w.focused(cx))
        .unwrap()
        .unwrap();
    assert_ne!(focus, sheet.read_with(cx, |s, cx| s.focus_handle(cx)));
    assert_eq!(sheet.read_with(cx, |s, _| s.verdict(1)), Verdict::Comment);

    let trigger = verdict_trigger(&ui, cx, 1);
    click(&ui, cx, trigger);
    assert!(sheet.read_with(cx, |s, _| s.menu_is_open()));
    ui.press(cx, "down enter");
    assert_eq!(sheet.read_with(cx, |s, _| s.verdict(1)), Verdict::Approve);
    assert!(!sheet.read_with(cx, |s, _| s.menu_is_open()));
    assert!(cx
        .update_window(ui.window, |_, w, _| focus.is_focused(w))
        .unwrap());

    // Opening starts at Approve; moving to Request changes and cancelling must not pick it.
    for close in ["escape", "tab"] {
        let trigger = verdict_trigger(&ui, cx, 1);
        click(&ui, cx, trigger);
        assert!(sheet.read_with(cx, |s, _| s.menu_is_open()));
        ui.press(cx, &format!("down {close}"));
        assert!(!sheet.read_with(cx, |s, _| s.menu_is_open()));
        assert_eq!(sheet.read_with(cx, |s, _| s.verdict(1)), Verdict::Approve);
        assert!(cx
            .update_window(ui.window, |_, w, _| focus.is_focused(w))
            .unwrap());
    }

    let trigger = verdict_trigger(&ui, cx, 1);
    click(&ui, cx, trigger);
    ui.press(cx, "up up space");
    assert!(!sheet.read_with(cx, |s, _| s.menu_is_open()));
    assert_eq!(sheet.read_with(cx, |s, _| s.verdict(1)), Verdict::Comment);
    assert!(cx
        .update_window(ui.window, |_, w, _| focus.is_focused(w))
        .unwrap());
    ui.press(cx, secondary("2"));

    let trigger = verdict_trigger(&ui, cx, 1);
    click(&ui, cx, trigger);
    assert!(sheet.read_with(cx, |s, _| s.menu_is_open()));
    click(&ui, cx, trigger);
    assert!(
        !sheet.read_with(cx, |s, _| s.menu_is_open()),
        "clicking an open trigger must close without reopening"
    );
    assert_eq!(sheet.read_with(cx, |s, _| s.verdict(1)), Verdict::Approve);
    assert!(cx
        .update_window(ui.window, |_, w, _| focus.is_focused(w))
        .unwrap());

    drop(sheet);
    drop(backend);
    ui.close(cx).await;
}
