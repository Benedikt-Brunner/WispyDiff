//! Real production root and isolated real-IO fixture for comments/inbox UI suites.
#![allow(dead_code)]
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::component::Root;
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    px, size, AnyWindowHandle, App, AppContext, Entity, Focusable, TestAppContext,
    VisualTestContext,
};
use wispydiff::{app::WispyApp, backend::Backend, diff::DiffViewer, session::Session};

pub const TIMEOUT: Duration = Duration::from_secs(30);
static GENERATED: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();

pub struct Fixture {
    pub data: tempfile::TempDir,
    server: Child,
    pub port: u16,
}
impl Fixture {
    pub fn new() -> Self {
        let (generated, executable) = GENERATED.get_or_init(|| {
            // These are process-wide and set before any backend/worker starts.
            std::env::set_var("WISPY_PREFETCH_INTERVAL", "3600");
            std::env::set_var("WISPY_OPENER", "/bin/true");
            let dir = tempfile::tempdir().unwrap();
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            let result = Command::new(env!("CARGO"))
                .current_dir(&workspace)
                .args(["run", "-p", "wispy-fixtures", "--", "generate", "--out"])
                .arg(dir.path())
                .args(["--seed", "42"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "fixture: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let metadata = Command::new(env!("CARGO"))
                .current_dir(workspace)
                .args(["metadata", "--no-deps", "--format-version", "1"])
                .output()
                .unwrap();
            assert!(metadata.status.success());
            let value: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
            (
                dir,
                PathBuf::from(value["target_directory"].as_str().unwrap())
                    .join("debug/wispy-fixtures"),
            )
        });
        let mut command = Command::new(executable);
        command
            .args(["serve", "--fixture"])
            .arg(generated.path())
            .stderr(Stdio::null());
        let (server, port) = spawn_fixture_server(command);
        Self {
            data: tempfile::tempdir().unwrap(),
            server,
            port,
        }
    }
    pub fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub fn get(&self, path: &str) -> serde_json::Value {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        serde_json::from_str(body.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

pub struct Ui {
    pub app: Entity<WispyApp>,
    pub session: Entity<Session>,
    pub window: AnyWindowHandle,
}
impl Ui {
    pub fn new(backend: Arc<Backend>, cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();
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
        Self {
            app,
            session,
            window,
        }
    }
    pub fn press(&self, cx: &mut TestAppContext, keys: &str) {
        for key in keys.split_whitespace() {
            self.frame(cx);
            cx.simulate_keystrokes(self.window, key);
            self.frame(cx);
        }
    }
    pub fn input(&self, cx: &mut TestAppContext, text: &str) {
        self.frame(cx);
        cx.simulate_input(self.window, text);
        self.frame(cx);
    }
    pub fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    pub async fn wait(&self, cx: &mut TestAppContext, mut f: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        cx.wait_for(self.window, TIMEOUT, |_, cx| {
            let ready = f(cx);
            assert!(
                ready || Instant::now() < deadline,
                "UI condition exceeded real IO timeout"
            );
            ready
        })
        .await;
    }
    pub async fn open(&self, cx: &mut TestAppContext, pr: u64) -> Entity<DiffViewer> {
        self.press(cx, secondary("k"));
        self.wait(cx, |cx| self.app.read(cx).palette_view().is_some())
            .await;
        self.input(cx, &format!("wispy/fixture#{pr}"));
        self.press(cx, "enter");
        self.wait(cx, |cx| {
            let s = self.session.read(cx);
            assert!(s.error.is_none(), "open: {:?}", s.error);
            !s.home
                && s.shown
                    .as_ref()
                    .is_some_and(|v| v.range.lo == pr.saturating_sub(1).min(3) as usize || pr == 5)
                && self
                    .app
                    .read(cx)
                    .diff
                    .as_ref()
                    .is_some_and(|d| !d.read(cx).built.items.is_empty())
                && s.busy.is_none()
        })
        .await;
        self.diff(cx)
    }
    pub fn diff(&self, cx: &TestAppContext) -> Entity<DiffViewer> {
        self.app.read_with(cx, |a, _| a.diff.clone().unwrap())
    }
    pub async fn threads(&self, cx: &mut TestAppContext) {
        self.wait(cx, |cx| !self.session.read(cx).threads.is_empty())
            .await;
    }
    pub async fn drafts(&self, cx: &mut TestAppContext, count: usize) {
        self.wait(cx, |cx| self.session.read(cx).drafts.len() == count)
            .await;
    }
    pub async fn sheet(&self, cx: &mut TestAppContext) {
        self.press(cx, "r");
        cx.wait_for(self.window, TIMEOUT, |window, cx| {
            self.app
                .read(cx)
                .sheet
                .as_ref()
                .is_some_and(|s| window.focused(cx).is_some_and(|f| f != s.focus_handle(cx)))
        })
        .await;
    }
    pub async fn submitted(&self, cx: &mut TestAppContext) {
        self.press(cx, secondary("enter"));
        self.wait(cx, |cx| self.app.read(cx).sheet.is_none()).await;
    }
    pub async fn home(&self, cx: &mut TestAppContext) {
        self.press(cx, secondary("i"));
        self.wait(cx, |cx| {
            self.session.read(cx).home && self.session.read(cx).inbox.len() == 2
        })
        .await;
    }
    pub async fn close(self, cx: &mut TestAppContext) {
        let backend = self
            .session
            .read_with(cx, |s, _| Arc::downgrade(&s.backend));
        cx.update_window(self.window, |_, w, _| w.remove_window())
            .unwrap();
        cx.update(|cx| {
            cx.remove_global::<wispydiff::settings::Settings>();
        });
        drop(self);
        cx.update(|_| {});
        cx.run_until_parked();
        let deadline = Instant::now() + TIMEOUT;
        while backend.strong_count() != 0 {
            cx.update(|_| {});
            assert!(
                Instant::now() < deadline,
                "backend workers did not finish before fixture cleanup"
            );
            cx.executor().timer(Duration::from_millis(10)).await;
        }
    }
}
pub fn secondary(key: &str) -> &'static str {
    match (cfg!(target_os = "macos"), key) {
        (true, "a") => "cmd-a",
        (false, "a") => "ctrl-a",
        (true, "c") => "cmd-c",
        (false, "c") => "ctrl-c",
        (true, "k") => "cmd-k",
        (false, "k") => "ctrl-k",
        (true, "i") => "cmd-i",
        (false, "i") => "ctrl-i",
        (true, "b") => "cmd-b",
        (false, "b") => "ctrl-b",
        (true, "g") => "cmd-g",
        (false, "g") => "ctrl-g",
        (true, "enter") => "cmd-enter",
        (false, "enter") => "ctrl-enter",
        (true, "1") => "cmd-1",
        (false, "1") => "ctrl-1",
        (true, "2") => "cmd-2",
        (false, "2") => "ctrl-2",
        (true, "3") => "cmd-3",
        (false, "3") => "ctrl-3",
        (true, "shift-m") => "cmd-shift-m",
        (false, "shift-m") => "ctrl-shift-m",
        _ => panic!("key {key}"),
    }
}
pub fn inbox_select(ui: &Ui, cx: &mut TestAppContext, pr: u64) {
    let index = ui.session.read_with(cx, |s, _| {
        s.inbox
            .iter()
            .flat_map(|e| &e.group.prs)
            .position(|p| p.number == pr)
            .unwrap()
    });
    for _ in 0..6 {
        ui.press(cx, "k");
    }
    for _ in 0..index {
        ui.press(cx, "j");
    }
}
/// The selection's actual painted rectangle (no hard-coded inbox coordinates).
pub fn inbox_selection_bounds(
    ui: &Ui,
    cx: &mut TestAppContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    ui.frame(cx);
    cx.update_window(ui.window, |_, window, cx| {
        let background = gpui_kit::solid_background(wispydiff::ui::palette(cx).selection);
        let quad = window
            .painted_quads()
            .into_iter()
            .find(|q| q.background == background && q.bounds.size.width.0 > 300.)
            .expect("painted inbox selection");
        let scale = window.scale_factor();
        gpui_kit::Bounds::new(
            gpui_kit::point(
                px(quad.bounds.origin.x.0 / scale),
                px(quad.bounds.origin.y.0 / scale),
            ),
            size(
                px(quad.bounds.size.width.0 / scale),
                px(quad.bounds.size.height.0 / scale),
            ),
        )
    })
    .unwrap()
}
pub fn click(ui: &Ui, cx: &mut TestAppContext, position: gpui_kit::Point<gpui_kit::Pixels>) {
    ui.frame(cx);
    VisualTestContext::from_window(ui.window, cx).simulate_click(position, Default::default());
    ui.frame(cx);
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
