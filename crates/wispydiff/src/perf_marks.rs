//! Named timing marks (was `src/perf.ts`): a scenario starts with `mark("x:start")`; the view that
//! shows its result marks `x:first-visible` once that frame is presented. `--perf` reads them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static MARKS: Mutex<Option<HashMap<String, Vec<Instant>>>> = Mutex::new(None);

pub fn mark(name: &str) {
    let mut marks = MARKS.lock().unwrap_or_else(|p| p.into_inner());
    let marks = marks.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    if std::env::var_os("WISPY_DEBUG_TIMING").is_some() {
        let scenario = name.split(':').next().unwrap_or(name);
        let started = ["open", "range", "toggle", "view", "layout"]
            .iter()
            .filter_map(|s| marks.get(&format!("{s}:start")).and_then(|v| v.last()).map(|t| (*s, *t)))
            .filter(|(s, _)| *s == scenario || (scenario == "view" && (*s == "open" || *s == "range")) || (scenario == "layout" && *s == "toggle"))
            .max_by_key(|(_, t)| *t);
        match started {
            Some((from, t)) => eprintln!("[timing] {name} {:.1} ms after {from}:start", (now - t).as_secs_f64() * 1000.0),
            None => eprintln!("[timing] {name}"),
        }
    }
    marks.entry(name.to_string()).or_default().push(now);
}

/// How many times `name` was marked.
pub fn count(name: &str) -> usize {
    MARKS.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(name)).map_or(0, Vec::len)
}

/// The time between the latest `from` mark and the latest `to` mark after it.
pub fn between(from: &str, to: &str) -> Option<Duration> {
    let marks = MARKS.lock().unwrap_or_else(|p| p.into_inner());
    let marks = marks.as_ref()?;
    let start = *marks.get(from)?.last()?;
    let end = *marks.get(to)?.iter().rev().find(|t| **t >= start)?;
    Some(end - start)
}

static PLACEHOLDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A row drawn empty because its data wasn't loaded yet (the perf script counts these).
pub fn note_placeholder() {
    PLACEHOLDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn placeholders() -> usize {
    PLACEHOLDERS.load(std::sync::atomic::Ordering::Relaxed)
}
