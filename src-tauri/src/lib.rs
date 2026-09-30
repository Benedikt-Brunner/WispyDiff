mod commands;
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
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::open_pr, commands::refresh_pr, commands::get_rows])
        .run(tauri::generate_context!())
        .expect("error while running WispyDiff");
}
