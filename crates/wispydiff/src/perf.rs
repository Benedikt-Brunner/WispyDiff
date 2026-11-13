//! `--perf owner/repo#N`: drives the real window through the SPEC's performance scenarios and
//! prints JSON (`just bench` runs it against the fixture and checks the budgets).
//!
//! Latencies run from the `*:start` mark to the `*:first-visible` mark, set once the frame showing
//! the result has been presented. Scrolling sends one real wheel event per frame; the intervals
//! between frames are the frame times.
//!
//! Measure with the window visible (or on X11): Wayland compositors and hidden Xwayland windows
//! slow a window's frames down, which isn't the app.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui_kit::{point, px, App, AsyncWindowContext, Entity, InputEvent, Modifiers, Pixels, ScrollDelta, ScrollWheelEvent, TouchPhase, Window};
use serde_json::{json, Value};

use crate::app::WispyApp;
use crate::diff::layout::Mode;
use crate::diff::DiffViewer;
use crate::perf_marks::{between, count};
use crate::session::BaseMode;

const OPEN_SAMPLES: usize = 5;
const TOGGLE_SAMPLES: usize = 6;
const USAGE_SAMPLES: usize = 5;
const SCROLL_FRAMES: usize = 240;

pub fn start(app: Entity<WispyApp>, pr: String, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let report = match run(&app, &pr, cx).await {
                Ok(report) => report,
                Err(err) => json!({ "error": err }),
            };
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            // Every scenario has a "pass"; a missed budget (or an error) fails the run.
            let passed = report.get("error").is_none()
                && report.as_object().is_some_and(|o| o.values().all(|v| v.get("pass").is_none_or(|p| p.as_bool() == Some(true))));
            if !passed {
                eprintln!("performance budgets missed");
            }
            std::process::exit(if passed { 0 } else { 1 });
        })
        .detach();
}

fn e(err: impl ToString) -> String {
    err.to_string()
}

async fn run(app: &Entity<WispyApp>, pr: &str, cx: &mut AsyncWindowContext) -> Result<Value, String> {
    let session = cx.update(|_, cx| app.read(cx).session.clone()).map_err(e)?;
    let quick = std::env::var_os("WISPY_PERF_QUICK").is_some();

    // First open: discovers and fetches the stack (not a budget, but worth knowing).
    let first = open(app, pr, cx).await?;
    let size = cx.update(|_, cx| session.read(cx).stack.as_ref().map_or(1, |s| s.prs.len())).map_err(e)?;
    // Every range precomputed (as after prefetching), so switching is a cache read.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let all = cx
            .update(|_, cx| {
                let s = session.read(cx);
                (0..size).all(|lo| (lo..size).all(|hi| s.ready(lo, hi)))
            })
            .map_err(e)?;
        if all {
            break;
        }
        if Instant::now() > deadline {
            return Err("timed out waiting for the ranges to be precomputed".into());
        }
        tick(cx).await;
    }

    // Cached opens (from the inbox, as a user would).
    let mut cached = Vec::new();
    for _ in 0..if quick { 1 } else { OPEN_SAMPLES } {
        cx.update(|_, cx| session.update(cx, |s, cx| s.go_home(cx))).map_err(e)?;
        settle(cx).await;
        cached.push(open(app, pr, cx).await?);
    }

    // Range switches (every range is precomputed by now).
    let mut range_switches = Vec::new();
    for lo in 0..if quick { 0 } else { size } {
        for hi in lo..size {
            let before = count("view:first-visible");
            cx.update(|_, cx| session.update(cx, |s, cx| s.choose_range(lo, hi, None, cx))).map_err(e)?;
            range_switches.push(wait_mark(cx, "range:start", "view:first-visible", before).await?);
        }
    }

    // The whole stack: toggling every file between unified and side by side.
    let before = count("view:first-visible");
    cx.update(|_, cx| session.update(cx, |s, cx| s.choose_range(0, size - 1, None, cx))).map_err(e)?;
    let _ = wait_mark(cx, "range:start", "view:first-visible", before).await;
    let viewer = diff(app, cx)?;
    let mut toggles = Vec::new();
    for _ in 0..if quick { 1 } else { TOGGLE_SAMPLES } {
        let before = count("layout:first-visible");
        cx.update(|_, cx| viewer.update(cx, |v, cx| v.perf_toggle_all_modes(cx))).map_err(e)?;
        toggles.push(wait_mark(cx, "toggle:start", "layout:first-visible", before).await?);
    }
    // Back to unified for what follows.
    cx.update(|_, cx| session.update(cx, |s, cx| s.set_default_mode(BaseMode::Unified, cx))).map_err(e)?;
    settle(cx).await;

    // Usages of a name in the touched files, and the first hit of a whole-repo search.
    let mut usages = Vec::new();
    for _ in 0..if quick { 1 } else { USAGE_SAMPLES } {
        let before = count("usages:result");
        cx.update(|window, cx| viewer.update(cx, |v, cx| v.open_usages("Helper", window, cx))).map_err(e)?;
        usages.push(wait_mark(cx, "usages:start", "usages:result", before).await?);
    }
    let before = count("grep:first-hit");
    cx.update(|window, cx| {
        viewer.update(cx, |v, cx| {
            v.open_search("compute".into(), window, cx);
            if let Some(panel) = v.code_panel().cloned() {
                panel.update(cx, |p, cx| p.search("compute".into(), cx));
            }
        })
    })
    .map_err(e)?;
    let grep = wait_mark(cx, "grep:start", "grep:first-hit", before).await?;

    // Scrolling: the whole diff (a flick), then the largest file in both modes.
    let flick = scroll(&viewer, cx, px(240.), None).await?;
    let largest = cx.update(|_, cx| viewer.read(cx).largest_file()).map_err(e)?;
    let unified = scroll(&viewer, cx, px(60.), Some((largest, Mode::Unified))).await?;
    let split = scroll(&viewer, cx, px(60.), Some((largest, Mode::Split))).await?;

    let latency = |samples: &[Duration], budget: f64| {
        let ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        let median = median(&ms);
        let worst = ms.iter().cloned().fold(0.0, f64::max);
        json!({ "samples_ms": ms, "median_ms": median, "worst_ms": worst, "budget_ms": budget,
                "pass": median < budget && worst < 2.0 * budget })
    };
    let grep_ms = grep.as_secs_f64() * 1000.0;
    Ok(json!({
        "first_open_ms": first.as_secs_f64() * 1000.0,
        "cached_open": latency(&cached, 150.0),
        "range_switch": latency(&range_switches, 100.0),
        "toggle_all_modes": latency(&toggles, 100.0),
        "usages": latency(&usages, 50.0),
        "grep_first_hit": json!({ "ms": grep_ms, "budget_ms": 300.0, "pass": grep_ms < 300.0 }),
        "scroll_flick": flick,
        "scroll_largest_unified": unified,
        "scroll_largest_split": split,
    }))
}

fn diff(app: &Entity<WispyApp>, cx: &mut AsyncWindowContext) -> Result<Entity<DiffViewer>, String> {
    cx.update(|_, cx| app.read(cx).diff.clone()).map_err(e)?.ok_or_else(|| "no diff open".to_string())
}

async fn open(app: &Entity<WispyApp>, pr: &str, cx: &mut AsyncWindowContext) -> Result<Duration, String> {
    let session = cx.update(|_, cx| app.read(cx).session.clone()).map_err(e)?;
    let before = count("view:first-visible");
    let pr = pr.to_string();
    cx.update(|_, cx| session.update(cx, |s, cx| s.open(pr, cx))).map_err(e)?;
    wait_mark(cx, "open:start", "view:first-visible", before).await
}

/// Waits for the `before + 1`th `to` mark; the time from the latest `from` mark to it.
async fn wait_mark(cx: &mut AsyncWindowContext, from: &str, to: &str, before: usize) -> Result<Duration, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while count(to) <= before {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for `{to}`"));
        }
        tick(cx).await;
    }
    let d = between(from, to).ok_or_else(|| format!("no `{from}` before `{to}`"))?;
    settle(cx).await;
    Ok(d)
}

/// Scrolls `step` per frame for `SCROLL_FRAMES` frames, from the top of `file` (shown in `mode`)
/// or of the whole diff, and reports the frame intervals and blank rows.
async fn scroll(viewer: &Entity<DiffViewer>, cx: &mut AsyncWindowContext, step: Pixels, file: Option<(usize, Mode)>) -> Result<Value, String> {
    cx.update(|_, cx| viewer.update(cx, |v, cx| v.perf_prepare_scroll(file, cx))).map_err(e)?;
    if let Some((file, Mode::Split)) = file {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !cx.update(|_, cx| viewer.update(cx, |v, cx| v.split_rows(file, cx).is_some())).map_err(e)? {
            if Instant::now() > deadline {
                return Err("timed out loading side-by-side rows".into());
            }
            tick(cx).await;
        }
    }
    settle(cx).await;
    let blank_before = crate::perf_marks::placeholders();
    let (tx, rx) = oneshot::channel();
    cx.update(move |window, _| {
        let state = Rc::new(RefCell::new(FrameLoop { frames: Vec::with_capacity(SCROLL_FRAMES + 1), done: Some(tx) }));
        frame(window, state, step);
    })
    .map_err(e)?;
    let frames = rx.await.map_err(|_| "scroll aborted".to_string())?;
    let blank = crate::perf_marks::placeholders() - blank_before;
    let intervals: Vec<f64> = frames.windows(2).map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0).collect();
    let mut sorted = intervals.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = sorted.get((sorted.len() as f64 * 0.95) as usize).copied().unwrap_or(0.0);
    let slow = intervals.iter().filter(|&&ms| ms > 33.0).count() as f64 / intervals.len().max(1) as f64;
    Ok(json!({
        "frames": intervals.len(), "median_ms": median(&intervals), "p95_ms": p95, "max_ms": sorted.last(),
        "over_33ms": slow, "blank_rows_rendered": blank,
        "pass": p95 <= 20.0 && slow <= 0.02 && blank == 0,
    }))
}

struct FrameLoop {
    frames: Vec<Instant>,
    done: Option<oneshot::Sender<Vec<Instant>>>,
}

fn frame(window: &mut Window, state: Rc<RefCell<FrameLoop>>, step: Pixels) {
    window.on_next_frame(move |window, cx| {
        let count = {
            let mut s = state.borrow_mut();
            s.frames.push(Instant::now());
            s.frames.len()
        };
        if count > SCROLL_FRAMES {
            let mut s = state.borrow_mut();
            let frames = std::mem::take(&mut s.frames);
            if let Some(done) = s.done.take() {
                let _ = done.send(frames);
            }
            return;
        }
        // A real wheel event over the rows, through the list's own scroll handling.
        let size = window.viewport_size();
        window.dispatch_event(
            ScrollWheelEvent {
                position: point(size.width * 0.7, size.height * 0.5),
                delta: ScrollDelta::Pixels(point(px(0.), -step)),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
        frame(window, state, step);
    });
}

async fn tick(cx: &mut AsyncWindowContext) {
    cx.background_executor().timer(Duration::from_millis(5)).await;
}

/// Lets a few frames pass so one measurement doesn't run into the next.
async fn settle(cx: &mut AsyncWindowContext) {
    cx.background_executor().timer(Duration::from_millis(250)).await;
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
}
