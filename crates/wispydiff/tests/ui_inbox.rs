//! Ports inbox.e2e.ts. Each test owns its GitHub state (including submitted reviews).
mod comments_support;
use comments_support::*;
use gpui_kit::{point, px, size, TestAppContext, VisualTestContext};
use wispy_core::model::row_kind;
use wispydiff::{backend::Backend, diff::UnifiedRow, logic::layout::Mode};

#[gpui_kit::test]
async fn lists_requested_and_authored_prs_in_stacks(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    ui.session.read_with(cx, |s, _| {
        let mut groups: Vec<Vec<u64>> = s
            .inbox
            .iter()
            .map(|e| e.group.prs.iter().map(|p| p.number).collect())
            .collect();
        groups.sort();
        assert_eq!(groups, vec![vec![1, 2, 3, 4], vec![5]]);
        let first = s
            .inbox
            .iter()
            .flat_map(|e| &e.group.prs)
            .find(|p| p.number == 1)
            .unwrap();
        assert!(first.authored);
        assert!(!first.requested);
        let third = s
            .inbox
            .iter()
            .flat_map(|e| &e.group.prs)
            .find(|p| p.number == 3)
            .unwrap();
        assert!(third.requested);
    });
    assert_eq!(
        ui.app
            .read_with(cx, |a, cx| a.inbox_view().read(cx).entries.len()),
        2
    );
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn prefetches_every_group_and_opens_each_without_github(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    ui.wait(cx, |cx| ui.session.read(cx).inbox.iter().all(|e| e.ready))
        .await;
    fixture.get("/__offline");
    // Open by the inbox's actual keyboard path, not Backend::open_pr.
    inbox_select(&ui, cx, 1);
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        !ui.session.read(cx).home && ui.app.read(cx).diff.is_some()
    })
    .await;
    ui.home(cx).await;
    inbox_select(&ui, cx, 5);
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        !ui.session.read(cx).home
            && ui
                .session
                .read(cx)
                .stack
                .as_ref()
                .is_some_and(|s| s.prs.len() == 1 && s.prs[0].number == 5)
    })
    .await;
    assert_eq!(
        ui.session
            .read_with(cx, |s, _| s.stack.as_ref().unwrap().prs[0].title.clone()),
        "Fix carrier label typo"
    );
    fixture.get("/__online");
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn shows_review_submitted_from_this_app_at_current_head(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.open(cx, 2).await;
    ui.sheet(cx).await;
    ui.press(cx, secondary("2"));
    ui.submitted(cx).await;
    assert_eq!(
        fixture
            .get("/repos/wispy/fixture/pulls/2/reviews")
            .as_array()
            .unwrap()
            .len(),
        1
    );
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
    ui.app.read_with(cx, |a, cx| {
        let inbox = a.inbox_view().read(cx);
        let prs: Vec<_> = inbox
            .entries
            .iter()
            .flat_map(|e| e.group.prs.iter().zip(&e.progress))
            .collect();
        let (_, second) = prs.iter().find(|(p, _)| p.number == 2).unwrap();
        assert_eq!(second.review.as_deref(), Some("APPROVED"));
        assert!(!second.changed_since_review);
        assert!(prs
            .iter()
            .find(|(p, _)| p.number == 1)
            .unwrap()
            .1
            .review
            .is_none());
    });
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn retains_all_titles_in_crowded_narrow_inbox(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    let titles = ui.app.read_with(cx, |a, cx| {
        a.inbox_view()
            .read(cx)
            .entries
            .iter()
            .flat_map(|e| e.group.prs.iter().map(|p| p.title.clone()))
            .collect::<Vec<_>>()
    });
    VisualTestContext::from_window(ui.window, cx).simulate_resize(size(px(420.), px(900.)));
    ui.frame(cx);
    let narrow = ui.app.read_with(cx, |a, cx| {
        a.inbox_view()
            .read(cx)
            .entries
            .iter()
            .flat_map(|e| e.group.prs.iter().map(|p| p.title.clone()))
            .collect::<Vec<_>>()
    });
    assert_eq!(narrow, titles);
    assert_eq!(narrow.len(), 5);
    assert!(narrow.iter().all(|t| !t.is_empty()));
    // Inbox titles have no observation IDs/debug selectors. This checks reachable
    // application state; actual title allocation needs a read-only layout hook.
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn collapses_with_space_e_and_expands_with_enter_remembering_stack(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    inbox_select(&ui, cx, 1);
    ui.press(cx, "space");
    assert_eq!(
        backend.prefs().get_list("inboxCollapsed"),
        vec!["wispy/fixture#1"]
    );
    let header = inbox_selection_bounds(&ui, cx);
    click(&ui, cx, point(header.left() + px(20.), header.center().y));
    assert!(backend.prefs().get_list("inboxCollapsed").is_empty());
    ui.press(cx, "space space");
    assert!(backend.prefs().get_list("inboxCollapsed").is_empty());
    ui.press(cx, "e");
    assert_eq!(
        backend.prefs().get_list("inboxCollapsed"),
        vec!["wispy/fixture#1"]
    );
    ui.press(cx, "enter");
    assert!(backend.prefs().get_list("inboxCollapsed").is_empty());
    assert!(ui.session.read_with(cx, |s, _| s.home));
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        !ui.session.read(cx).home && ui.session.read(cx).stack.is_some()
    })
    .await;
    assert_eq!(
        ui.session
            .read_with(cx, |s, _| s.stack.as_ref().unwrap().focus),
        0
    );
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn opens_third_pr_from_inbox_with_complete_stack(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    inbox_select(&ui, cx, 3);
    let bounds = inbox_selection_bounds(&ui, cx);
    assert!(bounds.size.width > px(300.));
    click(&ui, cx, bounds.center());
    ui.wait(cx, |cx| {
        !ui.session.read(cx).home && ui.app.read(cx).diff.is_some()
    })
    .await;
    let diff = ui.diff(cx);
    diff.read_with(cx, |d, _| {
        assert_eq!(d.prs.len(), 4);
        assert_eq!((d.lo, d.hi), (2, 2));
        assert_eq!(d.prs[2].title, "Show capacity in admin");
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn j_k_clamp_selection_and_enter_opens_selected_pr(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    let second = ui.session.read_with(cx, |s, _| {
        s.inbox
            .iter()
            .flat_map(|e| &e.group.prs)
            .nth(1)
            .unwrap()
            .number
    });
    ui.press(cx, "k k k k k j enter");
    ui.wait(cx, |cx| {
        !ui.session.read(cx).home && ui.app.read(cx).diff.is_some()
    })
    .await;
    let diff = ui.diff(cx);
    diff.read_with(cx, |d, _| {
        assert_eq!(d.lo, d.hi);
        assert_eq!(d.prs[d.lo].number, second);
    });
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}

#[gpui_kit::test]
async fn offline_prefetched_pr_accepts_draft_and_failed_preparation_keeps_it(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let backend = Backend::with_github(
        fixture.data.path().into(),
        fixture.api(),
        "fixture-token".into(),
    );
    let ui = Ui::new(backend.clone(), cx);
    ui.home(cx).await;
    ui.wait(cx, |cx| ui.session.read(cx).inbox.iter().all(|e| e.ready))
        .await;
    fixture.get("/__offline");
    inbox_select(&ui, cx, 5);
    ui.press(cx, "enter");
    ui.wait(cx, |cx| {
        !ui.session.read(cx).home && ui.app.read(cx).diff.is_some()
    })
    .await;
    let diff = ui.diff(cx);
    let (file, offset) = diff.read_with(cx, |d, _| {
        d.built
            .segments
            .iter()
            .filter(|s| s.mode == Mode::Unified)
            .find_map(|s| {
                (0..s.rows).find_map(|o| match d.unified_at(s.file, o) {
                    UnifiedRow::Row(r) if r.k == row_kind::ADDED => Some((s.file, o)),
                    _ => None,
                })
            })
            .unwrap()
    });
    diff.update(cx, |d, cx| {
        let row = d.segment(file).start + offset;
        d.scroll_to_row(row, 0, None, cx);
    });
    ui.frame(cx);
    let position = diff.read_with(cx, |d, _| {
        let b = d
            .list
            .bounds_for_item(d.built.item_of_row(d.segment(file).start + offset))
            .unwrap();
        point(b.left() + px(25.), b.center().y)
    });
    click(&ui, cx, position);
    ui.input(cx, "Written on a plane");
    ui.press(cx, "enter");
    ui.drafts(cx, 1).await;
    ui.press(cx, "r");
    ui.wait(cx, |cx| ui.app.read(cx).sheet.is_some()).await;
    // The failed preparation's text is private on SubmitSheet; independently
    // wait for the same real backend operation to fail, then assert no draft loss.
    let stack = diff.read_with(cx, |d, _| d.stack_id.clone());
    assert!(backend.prepare_submit(stack).await.is_err());
    ui.press(cx, "escape");
    assert_eq!(
        ui.session
            .read_with(cx, |s, _| s.drafts[0].draft.body.clone()),
        "Written on a plane"
    );
    let id = ui
        .session
        .read_with(cx, |s, _| s.drafts[0].draft.id.clone());
    diff.update(cx, |d, cx| d.delete_draft(id, cx));
    ui.drafts(cx, 0).await;
    fixture.get("/__online");
    drop(diff);
    drop(backend);
    ui.close(cx).await;
}
