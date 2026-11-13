//! Ports of open.e2e.ts and keys.e2e.ts. Every test mounts the production WispyApp under Root.
//! The fixture repository is immutable and shared; fake API state and app data are per test.
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::component::{Root, WindowExt};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    point, px, size, AnyWindowHandle, App, AppContext, Entity, ListOffset, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, ScrollDelta,
    ScrollWheelEvent, TestAppContext, VisualTestContext,
};
use tempfile::TempDir;
use wispy_core::model::row_kind;
use wispydiff::app::WispyApp;
use wispydiff::backend::Backend;
use wispydiff::diff::file_list::FileRow;
use wispydiff::diff::layout::{Item, Mode};
use wispydiff::diff::{extension_of, DiffViewer};
use wispydiff::logic::gaps::hidden_lines;
use wispydiff::session::{BaseMode, Session};
use wispydiff::theme::ThemeId;
use wispydiff::ui::Looks;

const TIMEOUT: Duration = Duration::from_secs(30);
const LOADED_MARKER: &str = "__ui_reading_repo_state_loaded__";
// A Backend owns a CPU-sized Tokio pool. Keep these real-IO roots sequential even when
// libtest starts many test threads; each root still has a separate server and database.
static ROOT_SERIAL: Mutex<()> = Mutex::new(());

fn modifier(key: &str) -> String {
    format!(
        "{}-{key}",
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    )
}

struct Generated {
    _dir: TempDir,
    fixture: PathBuf,
    executable: PathBuf,
}

fn generated() -> &'static Generated {
    static GENERATED: OnceLock<Generated> = OnceLock::new();
    GENERATED.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        // Set process-wide overrides once, before creating any backend or worker.
        for (key, value) in [
            ("WISPY_PREFETCH_INTERVAL", "3600".to_string()),
            (
                "WISPY_CLAUDE_BIN",
                workspace
                    .join("testing/fake-cli/claude")
                    .display()
                    .to_string(),
            ),
            (
                "WISPY_CODEX_BIN",
                workspace
                    .join("testing/fake-cli/codex")
                    .display()
                    .to_string(),
            ),
            (
                "WISPY_OPENER",
                workspace
                    .join("testing/fake-cli/open")
                    .display()
                    .to_string(),
            ),
            ("WISPY_FAKE_LOG", "/dev/null".to_string()),
        ] {
            std::env::set_var(key, value);
        }
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        let output = Command::new(env!("CARGO"))
            .current_dir(&workspace)
            .args(["run", "-p", "wispy-fixtures", "--", "generate", "--out"])
            .arg(&fixture)
            .args(["--seed", "42"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture generation: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let metadata = Command::new(env!("CARGO"))
            .current_dir(&workspace)
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .output()
            .unwrap();
        assert!(metadata.status.success());
        let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
        let executable = PathBuf::from(metadata["target_directory"].as_str().unwrap())
            .join("debug/wispy-fixtures");
        Generated {
            _dir: dir,
            fixture,
            executable,
        }
    })
}

struct Fixture {
    _data: TempDir,
    server: Child,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

struct Reading {
    _serial: MutexGuard<'static, ()>,
    window: AnyWindowHandle,
    app: Entity<WispyApp>,
    session: Entity<Session>,
    backend: Arc<Backend>,
    _fixture: Fixture,
}

impl Reading {
    fn new(cx: &mut TestAppContext) -> Self {
        let serial = ROOT_SERIAL
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cx.executor().allow_parking();
        let generated = generated();
        let mut command = Command::new(&generated.executable);
        command
            .args(["serve", "--fixture"])
            .arg(&generated.fixture)
            .stderr(Stdio::null());
        let (server, port) = spawn_fixture_server(command);
        let data = tempfile::tempdir().unwrap();
        let backend = Backend::with_github(
            data.path().to_path_buf(),
            format!("http://127.0.0.1:{port}"),
            "fixture-token".into(),
        );
        // A fixture-only mark makes completion of Session's initial repo-state load observable.
        // It matches no file content key and never affects a file's viewed state.
        futures::executor::block_on(backend.set_viewed(
            "wispy/fixture".into(),
            LOADED_MARKER.into(),
            true,
        ))
        .unwrap();
        cx.update(|cx| wispydiff::init(backend.clone(), cx));
        let mut entities = None;
        let window = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                let session = cx.new(|cx| Session::new(backend.clone(), cx));
                let app = cx.new(|cx| WispyApp::new(session.clone(), window, cx));
                entities = Some((app.clone(), session));
                Root::new(app, window, cx)
            })
            .into();
        let (app, session) = entities.unwrap();
        let test = Self {
            _serial: serial,
            window,
            app,
            session,
            backend,
            _fixture: Fixture {
                _data: data,
                server,
            },
        };
        test.frame(cx);
        test
    }

    fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn keys(&self, cx: &mut TestAppContext, keys: &str) {
        self.frame(cx);
        cx.simulate_keystrokes(self.window, keys);
        self.frame(cx);
    }

    fn input(&self, cx: &mut TestAppContext, text: &str) {
        cx.update_window(self.window, |_, window, cx| window.input(text, cx))
            .unwrap();
        self.frame(cx);
    }

    fn palette(&self, cx: &mut TestAppContext, text: &str) {
        self.keys(cx, &modifier("k"));
        self.keys(cx, &modifier("a"));
        self.input(cx, text);
        self.keys(cx, "enter");
    }

    async fn open(&self, cx: &mut TestAppContext, pr: usize) {
        self.palette(cx, &format!("wispy/fixture#{pr}"));
        let deadline = Instant::now() + TIMEOUT;
        cx.wait_for(self.window, TIMEOUT, |_, cx| {
            assert!(
                Instant::now() < deadline,
                "opening PR #{pr} exceeded real IO timeout"
            );
            assert!(
                self.session.read(cx).error.is_none(),
                "opening failed: {:?}",
                self.session.read(cx).error
            );
            self.app.read(cx).palette_view().is_none()
                && self.app.read(cx).diff.as_ref().is_some_and(|d| {
                    let d = d.read(cx);
                    (d.lo, d.hi) == (pr - 1, pr - 1)
                        && d.list.viewport_bounds().size.height > px(0.)
                })
        })
        .await;
        // Opening the diff precedes the initial ignore/viewed state load. Wait for that
        // load explicitly before exercising keys that change either state.
        cx.wait_for(self.window, TIMEOUT, |_, cx| {
            assert!(
                Instant::now() < deadline,
                "repo state exceeded real IO timeout"
            );
            self.session.read(cx).busy.is_none()
                && self.session.read(cx).viewed.contains(LOADED_MARKER)
        })
        .await;
    }

    fn diff(&self, cx: &TestAppContext) -> Entity<DiffViewer> {
        self.app
            .read_with(cx, |app, _| app.diff.clone().expect("diff is open"))
    }

    fn read<R>(&self, cx: &TestAppContext, f: impl FnOnce(&DiffViewer, &App) -> R) -> R {
        self.diff(cx).read_with(cx, f)
    }

    async fn wait(
        &self,
        cx: &mut TestAppContext,
        mut predicate: impl FnMut(&DiffViewer, &App) -> bool,
    ) {
        let deadline = Instant::now() + TIMEOUT;
        cx.wait_for(self.window, TIMEOUT, |_, cx| {
            let ready = self
                .app
                .read(cx)
                .diff
                .as_ref()
                .is_some_and(|diff| predicate(diff.read(cx), cx));
            assert!(
                ready || Instant::now() < deadline,
                "diff condition exceeded real IO timeout; range {:?}",
                self.app
                    .read(cx)
                    .diff
                    .as_ref()
                    .map(|d| (d.read(cx).lo, d.read(cx).hi))
            );
            ready
        })
        .await;
    }

    async fn range(&self, cx: &mut TestAppContext, lo: usize, hi: usize) {
        self.wait(cx, |d, _| (d.lo, d.hi) == (lo, hi)).await;
    }

    fn top(&self, cx: &mut TestAppContext) {
        self.diff(cx)
            .update(cx, |d, cx| d.scroll_to_row(0, 0, Some(0), cx));
        self.frame(cx);
    }

    fn scroll(&self, cx: &TestAppContext) -> f32 {
        self.read(cx, |d, _| {
            -f32::from(d.list.scroll_px_offset_for_scrollbar().y)
        })
    }

    async fn settled(&self, cx: &mut TestAppContext) {
        self.wait(cx, |d, _| !d.reading_gliding()).await;
    }

    fn click(
        &self,
        cx: &mut TestAppContext,
        position: gpui_kit::Point<Pixels>,
        modifiers: Modifiers,
    ) {
        VisualTestContext::from_window(self.window, cx).simulate_click(position, modifiers);
        self.frame(cx);
    }

    // File-list rows have a fixed 24px height. Use their real scroll offset and the diff's
    // measured top; no app-only test IDs or synthetic replacement widgets are introduced.
    fn click_file(&self, cx: &mut TestAppContext, index: usize) {
        let row = self.read(cx, |d, _| {
            d.file_rows
                .iter()
                .position(|r| matches!(r, FileRow::File { index: i, .. } if *i == index))
                .unwrap()
        });
        self.diff(cx).update(cx, |d, cx| {
            d.file_scroll
                .scroll_to_item(row, gpui_kit::ScrollStrategy::Top);
            cx.notify();
        });
        self.frame(cx);
        let position = self.read(cx, |d, _| {
            let state = d.file_scroll.0.borrow();
            let height = px(24.);
            let y = state.base_handle.bounds().top()
                + px(4.)
                + height * (row as f32 + 0.5)
                + state.base_handle.offset().y;
            point(px(120.), y)
        });
        self.click(cx, position, Modifiers::default());
        assert_eq!(
            self.read(cx, |d, _| d.current()),
            Some(index),
            "file-list click selects {index}"
        );
    }

    fn double_shift(&self, cx: &mut TestAppContext) {
        let mut visual = VisualTestContext::from_window(self.window, cx);
        for _ in 0..2 {
            visual.simulate_event(ModifiersChangedEvent {
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
                ..Default::default()
            });
            visual.simulate_event(ModifiersChangedEvent::default());
        }
        self.frame(cx);
        assert!(
            self.read(cx, |d, _| d.file_search().is_some()),
            "double Shift opens file search"
        );
    }

    async fn search_file(&self, cx: &mut TestAppContext, query: &str) {
        self.double_shift(cx);
        self.input(cx, query);
        self.keys(cx, "enter");
        self.wait(cx, |d, _| d.file_search().is_none()).await;
    }

    async fn finish(self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, _| window.remove_window())
            .unwrap();
        let weak = Arc::downgrade(&self.backend);
        let Self {
            app,
            session,
            backend,
            _fixture,
            _serial,
            ..
        } = self;
        drop(app);
        drop(session);
        cx.update(|cx| {
            cx.remove_global::<wispydiff::settings::Settings>();
        });
        cx.run_until_parked();
        drop(backend);
        let deadline = Instant::now() + TIMEOUT;
        while weak.strong_count() != 0 {
            assert!(Instant::now() < deadline, "backend workers did not finish");
            cx.executor().timer(Duration::from_millis(10)).await;
            cx.update(|_| {});
        }
        drop(_fixture);
        drop(_serial);
    }
}

fn mode(d: &DiffViewer, file: usize) -> Mode {
    d.segment(file).mode
}
fn viewed(d: &DiffViewer, file: usize, cx: &App) -> bool {
    d.is_viewed(&d.view.summary.files[file].content_key, cx)
}
fn gap_hidden(d: &DiffViewer, file: usize, gap: usize) -> u32 {
    let map = &d.built.unified[&file];
    hidden_lines(
        &map.file.gaps[gap],
        map.reveal.as_ref().and_then(|r| r.get(gap)),
    )
}

// open.e2e: "shows the palette focused ...", "rejects input ...", "reopens the palette ...".
#[gpui_kit::test]
async fn palette_focus_validation_and_escape(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.keys(cx, &modifier("k"));
    cx.update_window(t.window, |_, w, cx| assert!(w.focused_input(cx).is_some()))
        .unwrap();
    t.input(cx, "not a pr");
    t.keys(cx, "enter");
    cx.wait_for(t.window, TIMEOUT, |_, cx| {
        t.app.read(cx).palette_view().is_some_and(|p| {
            p.read(cx)
                .error
                .as_deref()
                .is_some_and(|e| e.contains("not a pull request reference"))
        })
    })
    .await;
    t.keys(cx, "escape");
    assert!(t.app.read_with(cx, |a, _| a.palette_view().is_none()));
    t.open(cx, 2).await;
    t.keys(cx, &modifier("k"));
    cx.update_window(t.window, |_, w, cx| assert!(w.focused_input(cx).is_some()))
        .unwrap();
    t.keys(cx, "escape");
    assert!(t.app.read_with(cx, |a, _| a.palette_view().is_none()));
    t.finish(cx).await;
}

// open.e2e: "opens a PR by owner/repo#N and renders its diff"; "discovers the whole stack ...".
#[gpui_kit::test]
async fn opens_pr_with_highlighting_words_and_stack(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 2).await;
    t.read(cx, |d, _| {
        assert_eq!((d.lo, d.hi, d.prs.len()), (1, 1, 4));
        assert_eq!(d.prs[1].title, "Add bin capacity checks");
        assert_eq!(
            (&*d.prs[1].base_ref, &*d.prs[1].head_ref),
            ("stack/1", "stack/2")
        );
        assert_eq!(d.prs[0].base_ref, "main");
        assert_eq!(d.view.summary.files.len(), 21);
        for kind in [row_kind::ADDED, row_kind::DELETED] {
            assert!(d.view.rows.iter().any(|r| r.k == kind));
            assert!(d.view.rows.iter().any(|r| r.k == kind && !r.w.is_empty()));
        }
        assert!(d
            .view
            .rows
            .iter()
            .any(|r| r.k == row_kind::ADDED
                && r.s.iter().any(|(token, _)| *token == 1 || *token == 2)));
        let viewport = d.list.viewport_bounds();
        assert!(viewport.size.width > px(1000.) && viewport.size.height > px(700.));
        let bounds = d
            .list
            .bounds_for_item(0)
            .expect("first real diff row laid out");
        assert!(bounds.bottom() > viewport.top());
    });
    t.finish(cx).await;
}

// open.e2e: "navigates files with n/p" (j/k don't step through changes any more).
#[gpui_kit::test]
async fn files_with_n_and_p(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 2).await;
    t.top(cx);
    // TestWindow has no display frame callbacks; E-Ink uses SPEC's instant jumps.
    cx.update(|cx| Looks::set_theme(ThemeId::Eink, cx));
    t.keys(cx, "j k");
    t.settled(cx).await;
    assert_eq!(t.scroll(cx), 0.);
    t.keys(cx, "n");
    let next = t.scroll(cx);
    assert!(next > 0.);
    t.keys(cx, "n");
    assert!(t.scroll(cx) > next);
    t.keys(cx, "p p");
    t.settled(cx).await;
    assert_eq!(t.scroll(cx), 0.);
    t.finish(cx).await;
}

// open.e2e: "selects a contiguous range with shift-click and attributes lines to PRs",
// "moves and extends the range with [ ] { }", "opens a different member ... from the cache".
#[gpui_kit::test]
async fn contiguous_ranges_attribution_and_cached_member(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 2).await;
    // At this fixed window/font size, these positions are inside chips #1 and #3.
    // These custom divs do not register Kit snapshots; use the measured bar's vertical position.
    let y = t.read(cx, |d, _| d.list.viewport_bounds().top()) - px(20.);
    t.click(cx, point(px(172.), y), Modifiers::default());
    t.range(cx, 0, 0).await;
    t.click(
        cx,
        point(px(642.), y),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    VisualTestContext::from_window(t.window, cx).simulate_event(ModifiersChangedEvent::default());
    t.range(cx, 0, 2).await;
    t.read(cx, |d, cx| {
        assert_eq!(
            (&*d.prs[d.lo].base_ref, &*d.prs[d.hi].head_ref),
            ("main", "stack/3")
        );
        assert_eq!(d.range_label(), "#1–#3");
        assert!(d.view.rows.iter().any(|r| r.a.is_some()));
        assert!(d.view.summary.files.iter().any(|f| !f.prs.is_empty()));
        assert!(d.tag_room(cx) > px(0.));
    });
    t.keys(cx, "]");
    t.range(cx, 1, 3).await;
    t.keys(cx, "shift-[");
    t.range(cx, 0, 3).await;
    t.keys(cx, "[");
    assert_eq!(t.read(cx, |d, _| (d.lo, d.hi)), (0, 3));
    cx.wait_for(t.window, TIMEOUT, |_, cx| t.session.read(cx).ready(3, 3))
        .await;
    t.open(cx, 4).await;
    t.read(cx, |d, _| {
        assert_eq!(d.prs[3].title, "Capacity docs and tests")
    });
    assert!(t
        .session
        .read_with(cx, |s, _| s.stack.as_ref().unwrap().from_cache));
    t.finish(cx).await;
}

// open.e2e: "selects a contiguous range with shift-click and attributes lines to PRs",
// "moves and extends the range with [ ] { }", "opens a different member ... from the cache".
#[gpui_kit::test]
async fn range_keys_attribution_and_cached_member(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 2).await;
    t.keys(cx, "[");
    t.range(cx, 0, 0).await;
    t.keys(cx, "shift-]");
    t.range(cx, 0, 1).await;
    t.keys(cx, "shift-]");
    t.range(cx, 0, 2).await;
    t.read(cx, |d, cx| {
        assert_eq!(
            (&*d.prs[d.lo].base_ref, &*d.prs[d.hi].head_ref),
            ("main", "stack/3")
        );
        assert_eq!(d.range_label(), "#1–#3");
        assert!(d.view.rows.iter().any(|r| r.a.is_some()));
        assert!(d.view.summary.files.iter().any(|f| !f.prs.is_empty()));
        assert!(d.tag_room(cx) > px(0.));
    });
    t.keys(cx, "]");
    t.range(cx, 1, 3).await;
    t.keys(cx, "shift-[");
    t.range(cx, 0, 3).await;
    t.keys(cx, "[");
    assert_eq!(t.read(cx, |d, _| (d.lo, d.hi)), (0, 3));
    cx.wait_for(t.window, TIMEOUT, |_, cx| t.session.read(cx).ready(3, 3))
        .await;
    t.open(cx, 4).await;
    t.read(cx, |d, _| {
        assert_eq!(d.prs[3].title, "Capacity docs and tests")
    });
    assert!(t
        .session
        .read_with(cx, |s, _| s.stack.as_ref().unwrap().from_cache));
    t.finish(cx).await;
}

// open.e2e: "collapses lockfiles and generated files by default", "expands a collapsed file with e".
#[gpui_kit::test]
async fn noise_defaults_and_expand(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 1).await;
    let (attributes, lock) = t.read(cx, |d, _| {
        let muted: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .enumerate()
            .filter(|(i, _)| mode(d, *i) == Mode::Collapsed)
            .map(|(_, f)| f.path.as_str())
            .collect();
        assert_eq!(muted, ["src/Generated/ApiClient.php", "composer.lock"]);
        let files = &d.view.summary.files;
        (
            files
                .iter()
                .position(|f| f.path == ".gitattributes")
                .unwrap(),
            files
                .iter()
                .position(|f| f.path == "composer.lock")
                .unwrap(),
        )
    });
    t.click_file(cx, attributes);
    t.keys(cx, "shift-down");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(lock));
    t.read(cx, |d, _| {
        assert_eq!(mode(d, lock), Mode::Collapsed);
        assert_eq!(
            d.view.summary.files[lock].noise.as_deref(),
            Some("lockfile")
        );
    });
    t.keys(cx, "e");
    assert_eq!(t.read(cx, |d, _| mode(d, lock)), Mode::Unified);
    t.finish(cx).await;
}

// open.e2e: "toggles the current file to full-file side by side with s, keeping the line in view where it is".
#[gpui_kit::test]
async fn per_file_mode_keeps_head_line_and_split_words(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 1).await;
    let file = t.read(cx, |d, _| {
        d.view
            .summary
            .files
            .iter()
            .position(|f| f.path == "src/Module0/Service0.php")
            .unwrap()
    });
    t.click_file(cx, file);
    t.diff(cx).update(cx, |d, cx| {
        d.list.scroll_by(px(200.));
        cx.notify();
    });
    t.frame(cx);
    let (file, line, top) = t.read(cx, |d, _| {
        d.reading_first_line().expect("head line in viewport")
    });
    t.keys(cx, "s");
    t.wait(cx, |d, _| {
        mode(d, file) == Mode::Split
            && d.reading_split(file).is_some()
            && d.reading_line_top(file, line).is_some()
    })
    .await;
    t.read(cx, |d, _| {
        assert_eq!(d.reading_line_top(file, line), Some(top));
        let rows = d.reading_split(file).unwrap();
        assert!(rows.iter().any(|r| r.nk == row_kind::ADDED));
        assert!(rows.iter().any(|r| r.nk == row_kind::CONTEXT
            || r.nk == wispy_core::split::FILLER
            || r.ok == wispy_core::split::FILLER));
        assert!(rows.iter().any(|r| !r.nw.is_empty()));
    });
    t.keys(cx, "s");
    assert_eq!(t.read(cx, |d, _| mode(d, file)), Mode::Unified);
    assert_eq!(t.read(cx, |d, _| d.reading_line_top(file, line)), Some(top));
    t.finish(cx).await;
}

// open.e2e: "switches every file with S".
#[gpui_kit::test]
async fn global_modes(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 1).await;
    t.keys(cx, "shift-s");
    t.wait(cx, |d, cx| {
        d.session.read(cx).default_mode == BaseMode::Split
    })
    .await;
    t.read(cx, |d, cx| {
        assert_eq!(d.session.read(cx).default_mode, BaseMode::Split);
        assert!(d.built.segments.iter().any(|s| s.mode == Mode::Split));
        assert!(!d.built.segments.iter().any(|s| s.mode == Mode::Unified));
    });
    t.keys(cx, "shift-s");
    t.read(cx, |d, cx| {
        assert_eq!(d.session.read(cx).default_mode, BaseMode::Unified);
        assert!(!d.built.segments.iter().any(|s| s.mode == Mode::Split));
    });
    t.finish(cx).await;
}

// open.e2e: "wraps long lines with z instead of scrolling sideways".
#[gpui_kit::test]
async fn wraps_split_and_unified_rows(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 1).await;
    VisualTestContext::from_window(t.window, cx).simulate_resize(size(px(900.), px(900.)));
    t.keys(cx, "shift-s z");
    t.wait(cx, |d, cx| {
        d.session.read(cx).wrap
            && d.wrap_columns.1 > 0
            && d.reading_split(d.current().unwrap()).is_some()
    })
    .await;
    // Visit all files until a real rendered row wraps. ListState owns the measured heights.
    let mut wrapped = false;
    for file in 0..t.read(cx, |d, _| d.built.segments.len()) {
        if t.read(cx, |d, _| mode(d, file)) != Mode::Split {
            continue;
        }
        t.click_file(cx, file);
        t.wait(cx, |d, _| d.reading_split(file).is_some()).await;
        let count = t.read(cx, |d, _| d.segment(file).rows);
        for offset in (1..count).step_by(25) {
            t.diff(cx).update(cx, |d, cx| {
                d.scroll_to_row(d.segment(file).start + offset, 0, Some(file), cx)
            });
            t.frame(cx);
            if t.read(cx, |d, _| {
                d.built.items.iter().enumerate().any(|(i, item)| {
                    matches!(item, Item::Row(_))
                        && d.list
                            .bounds_for_item(i)
                            .is_some_and(|b| b.size.height > px(20.))
                })
            }) {
                wrapped = true;
                break;
            }
        }
        if wrapped {
            break;
        }
    }
    assert!(wrapped, "narrow split view measures taller wrapped rows");
    t.read(cx, |d, _| {
        assert_eq!(d.scroll_x, px(0.));
        assert!(d.wrap_columns.0 > d.wrap_columns.1);
    });
    t.keys(cx, "shift-s");
    t.read(cx, |d, cx| {
        assert!(d.session.read(cx).wrap);
        assert!(d.wrap_columns.0 > 0);
        assert!(!d.built.segments.iter().any(|s| s.mode == Mode::Split));
        assert_eq!(d.scroll_x, px(0.));
    });
    let columns = t.read(cx, |d, _| d.wrap_columns.0);
    t.keys(cx, "shift-]");
    t.range(cx, 0, 1).await;
    t.read(cx, |d, cx| {
        assert!(d.tag_room(cx) > px(0.));
        assert!(
            d.wrap_columns.0 < columns,
            "unified code makes room for attribution tags"
        );
    });
    t.keys(cx, "z");
    t.read(cx, |d, cx| {
        assert!(!d.session.read(cx).wrap);
        assert_eq!(d.wrap_columns, (0, 0));
        for (i, item) in d.built.items.iter().enumerate() {
            if let Item::Row(row) = item {
                if let Some(bounds) = d.list.bounds_for_item(i) {
                    let segment = d.built.segment_at(*row).unwrap();
                    assert_eq!(
                        bounds.size.height,
                        px(if *row == segment.start { 26. } else { 20. })
                    );
                }
            }
        }
    });
    t.finish(cx).await;
}

// open.e2e: "hides whitespace-only changes with w".
#[gpui_kit::test]
async fn whitespace_toggle(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 2).await;
    assert_eq!(t.read(cx, |d, _| d.view.summary.files.len()), 21);
    t.keys(cx, "w");
    t.wait(cx, |d, cx| {
        d.session.read(cx).shown.as_ref().unwrap().ignore_whitespace
            && d.view.summary.files.len() == 20
    })
    .await;
    t.keys(cx, "w");
    t.wait(cx, |d, cx| {
        !d.session.read(cx).shown.as_ref().unwrap().ignore_whitespace
            && d.view.summary.files.len() == 21
    })
    .await;
    t.finish(cx).await;
}

// open.e2e: "ignores files matching a pattern from the palette".
#[gpui_kit::test]
async fn palette_ignore_and_stop_ignoring(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 2).await;
    t.palette(cx, "> ignore src/Module0/**");
    t.wait(cx, |d, cx| {
        d.session
            .read(cx)
            .ignore_patterns
            .contains(&"src/Module0/**".to_string())
            && d.view
                .summary
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.path.starts_with("src/Module0/"))
                .all(|(i, _)| mode(d, i) == Mode::Collapsed)
    })
    .await;
    assert!(t.app.read_with(cx, |a, _| a.palette_view().is_none()));
    assert!(
        t.read(cx, |d, _| d
            .built
            .segments
            .iter()
            .filter(|s| s.mode == Mode::Collapsed)
            .count())
            >= 2
    );
    t.palette(cx, "> stop ignoring src/Module0/**");
    t.wait(cx, |d, cx| {
        d.session.read(cx).ignore_patterns.is_empty()
            && d.view
                .summary
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.path.starts_with("src/Module0/"))
                .all(|(i, _)| mode(d, i) == Mode::Unified)
    })
    .await;
    t.finish(cx).await;
}

// keys.e2e: "scrolls with the arrow keys and moves between files with shift+arrows".
#[gpui_kit::test]
async fn arrows_scroll_and_shift_arrows_select_files(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    // Exercise the instant E-Ink path: headless TestWindow does not drive vsync callbacks.
    cx.update(|cx| Looks::set_theme(ThemeId::Eink, cx));
    t.keys(cx, "down down");
    t.settled(cx).await;
    assert!(
        (t.scroll(cx) - 120.).abs() < 1.,
        "two arrow steps are 120px: {}",
        t.scroll(cx)
    );
    t.keys(cx, "up");
    t.settled(cx).await;
    assert!((t.scroll(cx) - 60.).abs() < 1.);
    t.top(cx);
    assert_eq!(t.read(cx, |d, _| d.current()), Some(0));
    t.keys(cx, "shift-down");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(1));
    t.keys(cx, "shift-up");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(0));
    t.finish(cx).await;
}

// keys.e2e: "collapses a file with space without marking it viewed, moving on to the next file".
#[gpui_kit::test]
async fn space_collapses_then_keeps_temporary_file_open(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    t.keys(cx, "space");
    t.read(cx, |d, cx| {
        assert_eq!(mode(d, 0), Mode::Collapsed);
        assert_eq!(d.current(), Some(1));
        assert!(!viewed(d, 0, cx));
    });
    t.click_file(cx, 0);
    assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Unified);
    t.keys(cx, "space");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(0));
    t.click_file(cx, 1);
    assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Unified);
    t.finish(cx).await;
}

// keys.e2e: "opens a collapsed file clicked in the file list until another file is shown".
#[gpui_kit::test]
async fn temporary_file_closes_on_navigation_and_s_keeps_it(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    t.keys(cx, "space");
    t.click_file(cx, 0);
    t.click_file(cx, 2);
    assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Collapsed);
    for key in ["n", "shift-down"] {
        t.click_file(cx, 0);
        assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Unified);
        t.keys(cx, key);
        t.read(cx, |d, _| {
            assert_eq!(mode(d, 0), Mode::Collapsed);
            assert_eq!(d.current(), Some(1));
        });
    }
    t.click_file(cx, 0);
    t.keys(cx, "s");
    t.wait(cx, |d, _| d.reading_split(0).is_some()).await;
    t.click_file(cx, 1);
    assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Split);
    t.finish(cx).await;
}

// keys.e2e: "finds files by fuzzy name with Shift Shift, opening a collapsed one for a look".
#[gpui_kit::test]
async fn fuzzy_file_search_and_temporary_open(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let (target, name, first) = t.read(cx, |d, _| {
        let files = &d.view.summary.files;
        let (index, file) = files
            .iter()
            .enumerate()
            .rev()
            .find(|(_, f)| {
                files
                    .iter()
                    .filter(|g| g.path.rsplit('/').next() == f.path.rsplit('/').next())
                    .count()
                    == 1
            })
            .unwrap();
        (
            index,
            file.path.rsplit('/').next().unwrap().to_string(),
            files[0].path.clone(),
        )
    });
    t.search_file(cx, &name).await;
    assert_eq!(t.read(cx, |d, _| d.current()), Some(target));
    t.double_shift(cx);
    t.keys(cx, "escape");
    t.read(cx, |d, _| {
        assert!(d.file_search().is_none());
        assert_eq!(d.current(), Some(target));
    });
    t.top(cx);
    t.keys(cx, "space");
    t.search_file(cx, &first).await;
    t.read(cx, |d, _| {
        assert_eq!(d.current(), Some(0));
        assert_eq!(mode(d, 0), Mode::Unified);
    });
    t.keys(cx, "n");
    t.read(cx, |d, _| {
        assert_eq!(d.current(), Some(1));
        assert_eq!(mode(d, 0), Mode::Collapsed);
    });
    t.click_file(cx, 0);
    t.keys(cx, "space");
    t.click_file(cx, 1);
    assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Unified);
    t.finish(cx).await;
}

// keys.e2e: "moves on to the next file when marking one viewed, so v v marks two in a row",
// "undoes the latest viewed marks with V, going back to each file".
#[gpui_kit::test]
async fn viewed_moves_on_and_undo_returns_in_reverse_order(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    t.keys(cx, "v");
    t.read(cx, |d, cx| {
        assert!(viewed(d, 0, cx));
        assert_eq!(mode(d, 0), Mode::Collapsed);
        assert_eq!(d.current(), Some(1));
    });
    t.keys(cx, "v");
    t.read(cx, |d, cx| {
        assert!(viewed(d, 1, cx));
        assert_eq!(d.current(), Some(2));
    });
    t.keys(cx, "shift-v");
    t.read(cx, |d, cx| {
        assert!(!viewed(d, 1, cx));
        assert!(viewed(d, 0, cx));
        assert_eq!(d.current(), Some(1));
    });
    t.keys(cx, "shift-v");
    t.read(cx, |d, cx| {
        assert!(!viewed(d, 0, cx));
        assert_eq!(d.current(), Some(0));
    });
    t.keys(cx, "shift-v");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(0));
    t.finish(cx).await;
}

// keys.e2e: "skips viewed files with n/p, but not with shift+arrows".
#[gpui_kit::test]
async fn navigation_skips_viewed_except_shift_arrows(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.click_file(cx, 1);
    t.keys(cx, "v");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(2));
    t.keys(cx, "p");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(0));
    t.keys(cx, "n");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(2));
    t.keys(cx, "shift-up");
    assert_eq!(t.read(cx, |d, _| d.current()), Some(1));
    t.finish(cx).await;
}

// keys.e2e: "marks the last files viewed even though they can't scroll to the top".
#[gpui_kit::test]
async fn last_file_stays_current_while_marking_and_unmarking(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let last = t.read(cx, |d, _| d.view.summary.files.len() - 1);
    t.click_file(cx, last - 1);
    t.keys(cx, "v");
    t.read(cx, |d, cx| {
        assert!(viewed(d, last - 1, cx));
        assert_eq!(d.current(), Some(last));
    });
    t.keys(cx, "v");
    t.read(cx, |d, cx| {
        assert!(viewed(d, last, cx));
        assert_eq!(d.current(), Some(last));
    });
    t.keys(cx, "v");
    t.read(cx, |d, cx| {
        assert!(!viewed(d, last, cx));
        assert!(viewed(d, last - 1, cx));
        assert_eq!(d.current(), Some(last));
    });
    t.finish(cx).await;
}

// keys.e2e: "closes the assistant with Escape even from its pickers": the agent / model /
// effort picker is a popup now; the first Escape closes it, the next the assistant.
#[gpui_kit::test]
async fn escape_closes_picker_then_assistant(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.keys(cx, "a");
    assert!(t.read(cx, |d, _| d.assistant.is_some()));
    let picker_open = |t: &Reading, cx: &mut TestAppContext| {
        t.read(cx, |d, cx| d.assistant.as_ref().is_some_and(|a| a.read(cx).picker_is_open()))
    };
    // The chip at the bottom right of the question box, next to the send button.
    let bottom = t.read(cx, |d, _| d.list.viewport_bounds().bottom());
    t.click(cx, point(px(1300.), bottom - px(30.)), Modifiers::default());
    assert!(picker_open(&t, cx), "the chip opens the picker");
    t.keys(cx, "escape");
    assert!(!picker_open(&t, cx));
    assert!(t.read(cx, |d, _| d.assistant.is_some()));
    t.keys(cx, "escape");
    assert!(t.read(cx, |d, _| d.assistant.is_none()));
    t.finish(cx).await;
}

// keys.e2e: "cycles through the stack's PRs with Tab and Shift+Tab".
#[gpui_kit::test]
async fn tab_cycles_prs_and_wraps(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    for (key, pr) in [("tab", 3), ("tab", 0), ("shift-tab", 3), ("shift-tab", 2)] {
        t.keys(cx, key);
        t.range(cx, pr, pr).await;
    }
    t.finish(cx).await;
}

// keys.e2e: "previews themes with ⌘T, reverts on Escape and keeps one on Enter".
#[gpui_kit::test]
async fn theme_preview_revert_and_persist(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let before = cx.read(|cx| cx.global::<Looks>().theme);
    t.keys(cx, &modifier("t"));
    assert_eq!(t.app.read_with(cx, |a, _| a.overlay_kind()), Some("theme"));
    t.keys(cx, "down");
    assert_eq!(
        cx.read(|cx| cx.global::<Looks>().preview),
        Some(ThemeId::Light)
    );
    t.keys(cx, "escape");
    cx.read(|cx| {
        assert_eq!(cx.global::<Looks>().theme, before);
        assert_eq!(cx.global::<Looks>().preview, None);
    });
    assert_eq!(t.app.read_with(cx, |a, _| a.overlay_kind()), None);
    t.keys(cx, &modifier("t"));
    t.keys(cx, "down down down down down enter");
    cx.read(|cx| {
        assert_eq!(cx.global::<Looks>().theme, ThemeId::Nord);
        assert_eq!(cx.global::<Looks>().preview, None);
    });
    assert_eq!(
        t.backend.prefs().get_str("theme", &["nord"], "missing"),
        "nord"
    );
    t.keys(cx, &modifier("t"));
    t.keys(cx, "up up up up up enter");
    assert_eq!(cx.read(|cx| cx.global::<Looks>().theme), ThemeId::System);
    t.finish(cx).await;
}

// keys.e2e: "zooms with ⌘+ / ⌘- / ⌘0 and ⌘+wheel, remembering the level".
#[gpui_kit::test]
async fn zoom_keys_wheel_and_persistence(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let base = t.read(cx, |d, _| d.list.viewport_bounds().size.width);
    t.keys(cx, &modifier("-"));
    assert_eq!(cx.read(wispydiff::ui::zoom), 0.9);
    assert_eq!(
        t.backend.prefs().get_str("zoom", &["0.9"], "missing"),
        "0.9"
    );
    assert!(t.read(cx, |d, _| d.list.viewport_bounds().size.width) > base);
    t.keys(cx, &modifier("0"));
    assert_eq!(cx.read(wispydiff::ui::zoom), 1.);
    t.keys(cx, &modifier("+"));
    assert_eq!(cx.read(wispydiff::ui::zoom), 1.1);
    let before = t.read(cx, |d, _| d.list.logical_scroll_top());
    for (dy, expected) in [(100., 1.25), (-100., 1.1), (-100., 1.)] {
        let position = t.read(cx, |d, _| d.list.viewport_bounds().center());
        VisualTestContext::from_window(t.window, cx).simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: if cfg!(target_os = "macos") {
                Modifiers {
                    platform: true,
                    ..Default::default()
                }
            } else {
                Modifiers {
                    control: true,
                    ..Default::default()
                }
            },
            ..Default::default()
        });
        t.frame(cx);
        assert_eq!(cx.read(wispydiff::ui::zoom), expected);
        assert_eq!(
            t.backend
                .prefs()
                .get_str("zoom", &[&expected.to_string()], "missing"),
            expected.to_string()
        );
    }
    t.read(cx, |d, _| {
        assert_eq!(d.list.logical_scroll_top().item_ix, before.item_ix);
        assert_eq!(
            d.list.logical_scroll_top().offset_in_item,
            before.offset_in_item
        );
        assert_eq!(d.list.viewport_bounds().size.width, base);
    });
    t.finish(cx).await;
}

// keys.e2e: "resizes the file list by dragging its edge, remembering the width".
#[gpui_kit::test]
async fn sidebar_drag_remembers_width_and_double_click_resets(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let top = t.read(cx, |d, _| d.list.viewport_bounds().top());
    let y = top + px(100.);
    cx.update_window(t.window, |_, w, cx| {
        w.drag(point(px(278.), y), point(px(380.), y), cx)
    })
    .unwrap();
    t.frame(cx);
    assert_eq!(t.read(cx, |d, _| d.file_list_width), 380.);
    assert_eq!(
        t.backend.prefs().get::<Option<f32>>("width.files"),
        Some(Some(380.))
    );
    t.keys(cx, &modifier("b"));
    assert!(!t.session.read_with(cx, |s, _| s.show_files));
    t.keys(cx, &modifier("b"));
    assert!(t.session.read_with(cx, |s, _| s.show_files));
    assert_eq!(t.read(cx, |d, _| d.file_list_width), 380.);
    let mut v = VisualTestContext::from_window(t.window, cx);
    let position = point(px(378.), y);
    v.simulate_event(MouseDownEvent {
        position,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    v.simulate_event(MouseUpEvent {
        position,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    t.frame(cx);
    assert_eq!(t.read(cx, |d, _| d.file_list_width), 280.);
    t.finish(cx).await;
}

// keys.e2e: "shows the file list as a tree with t, folding directories and opening the current file's".
#[gpui_kit::test]
async fn file_tree_folds_and_navigation_reopens_directories(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    t.keys(cx, "t");
    t.read(cx, |d, _| {
        assert!(d.list_mode_tree);
        assert_eq!(
            d.file_rows
                .iter()
                .filter(|r| matches!(r, FileRow::File { .. }))
                .count(),
            d.view.summary.files.len()
        );
        assert_eq!(d.current(), Some(0));
    });
    let (dir, row) = t.read(cx, |d, _| {
        let path = &d.view.summary.files[1].path;
        d.file_rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                FileRow::Dir { path: dir, .. } if path.starts_with(&format!("{dir}/")) => {
                    Some((dir.clone(), i))
                }
                _ => None,
            })
            .max_by_key(|(dir, _)| dir.len())
            .unwrap()
    });
    let y = t.read(cx, |d, _| d.list.viewport_bounds().top()) + px(4. + row as f32 * 24. + 12.);
    t.click(cx, point(px(70.), y), Modifiers::default());
    t.read(cx, |d, _| {
        assert!(d.tree_closed.contains(&dir));
        assert!(!d
            .file_rows
            .iter()
            .any(|r| matches!(r, FileRow::File { index: 1, .. })));
    });
    t.keys(cx, "n");
    t.read(cx, |d, _| {
        assert_eq!(d.current(), Some(1));
        assert!(!d.tree_closed.contains(&dir));
    });
    let (small, files) = t.read(cx, |d, _| {
        d.file_rows
            .iter()
            .filter_map(|r| match r {
                FileRow::Dir { path, files, .. } => Some((path.clone(), files.clone())),
                _ => None,
            })
            .min_by_key(|(_, files)| files.len())
            .unwrap()
    });
    for file in &files {
        t.click_file(cx, *file);
        t.keys(cx, "v");
        t.read(cx, |d, cx| assert!(viewed(d, *file, cx)));
    }
    t.read(cx, |d, _| {
        assert!(d.tree_closed.contains(&small));
        assert!(!d
            .file_rows
            .iter()
            .any(|r| matches!(r, FileRow::File { index, .. } if files.contains(index))));
    });
    t.keys(cx, "t");
    t.read(cx, |d, _| {
        assert!(!d.list_mode_tree);
        assert_eq!(d.file_rows.len(), d.view.summary.files.len());
        assert!(d
            .file_rows
            .iter()
            .all(|r| matches!(r, FileRow::File { .. })));
    });
    assert_eq!(
        t.backend.prefs().get_str("fileList", &["list"], "missing"),
        "list"
    );
    t.finish(cx).await;
}

// keys.e2e: "filters files by extension, remembered per PR".
#[gpui_kit::test]
async fn file_filter_persists_per_pr_hidden_search_and_show_all(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let (extensions, hidden_path, hidden_ext) = t.read(cx, |d, _| {
        let mut extensions: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .map(|f| extension_of(&f.path))
            .collect();
        extensions.sort();
        extensions.dedup();
        let path = d.view.summary.files[0].path.clone();
        (extensions, path.clone(), extension_of(&path))
    });
    t.keys(cx, "f");
    assert!(t.read(cx, |d, _| d.filter_open));
    // Start at the extension's actual sorted menu index, drive the menu with real keys.
    for _ in 0..extensions.iter().position(|e| *e == hidden_ext).unwrap() {
        t.keys(cx, "down");
    }
    t.keys(cx, "enter escape");
    let kept = t.read(cx, |d, _| {
        assert_eq!(d.filter().exts, [hidden_ext.clone()]);
        let paths: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .enumerate()
            .filter(|(i, _)| !d.filtered()[*i])
            .map(|(_, f)| f.path.clone())
            .collect();
        assert!(!paths.is_empty());
        assert!(d
            .view
            .summary
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| extension_of(&f.path) == hidden_ext)
            .all(|(i, _)| mode(d, i) == Mode::Hidden));
        assert_eq!(d.file_rows.len(), paths.len());
        paths
    });
    t.keys(cx, "tab");
    t.range(cx, 3, 3).await;
    assert!(t.read(cx, |d, _| d.filter().exts.is_empty()));
    t.keys(cx, "shift-tab");
    t.range(cx, 2, 2).await;
    t.read(cx, |d, _| {
        assert_eq!(d.filter().exts, [hidden_ext.clone()]);
        let paths: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .enumerate()
            .filter(|(i, _)| !d.filtered()[*i])
            .map(|(_, f)| f.path.clone())
            .collect();
        assert_eq!(paths, kept);
    });
    t.search_file(cx, &hidden_path).await;
    t.wait(cx, |d, _| {
        d.file_view.as_ref().is_some_and(|f| f.path == hidden_path)
    })
    .await;
    t.read(cx, |d, _| assert!(d.file_view.as_ref().unwrap().filtered));
    t.keys(cx, "escape");
    assert!(t.read(cx, |d, _| d.file_view.is_none()));
    t.keys(cx, "f");
    for (i, ext) in extensions.iter().enumerate() {
        if i > 0 {
            t.keys(cx, "down");
        }
        if *ext != hidden_ext {
            t.keys(cx, "space");
        }
    }
    t.read(cx, |d, _| {
        assert!(d.filtered().iter().all(|hidden| *hidden));
        assert!(d.file_rows.is_empty());
        assert_eq!(d.built.total_rows, 0);
    });
    t.keys(cx, "escape");
    // The empty-state Show all files link is at the diff's top left.
    let y = t.read(cx, |d, _| d.list.viewport_bounds().top()) - px(22.);
    t.click(cx, point(px(650.), y), Modifiers::default());
    assert!(
        t.read(cx, |d, _| d.filtered().iter().all(|hidden| !*hidden)),
        "Show all files clears the filter"
    );
    t.finish(cx).await;
}

// keys.e2e: "filters with the keyboard: x hides the current file's extension, F switches the filter off, f opens it".
#[gpui_kit::test]
async fn keyboard_file_filter_x_f_and_menu(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    let (hidden, next) = t.read(cx, |d, _| {
        let ext = extension_of(&d.view.summary.files[0].path);
        let next = d
            .view
            .summary
            .files
            .iter()
            .position(|f| extension_of(&f.path) != ext)
            .unwrap();
        (ext, next)
    });
    t.keys(cx, "x");
    t.read(cx, |d, _| {
        assert_eq!(d.filter().exts, [hidden.clone()]);
        assert_eq!(d.current(), Some(next));
        assert!(d.filtered()[0]);
    });
    let kept = t.read(cx, |d, _| d.filtered());
    t.keys(cx, "shift-f");
    t.read(cx, |d, _| {
        assert!(d.filter().off);
        assert_eq!(d.filter().exts, [hidden.clone()]);
        assert!(d.filtered().iter().all(|f| !*f));
    });
    t.keys(cx, "shift-f");
    assert_eq!(t.read(cx, |d, _| d.filtered()), kept);
    t.keys(cx, "f");
    assert!(t.read(cx, |d, _| d.filter_open));
    let before = t.read(cx, |d, _| d.filter().exts);
    t.keys(cx, "down enter");
    t.read(cx, |d, _| {
        assert_eq!(d.filter_highlight, 1);
        assert_ne!(d.filter().exts, before);
    });
    t.keys(cx, "escape");
    assert!(!t.read(cx, |d, _| d.filter_open));
    // Escape restores the diff's focus: x remains usable.
    t.keys(cx, "shift-f");
    assert!(t.read(cx, |d, _| d.filter().off));
    t.finish(cx).await;
}

fn gap_item(d: &DiffViewer, file: usize, gap: usize) -> Option<usize> {
    let offset = d.built.unified.get(&file)?.gap_row(gap)?;
    Some(d.built.item_of_row(d.segment(file).start + offset))
}

// keys.e2e: "shows unchanged lines between hunks: a click shows 10 more on each side, an arrow 20 on one, a double click all, ⇧E every gap of the file".
#[gpui_kit::test]
async fn gaps_click_arrow_double_click_and_shift_e_keep_place(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    let (file, gap, hidden) = t.read(cx, |d, _| {
        d.view
            .summary
            .files
            .iter()
            .enumerate()
            .find_map(|(file, f)| {
                f.gaps
                    .iter()
                    .enumerate()
                    .find(|(_, g)| g.len > 65 && g.new_start > 1 && g.offset < f.row_count)
                    .map(|(gap, g)| (file, gap, g.len))
            })
            .expect("fixture has a large interior gap")
    });
    t.click_file(cx, file);
    t.diff(cx).update(cx, |d, cx| {
        let item = gap_item(d, file, gap).unwrap();
        d.list.scroll_to(ListOffset {
            item_ix: item.saturating_sub(5),
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    t.frame(cx);
    let position = t.read(cx, |d, _| {
        let bounds = d
            .list
            .bounds_for_item(gap_item(d, file, gap).unwrap())
            .unwrap();
        point(bounds.left() + px(300.), bounds.center().y)
    });
    let before_top = position.y;
    t.click(cx, position, Modifiers::default());
    t.wait(cx, |d, _| gap_hidden(d, file, gap) == hidden - 20)
        .await;
    t.read(cx, |d, _| {
        let map = &d.built.unified[&file];
        assert_eq!(map.reveal.as_ref().unwrap()[gap], [10, 10]);
        assert_eq!(
            d.list
                .bounds_for_item(gap_item(d, file, gap).unwrap())
                .unwrap()
                .center()
                .y,
            before_top
        );
    });
    // ↓ is the first arrow in the gutter; synthetic text metrics still give it a normal hitbox.
    let arrow = t.read(cx, |d, cx| {
        let bounds = d
            .list
            .bounds_for_item(gap_item(d, file, gap).unwrap())
            .unwrap();
        let gutter = wispydiff::ui::upx(40., cx);
        point(bounds.left() + gutter, bounds.center().y)
    });
    t.click(cx, arrow, Modifiers::default());
    t.wait(cx, |d, _| gap_hidden(d, file, gap) == hidden - 40)
        .await;
    t.read(cx, |d, _| {
        assert_eq!(
            d.built.unified[&file].reveal.as_ref().unwrap()[gap],
            [30, 10]
        )
    });
    let position = t.read(cx, |d, _| {
        let bounds = d
            .list
            .bounds_for_item(gap_item(d, file, gap).unwrap())
            .unwrap();
        point(bounds.left() + px(300.), bounds.center().y)
    });
    let mut v = VisualTestContext::from_window(t.window, cx);
    v.simulate_event(MouseDownEvent {
        position,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    v.simulate_event(MouseUpEvent {
        position,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    t.wait(cx, |d, _| gap_item(d, file, gap).is_none()).await;
    let first = t.read(cx, |d, _| d.reading_first_line().unwrap());
    t.keys(cx, "shift-e");
    t.wait(cx, |d, _| {
        (0..d.view.summary.files[file].gaps.len()).all(|g| gap_item(d, file, g).is_none())
    })
    .await;
    assert_eq!(
        t.read(cx, |d, _| d.reading_line_top(first.0, first.1)),
        Some(first.2)
    );
    t.keys(cx, "shift-e");
    t.wait(cx, |d, _| gap_hidden(d, file, gap) == hidden).await;
    t.finish(cx).await;
}

// keys.e2e: "lists the keyboard shortcuts with ?".
#[gpui_kit::test]
async fn shortcuts_question_mark_and_escape(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.keys(cx, "?");
    assert_eq!(
        t.app.read_with(cx, |a, _| a.overlay_kind()),
        Some("shortcuts")
    );
    t.keys(cx, "escape");
    assert_eq!(t.app.read_with(cx, |a, _| a.overlay_kind()), None);
    // The other documented entry point reaches the same real overlay.
    t.keys(cx, &modifier("/"));
    assert_eq!(
        t.app.read_with(cx, |a, _| a.overlay_kind()),
        Some("shortcuts")
    );
    t.keys(cx, "escape");
    t.finish(cx).await;
}

// Keep unaffected assertions active while noise_defaults_and_expand records the bottom-file bug.
#[gpui_kit::test]
async fn default_noise_files_are_collapsed(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 1).await;
    t.read(cx, |d, _| {
        let muted: Vec<_> = d
            .view
            .summary
            .files
            .iter()
            .enumerate()
            .filter(|(i, _)| mode(d, *i) == Mode::Collapsed)
            .map(|(_, f)| (f.path.as_str(), f.noise.as_deref()))
            .collect();
        assert_eq!(
            muted,
            [
                ("src/Generated/ApiClient.php", Some("generated")),
                ("composer.lock", Some("lockfile"))
            ]
        );
    });
    t.finish(cx).await;
}

#[gpui_kit::test]
async fn e_expands_collapsed_current_file(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    t.keys(cx, "space shift-up");
    t.read(cx, |d, cx| {
        assert_eq!(d.current(), Some(0));
        assert_eq!(mode(d, 0), Mode::Collapsed);
        assert!(!viewed(d, 0, cx));
    });
    t.keys(cx, "e");
    t.read(cx, |d, cx| {
        assert_eq!(d.current(), Some(0));
        assert_eq!(mode(d, 0), Mode::Unified);
        assert!(!viewed(d, 0, cx));
    });
    t.finish(cx).await;
}

// The last-file search target remains tested above, with its expected behaviour ignored as a bug.
#[gpui_kit::test]
async fn file_search_temporary_open_escape_and_keep_open(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 3).await;
    t.top(cx);
    let first = t.read(cx, |d, _| d.view.summary.files[0].path.clone());
    t.keys(cx, "space");
    t.search_file(cx, &first).await;
    t.read(cx, |d, _| {
        assert_eq!(d.current(), Some(0));
        assert_eq!(mode(d, 0), Mode::Unified);
    });
    t.double_shift(cx);
    t.keys(cx, "escape");
    t.read(cx, |d, _| {
        assert!(d.file_search().is_none());
        assert_eq!(d.current(), Some(0));
    });
    t.keys(cx, "n");
    t.read(cx, |d, _| {
        assert_eq!(d.current(), Some(1));
        assert_eq!(mode(d, 0), Mode::Collapsed);
    });
    t.search_file(cx, first.rsplit('/').next().unwrap()).await;
    t.read(cx, |d, _| {
        assert_eq!(d.current(), Some(0));
        assert_eq!(mode(d, 0), Mode::Unified);
    });
    t.keys(cx, "space");
    t.click_file(cx, 1);
    assert_eq!(t.read(cx, |d, _| mode(d, 0)), Mode::Unified);
    t.finish(cx).await;
}

// Keep split rendering assertions active while the mode round-trip anchor test is ignored.
#[gpui_kit::test]
async fn split_rows_cover_whole_files_with_alignment_and_changed_words(cx: &mut TestAppContext) {
    let t = Reading::new(cx);
    t.open(cx, 1).await;
    let file = t.read(cx, |d, _| {
        d.view
            .summary
            .files
            .iter()
            .position(|f| f.path == "src/Module0/Service0.php")
            .unwrap()
    });
    t.click_file(cx, file);
    t.keys(cx, "s");
    t.wait(cx, |d, _| {
        mode(d, file) == Mode::Split && d.reading_split(file).is_some()
    })
    .await;
    t.read(cx, |d, _| {
        let rows = d.reading_split(file).unwrap();
        assert!(rows.iter().any(|r| r.nk == row_kind::ADDED));
        assert!(rows.iter().any(|r| r.nk == row_kind::CONTEXT));
        assert!(rows.iter().any(|r| r.nk == row_kind::CONTEXT
            || r.ok == wispy_core::split::FILLER
            || r.nk == wispy_core::split::FILLER));
        assert!(rows.iter().any(|r| !r.nw.is_empty()));
        assert_eq!(
            rows.iter().filter_map(|r| r.n).max(),
            Some(d.view.summary.files[file].new_lines)
        );
        assert_eq!(
            rows.iter().filter_map(|r| r.o).max(),
            Some(d.view.summary.files[file].old_lines)
        );
        assert_eq!(rows.len() + 1, d.segment(file).rows as usize);
    });
    t.finish(cx).await;
}

/// Starts the fake GitHub server on a free port it picks itself (`--port 0`) and returns the
/// port it printed once listening: no window where another test's server can take the port.
fn spawn_fixture_server(mut command: Command) -> (Child, u16) {
    use std::io::BufRead;
    let mut server = command
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .trim()
        .strip_prefix("port ")
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| {
            let _ = server.kill();
            panic!("fixture server did not start: {line:?}")
        });
    (server, port)
}
