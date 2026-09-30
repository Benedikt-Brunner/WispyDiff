use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use wispy_core::github::PullRequest;
use wispy_core::model::{DiffSummary, DiffView, Row};
use wispy_core::pr_ref::PrRef;
use wispy_core::service::PrService;
use wispy_core::anchors::{locate, Anchor, Location};
use wispy_core::drafts::Draft;
use wispy_core::github::{ReviewThread, Side, Verdict};
use wispy_core::progress::Checkpoint;
use wispy_core::review::{Outcome, Planned};
use wispy_core::service::{NewDraft, ShownDraft};
use wispy_core::split::SplitRow;
use wispy_core::stack::StackSnapshot;

use crate::state::{view_id, AppState};

#[derive(Serialize, Clone, Copy)]
pub struct Range {
    pub lo: usize,
    pub hi: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedStack {
    pub stack_id: String,
    pub ignore_whitespace: bool,
    /// Bottom to top.
    pub prs: Vec<PullRequest>,
    pub needs_rebase: Vec<bool>,
    pub focus: usize,
    pub range: Range,
    pub view_id: String,
    pub summary: DiffSummary,
    pub from_cache: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedRange {
    pub view_id: String,
    pub range: Range,
    pub ignore_whitespace: bool,
    pub summary: DiffSummary,
    /// Set when this is "changes since checkpoint" rather than the range's full diff.
    pub since: Option<Since>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Since {
    pub checkpoint_id: String,
    pub created_at: i64,
    /// Replaying the old version onto the new base conflicted: a raw diff is shown.
    pub conflicts: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RangeReady {
    stack_id: String,
    lo: usize,
    hi: usize,
}

/// Opens the stack containing a PR (by URL or `owner/repo#N`), showing that PR alone.
/// Returns the cached stack immediately when there is one (call [`refresh_pr`] afterwards).
#[tauri::command]
pub async fn open_pr(input: String, app: AppHandle, state: State<'_, AppState>) -> Result<OpenedStack, String> {
    let pr = PrRef::parse(&input).map_err(|e| e.to_string())?;
    let service = state.service()?;

    let cached = blocking({
        let (service, pr) = (service.clone(), pr.clone());
        move || {
            let Some(snapshot) = service.cached_stack(&pr)? else { return Ok(None) };
            let focus = snapshot.stack.focus;
            Ok(service.cached_range(&snapshot, focus, focus)?.map(|view| (snapshot, view)))
        }
    })
    .await?;
    if let Some((snapshot, view)) = cached {
        return Ok(open_stack(&app, &state, Arc::new(snapshot), Arc::new(view), true));
    }

    let (snapshot, view) = discover_and_load(&service, &pr).await?;
    Ok(open_stack(&app, &state, snapshot, view, false))
}

/// Re-discovers the stack; returns it only if anything (structure or heads) changed.
#[tauri::command]
pub async fn refresh_pr(
    input: String,
    known_stack_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<OpenedStack>, String> {
    let pr = PrRef::parse(&input).map_err(|e| e.to_string())?;
    let service = state.service()?;
    let stack = service.discover(&pr).await.map_err(|e| e.to_string())?;
    let snapshot = blocking({
        let service = service.clone();
        move || service.fetch(stack)
    })
    .await?;
    if crate::state::stack_id(&snapshot) == known_stack_id {
        return Ok(None);
    }
    let (snapshot, view) = load_focus(&service, Arc::new(snapshot)).await?;
    Ok(Some(open_stack(&app, &state, snapshot, view, false)))
}

/// Switches the shown range to stack PRs `lo..=hi` (optionally hiding whitespace-only changes).
#[tauri::command]
pub async fn select_range(
    stack_id: String,
    lo: usize,
    hi: usize,
    ignore_whitespace: bool,
    state: State<'_, AppState>,
) -> Result<OpenedRange, String> {
    let snapshot = state.stack(&stack_id).ok_or_else(|| format!("stack {stack_id} is no longer open"))?;
    let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
    let id = view_id(&stack_id, lo, hi, ignore_whitespace);
    let view = match state.view(&id) {
        Some(view) => view,
        None => {
            let service = state.service()?;
            let view = blocking(move || service.range_with(&snapshot, lo, hi, ignore_whitespace)).await?;
            state.register_view(&id, &stack_id, hi, view.clone());
            view
        }
    };
    Ok(OpenedRange { view_id: id, range: Range { lo, hi }, ignore_whitespace, summary: view.summary.clone(), since: None })
}

/// Shows only what changed in `lo..=hi` since a checkpoint (rebase-aware).
#[tauri::command]
pub async fn select_since(
    stack_id: String,
    lo: usize,
    hi: usize,
    checkpoint_id: String,
    state: State<'_, AppState>,
) -> Result<OpenedRange, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    let (lo, hi) = (lo.min(hi), hi.min(snapshot.len() - 1));
    let (view, checkpoint, conflicts) = blocking({
        let (service, snapshot) = (service.clone(), snapshot.clone());
        move || {
            let checkpoint = service
                .checkpoints(&snapshot, lo, hi)?
                .into_iter()
                .find(|c| c.id == checkpoint_id)
                .ok_or_else(|| wispy_core::Error::GitHub("checkpoint not found for this range".into()))?;
            let (view, conflicts) = service.interdiff(&snapshot, lo, hi, &checkpoint)?;
            Ok((view, checkpoint, conflicts))
        }
    })
    .await?;
    let id = format!("{}:since:{}", view_id(&stack_id, lo, hi, false), checkpoint.id);
    state.register_view(&id, &stack_id, hi, view.clone());
    Ok(OpenedRange {
        view_id: id,
        range: Range { lo, hi },
        ignore_whitespace: false,
        summary: view.summary.clone(),
        since: Some(Since { checkpoint_id: checkpoint.id, created_at: checkpoint.created_at, conflicts }),
    })
}

#[tauri::command]
pub async fn list_checkpoints(stack_id: String, lo: usize, hi: usize, state: State<'_, AppState>) -> Result<Vec<Checkpoint>, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    blocking(move || service.checkpoints(&snapshot, lo, hi.min(snapshot.len() - 1))).await
}

/// Records the current heads of `lo..=hi` as reviewed (local only).
#[tauri::command]
pub async fn mark_reviewed(stack_id: String, lo: usize, hi: usize, state: State<'_, AppState>) -> Result<Checkpoint, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    blocking(move || service.mark_reviewed(&snapshot, lo, hi, "manual")).await
}

#[tauri::command]
pub async fn get_viewed(repo: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let service = state.service()?;
    blocking(move || service.viewed(&repo)).await
}

#[tauri::command]
pub async fn set_viewed(repo: String, key: String, viewed: bool, state: State<'_, AppState>) -> Result<(), String> {
    let service = state.service()?;
    blocking(move || service.set_viewed(&repo, &key, viewed)).await
}

/// Unified rows `start..end` of file `file` (offsets within the file; 0 is its header).
#[tauri::command]
pub fn get_rows(view_id: String, file: usize, start: u32, end: u32, state: State<'_, AppState>) -> Result<Vec<Row>, String> {
    let view = state.view(&view_id).ok_or_else(|| format!("diff {view_id} is no longer open"))?;
    let summary = view.summary.files.get(file).ok_or("no such file")?;
    let end = end.min(summary.row_count);
    Ok(view.rows(summary.first_row + start.min(end), summary.first_row + end).to_vec())
}

/// Side-by-side rows `start..end` of file `file` (offsets within the file; 0 is its header,
/// which the frontend draws itself, so the first row returned for `start = 0` is the first pair).
#[tauri::command]
pub async fn get_split_rows(
    view_id: String,
    file: usize,
    start: u32,
    end: u32,
    state: State<'_, AppState>,
) -> Result<Vec<SplitRow>, String> {
    let key = format!("{view_id}#{file}");
    let rows = match state.split_file(&key) {
        Some(rows) => rows,
        None => {
            let (view, snapshot) = state.view_with_stack(&view_id).ok_or_else(|| format!("diff {view_id} is no longer open"))?;
            let service = state.service()?;
            let rows = Arc::new(blocking(move || service.split_rows(&snapshot, &view, file)).await?);
            state.remember_split_file(&key, rows.clone());
            rows
        }
    };
    let (start, end) = (start.max(1) as usize - 1, (end as usize).saturating_sub(1).min(rows.len()));
    Ok(rows[start.min(end)..end].to_vec())
}

#[tauri::command]
pub async fn get_ignore_patterns(repo: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let service = state.service()?;
    blocking(move || service.ignore_patterns(&repo)).await
}

#[tauri::command]
pub async fn set_ignore_patterns(repo: String, patterns: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let service = state.service()?;
    blocking(move || service.set_ignore_patterns(&repo, &patterns)).await
}

// ---------- comments ----------

fn snapshot_of(state: &AppState, stack_id: &str) -> Result<Arc<StackSnapshot>, String> {
    state.stack(stack_id).ok_or_else(|| format!("stack {stack_id} is no longer open"))
}

/// Pending drafts of the stack, anchored to each PR's current head.
#[tauri::command]
pub async fn list_drafts(stack_id: String, state: State<'_, AppState>) -> Result<Vec<ShownDraft>, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    blocking(move || service.shown_drafts(&snapshot)).await
}

#[tauri::command]
pub async fn create_draft(stack_id: String, draft: NewDraft, state: State<'_, AppState>) -> Result<Draft, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    blocking(move || service.create_draft(&snapshot, draft)).await
}

#[tauri::command]
pub async fn update_draft(id: String, body: Option<String>, as_file: Option<bool>, state: State<'_, AppState>) -> Result<Option<Draft>, String> {
    let service = state.service()?;
    blocking(move || service.update_draft(&id, body, as_file)).await
}

#[tauri::command]
pub async fn delete_draft(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let service = state.service()?;
    blocking(move || service.delete_draft(&id)).await
}

/// Whether a line comment on these lines would be accepted as such (else: file comment).
#[tauri::command]
pub async fn accepts_line_comment(
    stack_id: String,
    pr_index: usize,
    path: String,
    side: Side,
    start: u32,
    end: u32,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    blocking(move || service.accepts_line_comment(&snapshot, pr_index, &path, side, start, end)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrThreads {
    pub pr_index: usize,
    pub threads: Vec<ReviewThread>,
}

/// Review threads of every stack PR from the cache; with `refresh`, fetched from GitHub first.
#[tauri::command]
pub async fn list_threads(stack_id: String, refresh: bool, state: State<'_, AppState>) -> Result<Vec<PrThreads>, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    if refresh {
        service.refresh_threads(&snapshot).await.map_err(|e| e.to_string())?;
    }
    blocking(move || {
        (0..snapshot.len())
            .map(|pr_index| Ok(PrThreads { pr_index, threads: service.cached_threads(&snapshot, pr_index)? }))
            .collect()
    })
    .await
}

/// Where each anchor shows up in a view (see [`locate`]).
#[tauri::command]
pub fn locate_anchors(view_id: String, anchors: Vec<Anchor>, state: State<'_, AppState>) -> Result<Vec<Option<Location>>, String> {
    let (view, top) = state.view_with_top(&view_id).ok_or_else(|| format!("diff {view_id} is no longer open"))?;
    Ok(anchors.iter().map(|anchor| locate(&view, top, anchor)).collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPlan {
    /// The stack as it is on GitHub right now (drafts are planned against it).
    pub stack: OpenedStack,
    pub prs: Vec<PrPlan>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrPlan {
    pub pr_index: usize,
    pub planned: Vec<Planned>,
}

/// Re-fetches the stack, settles interrupted submits, and plans every PR's pending drafts.
#[tauri::command]
pub async fn prepare_submit(stack_id: String, app: AppHandle, state: State<'_, AppState>) -> Result<SubmitPlan, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    let pr = snapshot.pr_ref(snapshot.stack.focus);
    let stack = service.discover(&pr).await.map_err(|e| e.to_string())?;
    let fresh = Arc::new(blocking({
        let service = service.clone();
        move || service.fetch(stack)
    })
    .await?);
    for index in 0..fresh.len() {
        service.reconcile(&fresh, index).await.map_err(|e| e.to_string())?;
    }
    let (plans, fresh) = blocking({
        let (service, fresh) = (service.clone(), fresh.clone());
        move || {
            let plans = (0..fresh.len())
                .map(|pr_index| Ok(PrPlan { pr_index, planned: service.plan(&fresh, pr_index)? }))
                .collect::<wispy_core::Result<Vec<_>>>()?;
            Ok((plans, fresh))
        }
    })
    .await?;
    let (fresh, view) = load_focus(&service, fresh).await?;
    let opened = open_stack(&app, &state, fresh, view, false);
    Ok(SubmitPlan { stack: opened, prs: plans.into_iter().filter(|p| !p.planned.is_empty()).collect() })
}

/// Posts one review for a stack PR (after [`prepare_submit`]).
#[tauri::command]
pub async fn submit_review(
    stack_id: String,
    pr_index: usize,
    verdict: Verdict,
    summary: Option<String>,
    state: State<'_, AppState>,
) -> Result<Outcome, String> {
    let (snapshot, service) = (snapshot_of(&state, &stack_id)?, state.service()?);
    let outcome = service.submit(&snapshot, pr_index, verdict, summary).await.map_err(|e| e.to_string())?;
    if outcome.failed.is_empty() {
        // Submitting counts as having reviewed this PR at its current head.
        let _ = service.mark_reviewed(&snapshot, pr_index, pr_index, "submit");
    }
    Ok(outcome)
}

// ---------- inbox ----------

/// The last known inbox (instant, offline-friendly); fresh entries arrive as `inbox-updated`.
#[tauri::command]
pub async fn get_inbox(state: State<'_, AppState>) -> Result<Vec<wispy_core::service::InboxEntry>, String> {
    let service = state.service()?;
    blocking(move || {
        let groups = service.cached_inbox()?;
        service.inbox_entries(groups)
    })
    .await
}

/// Asks the background worker to refresh the inbox and prefetch now.
#[tauri::command]
pub fn refresh_inbox(app: AppHandle) {
    if let Some(trigger) = app.try_state::<crate::prefetch::PrefetchTrigger>() {
        trigger.fire();
    }
}

async fn discover_and_load(service: &Arc<PrService>, pr: &PrRef) -> Result<(Arc<StackSnapshot>, Arc<DiffView>), String> {
    let stack = service.discover(pr).await.map_err(|e| e.to_string())?;
    let snapshot = blocking({
        let service = service.clone();
        move || service.fetch(stack)
    })
    .await?;
    load_focus(service, Arc::new(snapshot)).await
}

async fn load_focus(service: &Arc<PrService>, snapshot: Arc<StackSnapshot>) -> Result<(Arc<StackSnapshot>, Arc<DiffView>), String> {
    let service = service.clone();
    let for_view = snapshot.clone();
    let view = blocking(move || {
        let focus = for_view.stack.focus;
        service.range(&for_view, focus, focus)
    })
    .await?;
    Ok((snapshot, view))
}

fn open_stack(app: &AppHandle, state: &AppState, snapshot: Arc<StackSnapshot>, view: Arc<DiffView>, from_cache: bool) -> OpenedStack {
    let stack_id = state.register_stack(snapshot.clone());
    let focus = snapshot.stack.focus;
    let id = view_id(&stack_id, focus, focus, false);
    state.register_view(&id, &stack_id, focus, view.clone());
    spawn_precompute(app.clone(), stack_id.clone(), snapshot.clone());
    OpenedStack {
        stack_id,
        ignore_whitespace: false,
        prs: snapshot.stack.prs.clone(),
        needs_rebase: snapshot.needs_rebase(),
        focus,
        range: Range { lo: focus, hi: focus },
        view_id: id,
        summary: view.summary.clone(),
        from_cache,
    }
}

/// Computes every range of the stack into the cache (most useful first) so switching ranges
/// is a cache read. Stops as soon as another stack is opened.
fn spawn_precompute(app: AppHandle, stack_id: String, snapshot: Arc<StackSnapshot>) {
    if !app.state::<AppState>().start_precompute(&stack_id) {
        return;
    }
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        if let Ok(service) = state.service() {
            for (lo, hi) in snapshot.ranges_by_priority() {
                if !state.is_current_stack(&stack_id) {
                    break;
                }
                match service.precompute(&snapshot, lo, hi) {
                    Ok(_) => {
                        let _ = app.emit("range-ready", RangeReady { stack_id: stack_id.clone(), lo, hi });
                    }
                    Err(err) => eprintln!("precompute {stack_id} {lo}-{hi}: {err}"),
                }
            }
        }
        state.finish_precompute(&stack_id);
    });
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> wispy_core::Result<T> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}
