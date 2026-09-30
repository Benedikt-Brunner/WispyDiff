mod commands;
mod prefetch;
mod shell_env;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    shell_env::adopt_login_shell_path();

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
            commands::get_inbox,
            commands::refresh_inbox])
        .run(tauri::generate_context!())
        .expect("error while running WispyDiff");
}
