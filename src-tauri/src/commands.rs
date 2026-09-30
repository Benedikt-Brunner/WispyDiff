use serde::Serialize;
use tauri::State;
use wispy_core::github::PullRequest;
use wispy_core::model::{DiffSummary, Row};
use wispy_core::pr_ref::PrRef;
use wispy_core::service::LoadedPr;

use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedPr {
    /// Handle for [`get_rows`].
    pub view_id: String,
    pub pr: PrRef,
    pub pull_request: PullRequest,
    pub summary: DiffSummary,
    pub from_cache: bool,
}

fn register(state: &AppState, loaded: LoadedPr, from_cache: bool) -> OpenedPr {
    let view_id = format!("{}@{}..{}", loaded.pr, loaded.view.summary.base_sha, loaded.view.summary.head_sha);
    let summary = loaded.view.summary.clone();
    state.register_view(&view_id, loaded.view);
    OpenedPr { view_id, pr: loaded.pr, pull_request: loaded.pull_request, summary, from_cache }
}

/// Opens a PR by URL or `owner/repo#N`. Returns the cached diff immediately when there is
/// one (call [`refresh_pr`] afterwards); otherwise fetches and computes it.
#[tauri::command]
pub async fn open_pr(input: String, state: State<'_, AppState>) -> Result<OpenedPr, String> {
    let pr = PrRef::parse(&input).map_err(|e| e.to_string())?;
    let service = state.service()?;
    let cached = {
        let (service, pr) = (service.clone(), pr.clone());
        tauri::async_runtime::spawn_blocking(move || service.cached(&pr)).await.map_err(|e| e.to_string())?
    }
    .map_err(|e| e.to_string())?;
    if let Some(loaded) = cached {
        return Ok(register(&state, loaded, true));
    }
    let loaded = fetch_and_load(&service, pr).await?;
    Ok(register(&state, loaded, false))
}

/// Re-fetches PR metadata; returns a new view only if the PR's head moved.
#[tauri::command]
pub async fn refresh_pr(input: String, known_head_sha: String, state: State<'_, AppState>) -> Result<Option<OpenedPr>, String> {
    let pr = PrRef::parse(&input).map_err(|e| e.to_string())?;
    let service = state.service()?;
    let pull_request = service.fetch_pull_request(&pr).await.map_err(|e| e.to_string())?;
    if pull_request.head_sha == known_head_sha {
        return Ok(None);
    }
    let loaded = load_blocking(&service, pr, pull_request).await?;
    Ok(Some(register(&state, loaded, false)))
}

#[tauri::command]
pub fn get_rows(view_id: String, start: u32, end: u32, state: State<'_, AppState>) -> Result<Vec<Row>, String> {
    let view = state.view(&view_id).ok_or_else(|| format!("diff {view_id} is no longer open"))?;
    Ok(view.rows(start, end).to_vec())
}

async fn fetch_and_load(service: &std::sync::Arc<wispy_core::service::PrService>, pr: PrRef) -> Result<LoadedPr, String> {
    let pull_request = service.fetch_pull_request(&pr).await.map_err(|e| e.to_string())?;
    load_blocking(service, pr, pull_request).await
}

async fn load_blocking(
    service: &std::sync::Arc<wispy_core::service::PrService>,
    pr: PrRef,
    pull_request: PullRequest,
) -> Result<LoadedPr, String> {
    let service = service.clone();
    tauri::async_runtime::spawn_blocking(move || service.load(&pr, pull_request))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
