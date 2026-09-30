//! Background inbox refresh + prefetch: at launch, every few minutes, and whenever the window
//! gains focus, so every inbox stack opens instantly and offline.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Wakes the prefetch worker early.
pub struct PrefetchTrigger(Mutex<Sender<()>>);

impl PrefetchTrigger {
    pub fn fire(&self) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(());
    }
}

fn interval() -> Duration {
    let seconds = std::env::var("WISPY_PREFETCH_INTERVAL").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    Duration::from_secs(seconds)
}

pub fn start(app: &AppHandle) {
    let (sender, receiver) = channel::<()>();
    app.manage(PrefetchTrigger(Mutex::new(sender)));
    let app = app.clone();
    std::thread::spawn(move || loop {
        tauri::async_runtime::block_on(cycle(&app));
        match receiver.recv_timeout(interval()) {
            Ok(()) => while receiver.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    });
}

async fn cycle(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(service) = state.service() else { return };
    let groups = match service.refresh_inbox().await {
        Ok(groups) => groups,
        Err(err) => {
            eprintln!("inbox refresh failed (offline?): {err}");
            let _ = app.emit("inbox-status", "offline");
            return;
        }
    };
    let _ = app.emit("inbox-status", "online");
    emit_entries(app, &service, groups.clone());
    for group in &groups {
        if let Err(err) = service.prefetch(group).await {
            eprintln!("prefetch {}: {err}", group.key());
        }
        emit_entries(app, &service, groups.clone());
    }
    if let Err(err) = service.evict_finished(&groups).await {
        eprintln!("eviction: {err}");
    }
}

fn emit_entries(app: &AppHandle, service: &wispy_core::service::PrService, groups: Vec<wispy_core::inbox::InboxGroup>) {
    if let Ok(entries) = service.inbox_entries(groups) {
        let _ = app.emit("inbox-updated", entries);
    }
}
