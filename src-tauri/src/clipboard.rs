//! Images on the clipboard. WebKitGTK's paste event leaves them out, so on Linux they're read
//! from GTK's clipboard instead.

/// The clipboard's image as PNG, unless it holds text (the text box pastes that itself).
#[cfg(target_os = "linux")]
pub fn image_png(app: &tauri::AppHandle) -> Result<Option<Vec<u8>>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    // GTK only runs on the main thread.
    app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let png = if clipboard.wait_is_text_available() {
            None
        } else {
            clipboard.wait_for_image().and_then(|pixbuf| pixbuf.save_to_bufferv("png", &[]).ok())
        };
        let _ = tx.send(png);
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())
}

/// Elsewhere the paste event carries images itself.
#[cfg(not(target_os = "linux"))]
pub fn image_png(_app: &tauri::AppHandle) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}
