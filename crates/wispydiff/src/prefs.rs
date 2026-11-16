//! Per-viewer conveniences. Missing, malformed or unwritable storage never prevents use.
//! Preferences and recents persist; `Lasting` is session memory, like `src/lasting.ts`.

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

pub struct Prefs {
    path: PathBuf,
    values: Mutex<Map<String, Value>>,
}

impl Prefs {
    /// Loads once; absent or malformed storage starts with the caller's defaults.
    pub fn load(dir: impl AsRef<Path>) -> Self {
        let path = dir.as_ref().join("prefs.json");
        let values = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            values: Mutex::new(values),
        }
    }

    /// Typed values are validated by deserialization; callers choose their defaults.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_value(lock(&self.values).get(key)?.clone()).ok()
    }

    pub fn set<T: Serialize>(&self, key: &str, value: T) {
        let Ok(value) = serde_json::to_value(value) else {
            return;
        };
        let mut values = lock(&self.values);
        if values.get(key) == Some(&value) {
            return;
        }
        values.insert(key.to_string(), value);
        self.persist(&values);
    }

    /// Returns only an allowed value, or the supplied default (as `loadPref` did).
    pub fn get_str(&self, key: &str, allowed: &[&str], fallback: &str) -> String {
        self.get::<String>(key)
            .filter(|s| allowed.contains(&s.as_str()))
            .unwrap_or_else(|| fallback.to_string())
    }

    pub fn set_str(&self, key: &str, value: &str) {
        self.set(key, value);
    }

    /// Malformed lists become empty; non-string entries are discarded.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        self.get::<Vec<Value>>(key)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect()
    }

    pub fn set_list(&self, key: &str, values: &[String]) {
        self.set(key, values);
    }

    /// A JSON object of per-key entries; entries are validated by the caller.
    pub fn get_record(&self, key: &str) -> Map<String, Value> {
        self.get(key).unwrap_or_default()
    }

    pub fn recent_prs(&self) -> Vec<String> {
        self.get_list("recentPrs")
    }

    /// Most recent first, without duplicates, capped at 30 entries.
    pub fn remember_pr(&self, label: &str) {
        let mut values = lock(&self.values);
        let mut recent = vec![Value::String(label.to_string())];
        if let Some(old) = values.get("recentPrs").and_then(Value::as_array) {
            recent.extend(
                old.iter()
                    .filter(|v| v.as_str().is_some_and(|s| s != label))
                    .take(29)
                    .cloned(),
            );
        }
        values.insert("recentPrs".into(), Value::Array(recent));
        self.persist(&values);
    }

    fn persist(&self, values: &Map<String, Value>) {
        // Serialize writes under the values lock; the temporary file is always in the same
        // directory, so rename is atomic even when the data directory is on another volume.
        let _ = (|| -> std::io::Result<()> {
            let dir = self.path.parent().expect("prefs directory");
            std::fs::create_dir_all(dir)?;
            let temp = dir.join(format!("prefs-{}.tmp", wispy_core::drafts::new_id()));
            let result = (|| {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)?;
                file.write_all(&serde_json::to_vec(values)?)?;
                file.sync_all()?;
                std::fs::rename(&temp, &self.path)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(temp);
            }
            result
        })();
    }
}

/// Carries the preferences of the Tauri app (WebKit localStorage, keys `wispy.*`) over into a
/// new `prefs.json` in `dir`, once: nothing happens when `prefs.json` exists or nothing is found.
pub fn import_web_storage(dir: &Path) {
    let target = dir.join("prefs.json");
    if target.exists() {
        return;
    }
    let Some(values) = web_storage_files().iter().find_map(|file| read_web_storage(file, dir)) else { return };
    let prefs = Prefs::load(dir);
    for (key, value) in values {
        prefs.set(&key, value);
    }
}

/// Where WebKit kept the old app's localStorage (`dev.wispydiff.WispyDiff`).
fn web_storage_files() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut found = Vec::new();
    if cfg!(target_os = "macos") {
        // Newer WebKit: WebsiteData/Default/<salted origin>/<salted origin>/LocalStorage/localstorage.sqlite3.
        let root = home.join("Library/WebKit/dev.wispydiff.WispyDiff/WebsiteData");
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().is_some_and(|n| n == "localstorage.sqlite3") {
                    found.push(path);
                }
            }
        }
        found.push(root.join("LocalStorage/tauri_localhost_0.localstorage"));
    } else {
        let data = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        found.push(data.join("dev.wispydiff.WispyDiff/localstorage/tauri_localhost_0.localstorage"));
    }
    found.retain(|f| f.is_file());
    found
}

/// The `wispy.*` entries of a localStorage database, as typed preference values (lists and
/// records were JSON strings, widths numbers). Reads a copy: the live file may be mid-WAL.
fn read_web_storage(file: &Path, scratch: &Path) -> Option<Vec<(String, Value)>> {
    let copy = scratch.join(format!("web-storage-{}", wispy_core::drafts::new_id()));
    std::fs::create_dir_all(&copy).ok()?;
    let read = (|| {
        let db = copy.join("storage.sqlite");
        std::fs::copy(file, &db).ok()?;
        for suffix in ["-wal", "-shm"] {
            let mut side = file.as_os_str().to_owned();
            side.push(suffix);
            let mut to = db.as_os_str().to_owned();
            to.push(suffix);
            let _ = std::fs::copy(PathBuf::from(side), PathBuf::from(to));
        }
        let conn = rusqlite::Connection::open(&db).ok()?;
        let mut statement = conn.prepare("SELECT key, value FROM ItemTable WHERE key LIKE 'wispy.%'").ok()?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))
            .ok()?
            .flatten()
            .filter_map(|(key, value)| {
                let units: Vec<u16> = value.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
                let text = String::from_utf16(&units).ok()?;
                let key = key.strip_prefix("wispy.")?.to_string();
                Some((key.clone(), web_value(&key, text)))
            })
            .collect::<Vec<_>>();
        (!rows.is_empty()).then_some(rows)
    })();
    let _ = std::fs::remove_dir_all(&copy);
    read
}

fn web_value(key: &str, text: String) -> Value {
    if key.starts_with("width.") {
        if let Ok(width) = text.parse::<f64>() {
            return Value::from(width);
        }
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(json @ (Value::Array(_) | Value::Object(_))) => json,
        _ => Value::String(text),
    }
}

#[derive(Default)]
struct LastingState {
    values: HashMap<String, Box<dyn Any + Send + Sync>>,
    listeners: HashMap<String, Arc<dyn Fn() + Send + Sync>>,
}

/// State that outlives a view, but never persists between app sessions. Updates notify
/// subscribers after releasing the lock, allowing callbacks to read the new state.
#[derive(Clone, Default)]
pub struct Lasting(Arc<Mutex<LastingState>>);

impl Lasting {
    pub fn get<T: Clone + Send + Sync + 'static>(&self, key: &str, initial: T) -> T {
        lock(&self.0)
            .values
            .get(key)
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
            .unwrap_or(initial)
    }

    pub fn set<T: Clone + PartialEq + Send + Sync + 'static>(
        &self,
        key: &str,
        value: T,
        initial: T,
    ) {
        self.update(key, initial, |_| value);
    }

    pub fn update<T: Clone + PartialEq + Send + Sync + 'static>(
        &self,
        key: &str,
        initial: T,
        next: impl FnOnce(T) -> T,
    ) {
        let listeners = {
            let mut state = lock(&self.0);
            let current = state
                .values
                .get(key)
                .and_then(|v| v.downcast_ref::<T>())
                .cloned()
                .unwrap_or(initial);
            let value = next(current.clone());
            if value == current {
                return;
            }
            state.values.insert(key.to_string(), Box::new(value));
            state.listeners.values().cloned().collect::<Vec<_>>()
        };
        for listener in listeners {
            listener();
        }
    }

    /// Drop the subscription to stop receiving updates (as on component unmount).
    pub fn subscribe(&self, listener: impl Fn() + Send + Sync + 'static) -> Subscription {
        let id = wispy_core::drafts::new_id();
        lock(&self.0)
            .listeners
            .insert(id.clone(), Arc::new(listener));
        Subscription {
            state: Arc::downgrade(&self.0),
            id,
        }
    }
}

pub struct Subscription {
    state: Weak<Mutex<LastingState>>,
    id: String,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            lock(&state).listeners.remove(&self.id);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn defaults_validation_and_atomic_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Prefs::load(dir.path());
        assert_eq!(prefs.get_str("theme", &["light", "dark"], "light"), "light");
        prefs.set_str("theme", "invalid");
        assert_eq!(prefs.get_str("theme", &["light", "dark"], "dark"), "dark");
        prefs.set_str("theme", "light");
        prefs.set("list", json!(["one", 2, null, "two"]));
        assert_eq!(prefs.get_list("list"), ["one", "two"]);
        prefs.set("record", json!({"pr": [1, 2]}));
        prefs.set("wrong", json!([]));
        assert!(prefs.get_record("wrong").is_empty());
        assert!(prefs.get::<u32>("theme").is_none());
        let loaded = Prefs::load(dir.path());
        assert_eq!(loaded.get_record("record")["pr"], json!([1, 2]));
        assert_eq!(loaded.get_str("theme", &["light", "dark"], "dark"), "light");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        // Loading is once per store, not a read on every get.
        std::fs::write(dir.path().join("prefs.json"), "broken").unwrap();
        assert_eq!(loaded.get_list("list"), ["one", "two"]);
        assert!(Prefs::load(dir.path()).get_list("list").is_empty());
    }

    #[test]
    fn recents_are_deduplicated_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Prefs::load(dir.path());
        for i in 0..40 {
            prefs.remember_pr(&format!("owner/repo#{i}"));
        }
        prefs.remember_pr("owner/repo#20");
        let recent = Prefs::load(dir.path()).recent_prs();
        assert_eq!(recent.len(), 30);
        assert_eq!(recent[0], "owner/repo#20");
        assert_eq!(recent.iter().filter(|s| *s == "owner/repo#20").count(), 1);
    }

    #[test]
    fn unavailable_storage_still_keeps_session_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        let prefs = Prefs::load(file);
        prefs.set_list("names", &["test".into()]);
        assert_eq!(prefs.get_list("names"), ["test"]);
    }

    #[test]
    fn imports_the_web_apps_local_storage_once() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().join("tauri_localhost_0.localstorage");
        let conn = rusqlite::Connection::open(&storage).unwrap();
        conn.execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL)", []).unwrap();
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        for (key, value) in [
            ("wispy.theme", "dark"),
            ("wispy.width.files", "617"),
            ("wispy.recentPrs", r#"["o/r#2","o/r#1"]"#),
            ("wispy.fileFilters", r#"{"o/r#2":{"exts":["ts"]}}"#),
            ("other", "ignored"),
        ] {
            conn.execute("INSERT INTO ItemTable VALUES (?1, ?2)", rusqlite::params![key, utf16(value)]).unwrap();
        }
        drop(conn);
        let data = dir.path().join("data");
        let values = read_web_storage(&storage, &data).unwrap();
        let prefs = Prefs::load(&data);
        for (key, value) in values {
            prefs.set(&key, value);
        }
        let prefs = Prefs::load(&data);
        assert_eq!(prefs.get_str("theme", &["light", "dark"], "light"), "dark");
        assert_eq!(prefs.get::<f32>("width.files"), Some(617.));
        assert_eq!(prefs.recent_prs(), ["o/r#2", "o/r#1"]);
        assert_eq!(prefs.get_record("fileFilters")["o/r#2"], json!({"exts": ["ts"]}));
        assert!(prefs.get::<String>("other").is_none());
        // Only the prefs file is left behind (the copy is cleaned up).
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 1);
    }

    #[test]
    fn lasting_is_shared_session_memory_with_notifications() {
        let state = Lasting::default();
        let clone = state.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let subscription = state.subscribe(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        state.set("answer", 0, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        state.update("answer", 0, |current| current + 1);
        assert_eq!(clone.get("answer", 0), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(Lasting::default().get("answer", 0), 0);
        drop(subscription);
        state.set("answer", 2, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
