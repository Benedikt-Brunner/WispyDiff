//! Ports progress.e2e.ts; each independent scenario uses the real app root and services.
mod ui_c_support;
use gpui_kit::{px, TestAppContext};
use ui_c_support::*;
use wispydiff::diff::layout::Mode;

// e2e: marks a file viewed with v, which collapses and ticks it
#[gpui_kit::test]
async fn marks_file_viewed_collapsed_and_ticked(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    let path = ui.paths(cx)[0].clone();
    ui.keys(cx, "v");
    assert!(
        ui.viewed(cx, &path),
        "v must retain the viewed mark when the opening repo-state load completes"
    );
    ui.diff(cx).read_with(cx, |d, _| {
        assert_eq!(d.segment(0).mode, Mode::Collapsed);
        assert!(d
            .file_rows
            .iter()
            .any(|r| matches!(r, wispydiff::diff::file_list::FileRow::File { .. })));
        assert!(d.list.viewport_bounds().size.height > px(600.));
    });
    ui.close(cx).await;
}

// e2e: records a local checkpoint with M; says so when nothing changed ... (d)
#[gpui_kit::test]
async fn checkpoint_and_empty_since_view_remain_keyboard_navigable(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    ui.mark(cx).await;
    ui.toggle_since(cx, true).await;
    assert!(ui.paths(cx).is_empty());
    ui.session.read_with(cx, |s, _| {
        assert_eq!(s.checkpoints.len(), 1);
        assert_eq!(s.checkpoints[0].source, "manual");
    });
    ui.keys(cx, "n");
    ui.toggle_since(cx, false).await;
    assert_eq!(ui.paths(cx).len(), 1);
    ui.close(cx).await;
}

// e2e: keeps the file viewed across a rebase ...; shows only the author's new changes ...;
// marking a file viewed in the changes since ... marks it in the full diff too
#[gpui_kit::test]
async fn content_marks_survive_rebase_and_since_excludes_base_changes(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    let path = ui.paths(cx)[0].clone();
    ui.mark(cx).await;
    ui.focus_diff(cx);
    ui.keys(cx, "v");
    assert!(ui.viewed(cx, &path));
    f.git(&["checkout", "--quiet", "main"]);
    f.write(
        "docs/CHANGELOG.md",
        "# Changelog\n\n- unrelated work on main\n",
    );
    f.git(&["add", "-A"]);
    f.git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.test",
        "commit",
        "--quiet",
        "-m",
        "main moves on",
    ]);
    f.git(&["checkout", "--quiet", "solo"]);
    f.git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.test",
        "rebase",
        "--quiet",
        "main",
    ]);
    f.git(&["update-ref", "refs/pull/5/head", "solo"]);
    f.git(&["checkout", "--quiet", "main"]);
    ui.refresh(cx).await;
    ui.load_newer(cx).await;
    assert!(ui.viewed(cx, &path));
    let current = f.show(&format!("solo:{path}"));
    let mut lines: Vec<String> = current.split("\n").map(str::to_string).collect();
    let line = lines.iter_mut().find(|l| l.contains("result = ")).unwrap();
    line.push_str(" // reviewed fix");
    f.push(&path, &lines.join("\n"));
    ui.refresh(cx).await;
    ui.load_newer(cx).await;
    assert!(!ui.viewed(cx, &path));
    ui.toggle_since(cx, true).await;
    assert_eq!(ui.paths(cx), [path.clone()]);
    ui.diff(cx).read_with(cx, |d, _| {
        let file = &d.view.summary.files[0];
        assert_eq!((file.additions, file.deletions), (1, 1));
    });
    ui.keys(cx, "v");
    assert!(ui.viewed(cx, &path));
    ui.toggle_since(cx, false).await;
    assert!(ui.viewed(cx, &path));
    ui.keys(cx, "v");
    assert!(!ui.viewed(cx, &path));
    ui.close(cx).await;
}

// e2e: offers new commits ... as soon as background refresh sees them;
// warns ... changed in newer commits, once per version
#[gpui_kit::test]
async fn background_push_and_changed_file_viewed_warning(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    let path = ui.paths(cx)[0].clone();
    f.push("docs/NOTES.md", "pushed while the PR was open\n");
    ui.refresh(cx).await;
    assert_eq!(ui.paths(cx), [path.clone()]);
    ui.load_newer(cx).await;
    assert_eq!(ui.paths(cx), ["docs/NOTES.md".to_string(), path.clone()]);
    f.push("docs/NOTES.md", "pushed while the PR was open, amended\n");
    ui.refresh(cx).await;
    ui.keys(cx, "v");
    ui.wait(cx, "newer warning", |cx| {
        ui.app.read(cx).overlay_kind() == Some("newer")
    })
    .await;
    assert!(!ui.viewed(cx, "docs/NOTES.md"));
    ui.keys(cx, "enter");
    ui.wait(cx, "loaded via warning", |cx| {
        ui.session.read(cx).newer.is_none() && ui.app.read(cx).overlay_kind().is_none()
    })
    .await;
    assert!(!ui.viewed(cx, "docs/NOTES.md"));
    let content = f.show(&format!("solo:{path}"));
    f.push(&path, &format!("{content}\n// one more thing\n"));
    ui.refresh(cx).await;
    // A refresh's newer content-key check is real IO; an unchanged file must not warn.
    ui.keys(cx, "v");
    ui.wait(cx, "unchanged NOTES viewed without warning", |cx| {
        ui.viewed(cx, "docs/NOTES.md") && ui.app.read(cx).overlay_kind().is_none()
    })
    .await;
    ui.keys(cx, "v");
    ui.wait(cx, "new version warns", |cx| {
        ui.app.read(cx).overlay_kind() == Some("newer")
    })
    .await;
    ui.keys(cx, "escape");
    assert!(ui.viewed(cx, &path));
    ui.keys(cx, "shift-v v");
    assert!(ui.viewed(cx, &path));
    assert!(ui.app.read_with(cx, |a, _| a.overlay_kind().is_none()));
    ui.close(cx).await;
}

// e2e: shows in the inbox that #5 changed ... until it's marked again
#[gpui_kit::test]
async fn inbox_checkpoint_changed_flag_clears_after_marking_latest(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    ui.mark(cx).await;
    f.push("docs/NOTES.md", "new work\n");
    ui.refresh(cx).await;
    ui.load_newer(cx).await;
    ui.keys(cx, "ctrl-i");
    ui.wait(cx, "inbox marked changed", |cx| {
        ui.session
            .read(cx)
            .inbox
            .iter()
            .flat_map(|e| &e.progress)
            .any(|p| p.marked && p.changed_since_marked)
    })
    .await;
    ui.open(cx, 5).await;
    ui.mark(cx).await;
    ui.keys(cx, "ctrl-i");
    ui.wait(cx, "inbox marked current", |cx| {
        ui.session
            .read(cx)
            .inbox
            .iter()
            .flat_map(|e| &e.progress)
            .any(|p| p.marked && !p.changed_since_marked)
    })
    .await;
    ui.close(cx).await;
}

// e2e: switches to ... checkpoint with a comment on a file that isn't in them
#[gpui_kit::test]
async fn since_view_excludes_unchanged_comment_file_and_restores_draft(cx: &mut TestAppContext) {
    let f = Fixture::new();
    let ui = Ui::mount(cx, &f);
    ui.open(cx, 5).await;
    let path = ui.paths(cx)[0].clone();
    f.push("docs/NOTES.md", "notes\n");
    ui.refresh(cx).await;
    ui.load_newer(cx).await;
    ui.mark(cx).await;
    let offsets = ui.changed_offsets(cx, 1);
    let offset = offsets.iter().copied().find(|&o| ui.diff(cx).read_with(cx, |d,_| matches!(d.unified_at(1,o), wispydiff::diff::UnifiedRow::Row(r) if r.n.is_some()))).unwrap();
    ui.reveal_row(cx, 1, offset);
    let bounds = ui.row_bounds(cx, 1, offset).unwrap();
    ui.click(
        cx,
        gpui_kit::point(bounds.left() + px(40.), bounds.center().y),
    );
    ui.wait(cx, "line composer", |cx| {
        ui.diff(cx).read(cx).composer.is_some()
    })
    .await;
    ui.input(cx, "Still needed?");
    ui.keys(cx, "ctrl-enter");
    ui.wait(cx, "saved draft", |cx| {
        ui.session
            .read(cx)
            .drafts
            .iter()
            .any(|d| d.draft.body == "Still needed?")
    })
    .await;
    f.push("docs/NOTES.md", "reworded notes\n");
    ui.refresh(cx).await;
    ui.load_newer(cx).await;
    ui.toggle_since(cx, true).await;
    assert_eq!(ui.paths(cx), ["docs/NOTES.md"]);
    ui.toggle_since(cx, false).await;
    ui.wait(cx, "restored draft", |cx| ui.diff(cx).read(cx).items_by_key.values().any(|i| matches!(&i.kind, wispydiff::diff::CommentKind::Draft(d) if d.path.as_deref()==Some(path.as_str()) && d.draft.body=="Still needed?"))).await;
    ui.close(cx).await;
}
