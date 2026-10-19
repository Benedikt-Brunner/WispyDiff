use std::sync::mpsc::channel;
use std::time::Duration;

use serde_json::json;
use std::sync::Arc;

use wispy_core::cache::Cache;
use wispy_core::highlight::Language;
use wispy_core::highlight_cache::{BlobStore, HighlightCache};

#[test]
fn reads_do_not_wait_for_a_write_that_is_stuck() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("cache.sqlite");
    let cache = Cache::open(&path).unwrap();
    cache.put_app_value("inbox", &json!(["before"])).unwrap();

    // Another connection holds SQLite's write lock, so the cache's next write waits inside SQLite
    // (the way a large view being stored or a checkpoint on a slow disk keeps the writer busy).
    let other = rusqlite::Connection::open(&path).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| cache.put_app_value("inbox", &json!(["after"])));
        std::thread::sleep(Duration::from_millis(100)); // let the write get stuck first

        let (tx, rx) = channel();
        let cache = &cache;
        scope.spawn(move || tx.send(cache.app_value("inbox").unwrap()));
        let read = rx.recv_timeout(Duration::from_secs(2));

        other.execute_batch("COMMIT").unwrap();
        writer.join().unwrap().unwrap();
        assert_eq!(read.expect("the read waited for the write"), Some(json!(["before"])));
    });
    assert_eq!(cache.app_value("inbox").unwrap(), Some(json!(["after"])), "reads see committed writes");
}

#[test]
fn an_in_memory_cache_reads_its_own_writes() {
    let cache = Cache::open_in_memory().unwrap();
    cache.put_app_value("inbox", &json!([1])).unwrap();
    assert_eq!(cache.app_value("inbox").unwrap(), Some(json!([1])));
}

#[test]
fn files_highlighted_by_an_older_highlighter_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let lines = Arc::new(vec![vec![(1u8, "SELECT".to_string())]]);
    {
        let cache = Arc::new(Cache::open(&path).unwrap());
        cache.put_many(&[("abc:Some(Php)".to_string(), lines.clone())]);
        HighlightCache::new(1_000, Some(cache)).insert_many(vec![("def".to_string(), Some(Language::Php), lines.clone())]);
    }
    let cache = Arc::new(Cache::open(&path).unwrap());
    assert_eq!(cache.get("abc:Some(Php)"), None, "unversioned entry is gone");
    let highlights = HighlightCache::new(1_000, Some(cache));
    assert_eq!(highlights.get("def", Some(Language::Php)), Some(lines), "current entry survives");
}
