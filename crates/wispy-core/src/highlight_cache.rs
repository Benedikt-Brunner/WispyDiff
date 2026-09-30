use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::highlight::{Language, Seg};

pub type Lines = Arc<Vec<Vec<Seg>>>;

/// Highlighted blobs shared across the ranges of a stack (they reuse the same file versions),
/// so each blob is parsed once. Bounded by total cached lines; cleared wholesale when full.
pub struct HighlightCache {
    max_lines: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<(String, Option<Language>), Lines>,
    lines: usize,
}

impl HighlightCache {
    pub fn new(max_lines: usize) -> Self {
        HighlightCache { max_lines, inner: Mutex::new(Inner::default()) }
    }

    pub fn get(&self, blob: &str, language: Option<Language>) -> Option<Lines> {
        self.lock().entries.get(&(blob.to_string(), language)).cloned()
    }

    pub fn insert(&self, blob: String, language: Option<Language>, lines: Lines) {
        let mut inner = self.lock();
        if inner.lines + lines.len() > self.max_lines {
            inner.entries.clear();
            inner.lines = 0;
        }
        inner.lines += lines.len();
        inner.entries.insert((blob, language), lines);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Default for HighlightCache {
    /// Room for a few worst-case stacks' worth of file versions.
    fn default() -> Self {
        HighlightCache::new(3_000_000)
    }
}
