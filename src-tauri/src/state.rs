use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::Manager;
use wispy_core::cache::Cache;
use wispy_core::github::{GitHubClient, DEFAULT_API_BASE};
use wispy_core::highlight::Highlighter;
use wispy_core::model::DiffView;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;
use wispy_core::split::SplitRow;
use wispy_core::stack::StackSnapshot;
use wispy_core::token::resolve_github_token;

/// Diff views kept in memory; others are re-read from SQLite (~25 ms for the worst case).
const MAX_OPEN_VIEWS: usize = 4;
/// Side-by-side files kept in memory.
const MAX_SPLIT_FILES: usize = 32;

/// `WISPY_DATA_DIR` overrides the location (tests); default `~/Library/Application Support/WispyDiff`.
pub fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(dir) = std::env::var("WISPY_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(app.path().data_dir()?.join("WispyDiff"))
}

/// Identifies a stack at specific SHAs; any push produces a new id.
pub fn stack_id(snapshot: &StackSnapshot) -> String {
    let prs: Vec<String> =
        snapshot.stack.prs.iter().zip(&snapshot.heads).map(|(pr, head)| format!("{}@{}", pr.number, &head[..12.min(head.len())])).collect();
    format!("{}:{}", snapshot.repo_slug(), prs.join(","))
}

pub fn view_id(stack_id: &str, lo: usize, hi: usize, ignore_whitespace: bool) -> String {
    format!("{stack_id}:{lo}-{hi}{}", if ignore_whitespace { ":w" } else { "" })
}

struct OpenView {
    id: String,
    stack_id: String,
    /// Stack index of the range's top PR.
    top: u8,
    view: Arc<DiffView>,
}

pub struct AppState {
    data_dir: PathBuf,
    service: OnceLock<Result<Arc<PrService>, String>>,
    stacks: Mutex<HashMap<String, Arc<StackSnapshot>>>,
    views: Mutex<VecDeque<OpenView>>,
    split_files: Mutex<VecDeque<(String, Arc<Vec<SplitRow>>)>>,
    /// The stack whose ranges the background worker should be precomputing.
    current_stack: Mutex<Option<String>>,
    /// Stacks with a precompute worker running.
    precomputing: Mutex<HashSet<String>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        AppState {
            data_dir,
            service: OnceLock::new(),
            stacks: Mutex::new(HashMap::new()),
            views: Mutex::new(VecDeque::new()),
            split_files: Mutex::new(VecDeque::new()),
            current_stack: Mutex::new(None),
            precomputing: Mutex::new(HashSet::new()),
        }
    }

    /// Created lazily so a missing `gh` login is reported on first use instead of at launch.
    pub fn service(&self) -> Result<Arc<PrService>, String> {
        self.service.get_or_init(|| self.create_service().map(Arc::new)).clone()
    }

    fn create_service(&self) -> Result<PrService, String> {
        let token = resolve_github_token().map_err(|e| e.to_string())?;
        let api_base = std::env::var("WISPY_GITHUB_API").unwrap_or_else(|_| DEFAULT_API_BASE.to_string());
        let github = GitHubClient::new(api_base, token.clone()).map_err(|e| e.to_string())?;
        let cache = Cache::open(&self.data_dir.join("cache.sqlite")).map_err(|e| e.to_string())?;
        let store = RepoStore::new(self.data_dir.join("repos"), Some(token));
        Ok(PrService::new(github, store, cache, Highlighter::new()))
    }

    pub fn register_stack(&self, snapshot: Arc<StackSnapshot>) -> String {
        let id = stack_id(&snapshot);
        lock(&self.stacks).insert(id.clone(), snapshot);
        *lock(&self.current_stack) = Some(id.clone());
        id
    }

    pub fn stack(&self, id: &str) -> Option<Arc<StackSnapshot>> {
        lock(&self.stacks).get(id).cloned()
    }

    pub fn is_current_stack(&self, id: &str) -> bool {
        lock(&self.current_stack).as_deref() == Some(id)
    }

    /// Claims the precompute worker slot for a stack; false if one is already running.
    pub fn start_precompute(&self, id: &str) -> bool {
        lock(&self.precomputing).insert(id.to_string())
    }

    pub fn finish_precompute(&self, id: &str) {
        lock(&self.precomputing).remove(id);
    }

    pub fn register_view(&self, id: &str, stack_id: &str, top: usize, view: Arc<DiffView>) {
        let mut views = lock(&self.views);
        views.retain(|open| open.id != id);
        views.push_front(OpenView { id: id.to_string(), stack_id: stack_id.to_string(), top: top as u8, view });
        views.truncate(MAX_OPEN_VIEWS);
    }

    pub fn view(&self, id: &str) -> Option<Arc<DiffView>> {
        lock(&self.views).iter().find(|open| open.id == id).map(|open| open.view.clone())
    }

    /// The view and the stack it belongs to.
    pub fn view_with_stack(&self, id: &str) -> Option<(Arc<DiffView>, Arc<StackSnapshot>)> {
        let (view, stack_id) = lock(&self.views).iter().find(|open| open.id == id).map(|o| (o.view.clone(), o.stack_id.clone()))?;
        Some((view, self.stack(&stack_id)?))
    }

    /// The view and its range's top PR.
    pub fn view_with_top(&self, id: &str) -> Option<(Arc<DiffView>, u8)> {
        lock(&self.views).iter().find(|open| open.id == id).map(|open| (open.view.clone(), open.top))
    }

    pub fn split_file(&self, key: &str) -> Option<Arc<Vec<SplitRow>>> {
        lock(&self.split_files).iter().find(|(k, _)| k == key).map(|(_, rows)| rows.clone())
    }

    pub fn remember_split_file(&self, key: &str, rows: Arc<Vec<SplitRow>>) {
        let mut files = lock(&self.split_files);
        files.retain(|(k, _)| k != key);
        files.push_front((key.to_string(), rows));
        files.truncate(MAX_SPLIT_FILES);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}
