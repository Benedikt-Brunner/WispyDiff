//! WispyDiff: distraction-free review of stacked GitHub PRs, on GPUI.
//!
//! `wispydiff [owner/repo#N]` opens that PR at launch; `--perf owner/repo#N` runs the scripted
//! performance scenarios against it and prints JSON (see `perf.rs`).

pub mod app;
pub mod backend;
pub mod diff;
pub mod http;
pub mod inbox;
pub mod logic;
pub mod markdown;
pub mod menu;
pub mod overlays;
pub mod palette;
mod perf;
mod script;
pub mod perf_marks;
pub mod prefetch;
pub mod prefs;
pub mod session;
pub mod settings;
pub mod submit_sheet;
pub mod text;
pub mod theme;
pub mod ui;
pub mod wrap;

use std::sync::Arc;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{px, size, App, Bounds, WindowBounds, WindowOptions};

use crate::app::WispyApp;
use crate::backend::Backend;
use crate::session::Session;

/// Key bindings and component setup; once per app (tests too).
pub fn init(backend: Arc<Backend>, cx: &mut App) {
    gpui_kit::init(cx);
    settings::init(backend, cx);
    app::bind_keys(cx);
    menu::bind_keys(cx);
    inbox::bind_keys(cx);
    palette::bind_keys(cx);
    overlays::bind_keys(cx);
    submit_sheet::bind_keys(cx);
    diff::bind_keys(cx);
    diff::inserts::bind_keys(cx);
    diff::file_list::bind_keys(cx);
    diff::file_search::bind_keys(cx);
    diff::code_panel::bind_keys(cx);
    diff::assistant::bind_keys(cx);
    let theme = app::theme_from_prefs(cx);
    let zoom = app::saved_zoom(cx);
    ui::Looks::init(theme, false, zoom, cx);
}

/// The window's root: the app (with its session) inside gpui-kit's `Root`.
pub fn open_window(backend: Arc<Backend>, initial: Option<String>, perf: bool, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
    cx.spawn(async move |cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                app_id: Some("wispydiff".into()),
                // A scripted run may not get the focus; measure it like a focused window.
                inactive_frame_interval: if perf || std::env::var_os("WISPY_NO_THROTTLE").is_some() { None } else { WindowOptions::default().inactive_frame_interval },
                ..TitleBar::window_options()
            },
            |window, cx| {
                window.set_window_title("WispyDiff");
                let dark = matches!(window.appearance(), gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark);
                ui::Looks::set_system_dark(dark, cx);
                let session = cx.new(|cx| Session::new(backend.clone(), cx));
                let app = cx.new(|cx| WispyApp::new(session.clone(), window, cx));
                if perf {
                    perf::start(app.clone(), initial.clone().expect("--perf needs a PR to open"), window, cx);
                } else if let Some(pr) = &initial {
                    session.update(cx, |s, cx| s.open(pr.clone(), cx));
                }
                if let Ok(steps) = std::env::var("WISPY_SCRIPT") {
                    script::start(steps, window, cx);
                }
                cx.new(|cx| Root::new(app, window, cx))
            },
        )
        .expect("failed to open the WispyDiff window");
    })
    .detach();
}

pub fn run() {
    if let Some(path) = wispy_core::shell_env::login_shell_path() {
        std::env::set_var("PATH", path);
    }
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let perf = args.iter().position(|a| a == "--perf").map(|at| args.remove(at)).is_some();
    let initial = args.first().cloned();

    let data_dir = backend::default_data_dir();
    if std::env::var_os("WISPY_DATA_DIR").is_none() {
        prefs::import_web_storage(&data_dir);
    }
    let backend = Backend::new(data_dir);
    backend.warm_up();

    gpui_kit::application().with_assets(ui::Assets).with_http_client(http::ReqwestClient::new(backend.runtime_handle())).run(move |cx| {
        init(backend.clone(), cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        open_window(backend, initial, perf, cx);
    });
}
