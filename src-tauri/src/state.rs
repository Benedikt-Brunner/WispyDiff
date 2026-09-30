use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::Manager;
use wispy_core::cache::Cache;
use wispy_core::github::{GitHubClient, DEFAULT_API_BASE};
use wispy_core::highlight::Highlighter;
use wispy_core::model::DiffView;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;
use wispy_core::token::resolve_github_token;

/// `WISPY_DATA_DIR` overrides the location (tests); default `~/Library/Application Support/WispyDiff`.
pub fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(dir) = std::env::var("WISPY_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(app.path().data_dir()?.join("WispyDiff"))
}

pub struct AppState {
    data_dir: PathBuf,
    service: OnceLock<Result<Arc<PrService>, String>>,
    open_views: Mutex<HashMap<String, Arc<DiffView>>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        AppState { data_dir, service: OnceLock::new(), open_views: Mutex::new(HashMap::new()) }
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

    pub fn register_view(&self, id: &str, view: Arc<DiffView>) {
        let mut views = self.open_views.lock().unwrap_or_else(|p| p.into_inner());
        // Keep memory bounded: only a handful of diffs are ever open at once.
        if views.len() >= 8 && !views.contains_key(id) {
            views.clear();
        }
        views.insert(id.to_string(), view);
    }

    pub fn view(&self, id: &str) -> Option<Arc<DiffView>> {
        self.open_views.lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
    }
}
