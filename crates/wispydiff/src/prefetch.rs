//! Background inbox refresh + prefetch: at launch, every few minutes, and whenever the window
//! gains focus, so every inbox stack opens instantly and offline.

use std::collections::HashSet;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::StreamExt;
use wispy_core::service::PrService;

use crate::backend::{Backend, RefreshProgress};

/// Inbox groups prefetched at once: each mostly waits on GitHub and git fetches.
const PARALLEL_PREFETCHES: usize = 5;

/// Wakes the prefetch worker early.
pub struct PrefetchTrigger(Mutex<Sender<()>>);

impl PrefetchTrigger {
    pub fn fire(&self) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(());
    }
}

fn interval() -> Duration {
    let seconds = std::env::var("WISPY_PREFETCH_INTERVAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    Duration::from_secs(seconds)
}

pub(crate) fn start(backend: Weak<Backend>) -> PrefetchTrigger {
    let (sender, receiver) = channel::<()>();
    std::thread::spawn(move || loop {
        let Some(backend) = backend.upgrade() else {
            return;
        };
        backend.block_on_prefetch();
        if backend.is_shut_down() {
            return;
        }
        // Do not retain the Backend (and its sender/runtime) while the worker is asleep.
        drop(backend);
        match receiver.recv_timeout(interval()) {
            Ok(()) => while receiver.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    });
    PrefetchTrigger(Mutex::new(sender))
}

pub(crate) async fn cycle(backend: &Backend) {
    let Ok(service) = backend.service() else {
        return;
    };
    backend.emit_inbox_progress(Some(RefreshProgress::Listing));
    refresh(backend, &service).await;
    backend.emit_inbox_progress(None);
}

async fn refresh(backend: &Backend, service: &Arc<PrService>) {
    let groups = match service.refresh_inbox().await {
        Ok(groups) => groups,
        Err(err) => {
            eprintln!("inbox refresh failed (offline?): {err}");
            backend.emit_inbox_status(false);
            return;
        }
    };
    backend.emit_inbox_status(true);
    // Groups not ready yet stay so until their own prefetch ends: while others run alongside,
    // their stack can be cached before its diffs are precomputed.
    let mut unready: HashSet<String> = HashSet::new();
    emit_entries(backend, service, groups.clone(), &unready);
    if let Ok(entries) = service.inbox_entries(groups.clone()) {
        unready.extend(entries.iter().filter(|e| !e.ready).map(|e| e.group.key()));
    }
    let total = groups.len();
    if total > 0 {
        backend.emit_inbox_progress(Some(RefreshProgress::Prefetching { done: 0, total }));
    }
    let handle = backend.runtime_handle();
    let mut prefetches = futures::stream::iter(groups.iter().cloned().map(|group| {
        let (service, handle) = (service.clone(), handle.clone());
        async move {
            let key = group.key();
            if backend.is_shut_down() {
                return key;
            }
            // Prefetching runs git and diffing between its network waits: on the blocking pool,
            // so groups run side by side without holding the runtime's workers.
            let blocking = handle.clone();
            let result = blocking
                .spawn_blocking(move || handle.block_on(service.prefetch(&group)).map(drop).map_err(|e| e.to_string()))
                .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => eprintln!("prefetch {key}: {err}"),
                Err(err) => eprintln!("prefetch {key}: {err}"),
            }
            key
        }
    }))
    .buffer_unordered(PARALLEL_PREFETCHES);
    let mut done = 0;
    while let Some(key) = prefetches.next().await {
        done += 1;
        unready.remove(&key);
        emit_entries(backend, service, groups.clone(), &unready);
        if done < total && !backend.is_shut_down() {
            backend.emit_inbox_progress(Some(RefreshProgress::Prefetching { done, total }));
        }
    }
    if backend.is_shut_down() {
        return;
    }
    if let Err(err) = service.evict_finished(&groups).await {
        eprintln!("eviction: {err}");
    }
}

fn emit_entries(
    backend: &Backend,
    service: &PrService,
    groups: Vec<wispy_core::inbox::InboxGroup>,
    unready: &HashSet<String>,
) {
    if let Ok(mut entries) = service.inbox_entries(groups) {
        for entry in &mut entries {
            entry.ready &= !unready.contains(&entry.group.key());
        }
        backend.emit_inbox(entries);
    }
}
