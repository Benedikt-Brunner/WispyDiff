use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::highlight::{Language, Seg};

pub type Lines = Arc<Vec<Vec<Seg>>>;

/// Durable storage behind the in-memory cache (the SQLite cache), so highlighted files
/// survive restarts and serve side-by-side views offline.
pub trait BlobStore: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<Vec<Seg>>>;
    fn put_many(&self, items: &[(String, Lines)]);
}

/// Highlighted blobs shared across the ranges of a stack (they reuse the same file versions),
/// so each blob is parsed once. Bounded by total cached lines; cleared wholesale when full.
pub struct HighlightCache {
    max_lines: usize,
    inner: Mutex<Inner>,
    store: Option<Arc<dyn BlobStore>>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Lines>,
    lines: usize,
}

impl HighlightCache {
    pub fn new(max_lines: usize, store: Option<Arc<dyn BlobStore>>) -> Self {
        HighlightCache { max_lines, inner: Mutex::new(Inner::default()), store }
    }

    pub fn get(&self, blob: &str, language: Option<Language>) -> Option<Lines> {
        let key = key(blob, language);
        if let Some(lines) = self.lock().entries.get(&key).cloned() {
            return Some(lines);
        }
        let lines: Lines = Arc::new(self.store.as_ref()?.get(&key)?);
        self.remember(key, lines.clone());
        Some(lines)
    }

    pub fn insert_many(&self, items: Vec<(String, Option<Language>, Lines)>) {
        let keyed: Vec<(String, Lines)> = items.into_iter().map(|(blob, lang, lines)| (key(&blob, lang), lines)).collect();
        if let Some(store) = &self.store {
            store.put_many(&keyed);
        }
        for (key, lines) in keyed {
            self.remember(key, lines);
        }
    }

    fn remember(&self, key: String, lines: Lines) {
        let mut inner = self.lock();
        if inner.lines + lines.len() > self.max_lines {
            inner.entries.clear();
            inner.lines = 0;
        }
        inner.lines += lines.len();
        inner.entries.insert(key, lines);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn key(blob: &str, language: Option<Language>) -> String {
    format!("{blob}:{language:?}")
}

impl Default for HighlightCache {
    /// In-memory only, with room for a few worst-case stacks' worth of file versions.
    fn default() -> Self {
        HighlightCache::new(3_000_000, None)
    }
}
