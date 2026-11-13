//! Shared only by ui_progress/ui_intel/ui_assistant. Every test mounts the production root.
#![allow(dead_code)]
use gpui_kit::{
    base::test_support as observation, component::Root, point, px, size, test::TestWindowExt,
    AnyWindowHandle, App, AppContext, Bounds, Entity, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};
use wispy_core::git::Git;
use wispydiff::{
    app::WispyApp,
    backend::Backend,
    diff::{DiffViewer, UnifiedRow},
    session::Session,
};

pub const TIMEOUT: Duration = Duration::from_secs(30);
static CLEANUP: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());
static BASE: OnceLock<(PathBuf, PathBuf, PathBuf)> = OnceLock::new();
pub fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
pub struct Fixture {
    pub root: Option<tempfile::TempDir>,
    pub origin: PathBuf,
    pub backend: Arc<Backend>,
    server: Option<Child>,
    _serial: MutexGuard<'static, ()>,
}
impl Fixture {
    pub fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for task in CLEANUP.lock().unwrap().drain(..) {
            task.join().unwrap();
        }
        let (base, binary, log) = BASE.get_or_init(|| {
            let root = tempfile::tempdir().unwrap().keep();
            let log = root.join("assistant.jsonl");
            for (key, value) in [
                (
                    "WISPY_CLAUDE_BIN",
                    workspace().join("testing/fake-cli/claude"),
                ),
                (
                    "WISPY_CODEX_BIN",
                    workspace().join("testing/fake-cli/codex"),
                ),
                ("WISPY_OPENER", workspace().join("testing/fake-cli/open")),
                ("WISPY_FAKE_LOG", log.clone()),
            ] {
                std::env::set_var(key, value);
            }
            std::env::set_var("WISPY_PREFETCH_INTERVAL", "3600");
            let base = root.join("fixture");
            let output = Command::new(env!("CARGO"))
                .current_dir(workspace())
                .args(["run", "-p", "wispy-fixtures", "--", "generate", "--out"])
                .arg(&base)
                .args(["--seed", "42"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "generation: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let meta = Command::new(env!("CARGO"))
                .current_dir(workspace())
                .args(["metadata", "--no-deps", "--format-version", "1"])
                .output()
                .unwrap();
            assert!(meta.status.success());
            let meta: serde_json::Value = serde_json::from_slice(&meta.stdout).unwrap();
            (
                base,
                PathBuf::from(meta["target_directory"].as_str().unwrap())
                    .join("debug/wispy-fixtures"),
                log,
            )
        });
        fs::write(log, "").unwrap();
        let _ = fs::remove_file(log.with_extension("jsonl.go"));
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("fixture");
        copy_tree(base, &fixture);
        let origin = fixture.join("origin");
        let manifest = fixture.join("fixture.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["origin"] = origin.to_string_lossy().into_owned().into();
        fs::write(manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut command = Command::new(binary);
        command
            .args(["serve", "--fixture"])
            .arg(fixture)
            .stderr(Stdio::null());
        let (server, port) = spawn_fixture_server(command);
        let backend = Backend::with_github(
            root.path().join("data"),
            format!("http://127.0.0.1:{port}"),
            "fixture-token".into(),
        );
        Self {
            root: Some(root),
            origin,
            backend,
            server: Some(server),
            _serial: serial,
        }
    }
    pub fn git(&self, args: &[&str]) -> String {
        Git::new(self.origin.join(".git"), None)
            .run_string(&[&["--work-tree", self.origin.to_str().unwrap()][..], args].concat())
            .unwrap()
    }
    pub fn show(&self, spec: &str) -> String {
        String::from_utf8(
            Git::new(self.origin.join(".git"), None)
                .run(&["show", spec])
                .unwrap(),
        )
        .unwrap()
    }
    pub fn write(&self, path: &str, text: &str) {
        let p = self.origin.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    pub fn push(&self, path: &str, text: &str) {
        self.git(&["checkout", "--quiet", "solo"]);
        self.write(path, text);
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "--quiet",
            "-m",
            "address review",
        ]);
        self.git(&["update-ref", "refs/pull/5/head", "solo"]);
        self.git(&["checkout", "--quiet", "main"]);
    }
    pub fn log(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&BASE.get().unwrap().2)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    pub fn go(&self) {
        fs::write(format!("{}.go", BASE.get().unwrap().2.display()), "").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let mut server = self.server.take().unwrap();
        // Always reap the owned HTTP process, including the binary’s final test.
        let _ = server.kill();
        let _ = server.wait();
        let root = self.root.take().unwrap();
        let backend = Arc::downgrade(&self.backend);
        CLEANUP.lock().unwrap().push(std::thread::spawn(move || {
            let start = Instant::now();
            while backend.strong_count() > 0 {
                if start.elapsed() > TIMEOUT {
                    eprintln!("fixture cleanup: backend still live");
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(root);
        }));
    }
}

pub struct Ui {
    pub window: AnyWindowHandle,
    pub app: Entity<WispyApp>,
    pub session: Entity<Session>,
}
impl Ui {
    pub fn mount(cx: &mut TestAppContext, fixture: &Fixture) -> Self {
        cx.executor().allow_parking();
        cx.update(|cx| wispydiff::init(fixture.backend.clone(), cx));
        let mut entities = None;
        let window = cx
            .open_window(size(px(1440.), px(900.)), |window, cx| {
                let session = cx.new(|cx| Session::new(fixture.backend.clone(), cx));
                let app = cx.new(|cx| WispyApp::new(session.clone(), window, cx));
                entities = Some((app.clone(), session));
                Root::new(app, window, cx)
            })
            .into();
        let (app, session) = entities.unwrap();
        let ui = Self {
            window,
            app,
            session,
        };
        ui.frame(cx);
        ui
    }
    pub fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, w, cx| w.render_frame(cx))
            .unwrap();
    }
    pub fn keys(&self, cx: &mut TestAppContext, keys: &str) {
        cx.simulate_keystrokes(self.window, keys);
        self.frame(cx);
    }
    pub fn input(&self, cx: &mut TestAppContext, text: &str) {
        cx.simulate_input(self.window, text);
        self.frame(cx);
    }
    pub fn diff(&self, cx: &impl AppContext) -> Entity<DiffViewer> {
        self.app.read_with(cx, |a, _| a.diff.clone().unwrap())
    }
    pub async fn wait(
        &self,
        cx: &mut TestAppContext,
        label: &str,
        mut predicate: impl FnMut(&App) -> bool,
    ) {
        let start = Instant::now();
        loop {
            self.frame(cx);
            if cx.update(|cx| predicate(cx)) {
                return;
            }
            assert!(start.elapsed() < TIMEOUT, "timed out: {label}");
            cx.executor().timer(Duration::from_millis(10)).await;
        }
    }
    pub async fn open(&self, cx: &mut TestAppContext, number: u64) {
        self.keys(cx, "ctrl-k");
        self.wait(cx, "palette", |cx| {
            self.app.read(cx).palette_view().is_some()
        })
        .await;
        self.keys(cx, "ctrl-a");
        self.input(cx, &format!("wispy/fixture#{number}"));
        self.keys(cx, "enter");
        self.wait(cx, "open real diff", |cx| {
            let s = self.session.read(cx);
            assert!(s.error.is_none(), "{:?}", s.error);
            !s.home
                && s.busy.is_none()
                && self.app.read(cx).palette_view().is_none()
                && s.stack
                    .as_ref()
                    .is_some_and(|s| s.prs[s.focus].number == number)
                && self
                    .app
                    .read(cx)
                    .diff
                    .as_ref()
                    .is_some_and(|d| d.read(cx).list.viewport_bounds().size.height > px(0.))
        })
        .await;
    }
    pub fn click(&self, cx: &mut TestAppContext, at: Point<Pixels>) {
        let mut v = VisualTestContext::from_window(self.window, cx);
        v.simulate_click(at, Default::default());
        self.frame(cx);
    }
    pub fn click_button(&self, cx: &mut TestAppContext, at: Point<Pixels>, button: MouseButton) {
        let mut v = VisualTestContext::from_window(self.window, cx);
        v.simulate_mouse_down(at, button, Default::default());
        v.simulate_mouse_up(at, button, Default::default());
        self.frame(cx);
    }
    pub fn snapshots(&self, cx: &mut TestAppContext) -> Vec<gpui_kit::test::ElementSnapshot> {
        cx.update_window(self.window, |_, w, _| observation::snapshots(w))
            .unwrap()
    }
    pub fn focus_diff(&self, cx: &mut TestAppContext) {
        let diff = self.diff(cx);
        cx.update_window(self.window, |_, w, cx| {
            w.focus(&diff.read(cx).focus_handle_diff(), cx)
        })
        .unwrap();
        self.frame(cx);
    }
    pub fn paths(&self, cx: &impl AppContext) -> Vec<String> {
        self.diff(cx).read_with(cx, |d, _| {
            d.view
                .summary
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect()
        })
    }
    pub fn viewed(&self, cx: &impl AppContext, path: &str) -> bool {
        self.diff(cx).read_with(cx, |d, cx| {
            d.view
                .summary
                .files
                .iter()
                .find(|f| f.path == path)
                .is_some_and(|f| d.is_viewed(&f.content_key, cx))
        })
    }
    pub fn since(&self, cx: &impl AppContext) -> bool {
        self.session
            .read_with(cx, |s, _| s.shown.as_ref().unwrap().since.is_some())
    }
    pub async fn toggle_since(&self, cx: &mut TestAppContext, expected: bool) {
        self.keys(cx, "d");
        self.wait(cx, "since range", |cx| {
            self.session
                .read(cx)
                .shown
                .as_ref()
                .is_some_and(|s| s.since.is_some() == expected)
                && self
                    .app
                    .read(cx)
                    .diff
                    .as_ref()
                    .is_some_and(|d| d.read(cx).test_since_c() == expected)
        })
        .await;
    }
    pub async fn mark(&self, cx: &mut TestAppContext) {
        self.keys(cx, "shift-m");
        self.wait(cx, "checkpoint toast", |cx| {
            self.app
                .read(cx)
                .toast_text()
                .is_some_and(|s| s.contains("Marked #5 as reviewed"))
        })
        .await;
    }
    pub async fn refresh(&self, cx: &mut TestAppContext) {
        self.session.update(cx, |s, _| s.refresh_inbox());
        self.wait(cx, "new commits offered", |cx| {
            self.session.read(cx).newer.is_some()
        })
        .await;
    }
    pub async fn load_newer(&self, cx: &mut TestAppContext) {
        // title-bar button; derives x from its fixed right-side layout
        self.click(cx, point(px(1210.), px(18.)));
        self.wait(cx, "load latest", |cx| {
            self.session.read(cx).newer.is_none()
        })
        .await;
    }
    pub fn row_bounds(
        &self,
        cx: &impl AppContext,
        file: usize,
        offset: u32,
    ) -> Option<Bounds<Pixels>> {
        self.diff(cx).read_with(cx, |d, _| {
            d.list
                .bounds_for_item(d.built.item_of_row(d.segment(file).start + offset))
        })
    }
    pub fn reveal_row(&self, cx: &mut TestAppContext, file: usize, offset: u32) {
        self.diff(cx).update(cx, |d, cx| {
            d.scroll_to_row(d.segment(file).start + offset, 3, Some(file), cx)
        });
        self.frame(cx);
    }
    pub fn changed_offsets(&self, cx: &impl AppContext, file: usize) -> Vec<u32> {
        self.diff(cx).read_with(cx, |d, _| (0..d.segment(file).rows).filter(|&o| matches!(d.unified_at(file, o), UnifiedRow::Row(r) if r.k == wispy_core::model::row_kind::ADDED || r.k == wispy_core::model::row_kind::DELETED)).collect())
    }
    pub fn code_position(
        &self,
        cx: &mut TestAppContext,
        file: usize,
        offset: u32,
        column: f32,
    ) -> Point<Pixels> {
        let bounds = self.row_bounds(cx, file, offset).expect("row painted");
        let diff = self.diff(cx);
        let (gutter, cw) = cx
            .update_window(self.window, |_, w, cx| {
                diff.update(cx, |d, cx| {
                    let g = wispydiff::diff::rows::gutter_width(d, cx);
                    (g, d.char_width(w, cx))
                })
            })
            .unwrap();
        point(
            bounds.left() + gutter * 2. + px(3. + 13. + 8.) + cw * column,
            bounds.center().y,
        )
    }
    pub async fn close(self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, w, _| w.remove_window())
            .unwrap();
        drop(self);
        cx.update(|cx| cx.clear_globals());
        cx.run_until_parked();
    }
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
