mod commands;
mod prefetch;
mod shell_env;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    shell_env::adopt_login_shell_path();
    #[cfg(target_os = "linux")]
    disable_webkit_dmabuf_renderer();

    let builder = tauri::Builder::default();
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_wdio::init()).plugin(tauri_plugin_wdio_webdriver::init());

    builder
        .setup(|app| {
            let data_dir = state::data_dir(app.handle())?;
            app.manage(state::AppState::new(data_dir));
            // Resolve the token and compile highlight queries now, not on the first open.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let _ = handle.state::<state::AppState>().service();
                prefetch::start(&handle);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(true) = event {
                if let Some(trigger) = window.app_handle().try_state::<prefetch::PrefetchTrigger>() {
                    trigger.fire();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![commands::open_pr, commands::refresh_pr, commands::select_range,
            commands::get_rows,
            commands::get_split_rows,
            commands::get_row_widths,
            commands::get_ignore_patterns,
            commands::set_ignore_patterns,
            commands::list_drafts,
            commands::create_draft,
            commands::update_draft,
            commands::delete_draft,
            commands::accepts_line_comment,
            commands::list_threads,
            commands::locate_anchors,
            commands::prepare_submit,
            commands::submit_review,
            commands::select_since,
            commands::list_checkpoints,
            commands::mark_reviewed,
            commands::get_viewed,
            commands::set_viewed,
            commands::usages,
            commands::grep,
            commands::read_file,
            commands::locate_line,
            commands::ask_assistant,
            commands::list_assistant_threads,
            commands::delete_assistant_thread,
            commands::get_inbox,
            commands::refresh_inbox,
            commands::open_url])
        .run(tauri::generate_context!())
        .expect("error while running WispyDiff");
}

/// WebKitGTK's DMA-BUF renderer hangs Intel Raptor Lake GPUs (i915 with GuC), and because the
/// GuC then fails to reset the engine, the whole chip resets and the desktop freezes for seconds.
/// See https://gitlab.freedesktop.org/drm/i915/kernel/-/issues/15708. Drop this once that is fixed.
/// Must run before the webview (or any other thread) starts; an explicit setting wins.
#[cfg(target_os = "linux")]
fn disable_webkit_dmabuf_renderer() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}
