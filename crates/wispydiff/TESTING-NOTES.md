# GPUI testing research

Research only, 2026-11-13. Recommendation: retain the existing real-git/HTTP core tests; replace most WebdriverIO functional coverage with in-process, headless GPUI view integration tests; add a smaller real-IO view suite; keep binding performance budgets in a separately launched **release application with a real platform window**. A deterministic GPUI test is not a frame-rate benchmark.

## Source scope and reproducibility

The version matters. This report inspected the **published gpui-kit 0.6.6** sources, not an assumed API from Zed main. Kit pins `gpui-pre = "=0.3.6"`, `gpui-pre-platform = "=0.3.6"`, and uses gpui-base/component 0.6.6. The published GPUI manifest identifies Zed revision `bcf6582ce3500df93a8a39366640173e6786cea6`; the Kit/Base/Component archives identify Longbridge revision `9765ae2c9a5eccfa13891248a445991e6f6a09d8`. Treat published sources as authoritative for this snapshot: a package can contain adaptations beyond its recorded upstream revision. Pin Cargo.lock; do not assume all 0.6.x releases have identical helpers.

Downloaded source roots (outside this repository):

| Abbreviation below | Actual directory |
|---|---|
| G | `/tmp/wispydiff-research/gpui-pre-0.3.6` |
| K | `/tmp/wispydiff-research/gpui-kit-0.6.6` |
| B | `/tmp/wispydiff-research/gpui-base-0.6.6` |
| C | `/tmp/wispydiff-research/gpui-component-0.6.6` |
| S | `/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6` |
| P | `/tmp/wispydiff-research/gpui-pre-platform-0.3.6` |
| L | `/tmp/wispydiff-research/gpui-pre-linux-0.3.6` |
| W | `/tmp/wispydiff-research/gpui-pre-wgpu-0.3.6` |
| M | `/tmp/wispydiff-research/gpui-pre-macros-0.3.6` |
| Z | `/tmp/wispydiff-research/zed`, upstream checkout `aff99ecc3249cfa95e720c4d6c1eb4a72105a3ee` |

Sources can be reacquired from `https://crates.io/api/v1/crates/<package>/<version>/download`; the GPUI package's library name is `gpui`. Permanent published-source entry points: [Kit](https://docs.rs/crate/gpui-kit/0.6.6/source/), [GPUI](https://docs.rs/crate/gpui-pre/0.3.6/source/), [Component](https://docs.rs/crate/gpui-component/0.6.6/source/), [Scheduler](https://docs.rs/crate/gpui-pre-scheduler/0.3.6/source/). The links below point directly into the inspected local sources with line numbers.

Project evidence read: CLAUDE.md, SPEC.md, Justfile, e2e/wdio.conf.ts, helpers.ts, perf.e2e.ts, and the functional suites for opening/ranges, keyboard/layout, inbox/offline, progress, comments, intelligence, and assistants. No application changes or benchmark runs were made for this report. The example below is source-checked, not compiled here; Linux compositor recipes are proposed smoke-test configurations, not claimed successful runs on this machine.

## 1. Test infrastructure and what it actually exercises

### Setup, contexts, entities

Use the facade's attribute and explicit imports:

```toml
[dependencies]
gpui-kit = "=0.6.6"

[dev-dependencies]
gpui-kit = { version = "=0.6.6", features = ["test-support"] }
```

`#[gpui_kit::test]` re-exports `#[gpui::test]`. Both synchronous and asynchronous test functions accept `cx: &mut TestAppContext`. The macro constructs seeded TestDispatcher contexts; async tests run through `ForegroundExecutor::block_test`. `iterations`, `seeds`, and `retries` are supported, with `SEED`/`ITERATIONS` environment overrides. This works with normal cargo test/nextest. Prefer multiple seeds for scheduling/state-machine coverage, not automatic retries to mask failure. Avoid `use gpui_kit::*` in tests: it also imports the `test` attribute and can shadow Rust's built-in attribute in macro expansion. See [K/tests/test_macro.rs:1](/tmp/wispydiff-research/gpui-kit-0.6.6/tests/test_macro.rs:1), [M/src/test.rs:95](/tmp/wispydiff-research/gpui-pre-macros-0.3.6/src/test.rs:95), [G/src/test.rs:95](/tmp/wispydiff-research/gpui-pre-0.3.6/src/test.rs:95).

Kit's feature forwards to GPUI, gpui-platform, Base, and Component test support. The dependency needs that feature: `cfg(test)` on WispyDiff alone does not expose testing APIs in dependencies. GPUI's feature includes leak detection, fake HTTP-client support, and test platform code. Normal builds should not enable it. [K/Cargo.toml:55](/tmp/wispydiff-research/gpui-kit-0.6.6/Cargo.toml:55), [G/Cargo.toml:69](/tmp/wispydiff-research/gpui-pre-0.3.6/Cargo.toml:69).

Relevant real signatures in [G/src/app/test_context.rs:260](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/test_context.rs:260):

```rust
TestAppContext::update<R>(&self, f: impl FnOnce(&mut App) -> R) -> R
TestAppContext::read<R>(&self, f: impl FnOnce(&App) -> R) -> R
TestAppContext::add_window<F,V>(&mut self, build_window: F) -> WindowHandle<V>
TestAppContext::open_window<F,V>(&mut self, window_size: Size<Pixels>,
                               build_window: F) -> WindowHandle<V>
// F: FnOnce(&mut Window, &mut Context<V>) -> V, V: Render + 'static
TestAppContext::add_window_view<F,V>(&mut self, build_root_view: F)
    -> (Entity<V>, &mut VisualTestContext)
VisualTestContext::from_window(window: AnyWindowHandle, cx: &TestAppContext) -> Self
VisualTestContext::update<R>(&mut self,
                            f: impl FnOnce(&mut Window, &mut App) -> R) -> R
```

`add_window`/`add_window_view` use the fake display's maximized bounds; choose `open_window(size(px(1200.), px(800.)), ...)` for reproducible geometry. The test builders return **V**, while production `App::open_window(WindowOptions, builder)` takes a builder returning **Entity<V>**. With components, initialize using `cx.update(gpui_kit::init)`, create the application entity inside the builder, and return `Root::new(view, window, cx)`. Root supplies component overlays, dialogs, notifications, etc. See the complete working upstream pattern [K/tests/ui.rs:60](/tmp/wispydiff-research/gpui-kit-0.6.6/tests/ui.rs:60).

Entity state is directly inspectable: `view.read_with(cx, |view, app| ...)`, `view.update(cx, |view, context| ...)`, `view.update_in(&mut visual_cx, |view, window, context| ...)`; inside `cx.update(|app| ...)`, `view.read(app)` is available. `TestAppContext` implements `AppContext::{new, read_entity, update_entity, update_window, read_window}`. Do not call `view.read(test_cx)`; use `read_with` or enter `update`. `cx.notifications(&view)`, `cx.events(&view)`, and `cx.condition(&view, predicate).await` support observable completion; `condition` has a 3-second executor-timer timeout. [G/examples/testing.rs:231](/tmp/wispydiff-research/gpui-pre-0.3.6/examples/testing.rs:231), [G/src/app/test_context.rs:631](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/test_context.rs:631).

### Keyboard, pointer, wheel, time

Raw VisualTestContext APIs in [G/src/app/test_context.rs:876](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/test_context.rs:876):

```rust
simulate_keystrokes(&mut self, keystrokes: &str) // "ctrl-k a enter", space-separated
simulate_input(&mut self, input: &str)         // literal text to current input target
simulate_mouse_move(&mut self, position: Point<Pixels>,
                    button: impl Into<Option<MouseButton>>, modifiers: Modifiers)
simulate_mouse_down(&mut self, position: Point<Pixels>,
                    button: MouseButton, modifiers: Modifiers)
simulate_mouse_up(&mut self, position: Point<Pixels>,
                  button: MouseButton, modifiers: Modifiers)
simulate_click(&mut self, position: Point<Pixels>, modifiers: Modifiers) // primary button
simulate_event<E: InputEvent>(&mut self, event: E)
dispatch_action<A: Action>(&mut self, action: A)
simulate_resize(&self, size: Size<Pixels>)
debug_bounds(&mut self, selector: &'static str) -> Option<Bounds<Pixels>>
```

Positions are window-local logical pixels, not screen/device coordinates. For double click, held-key repeat, key-up, middle/back/forward click, Shift-click or wheel, construct the corresponding event with explicit fields. Example wheel:

```rust
cx.simulate_event(gpui_kit::ScrollWheelEvent {
    position: gpui_kit::point(px(400.), px(300.)),
    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-120.))),
    ..Default::default()
});
```

`TestAppContext::{simulate_keystrokes,simulate_input,dispatch_action}` additionally take `AnyWindowHandle`; VisualTestContext stores that handle. Simulated keys dispatch through keymaps/focus/action handlers, and pointer events through GPUI's normal input dispatch/hit testing. Focus the real input/view first and draw the event tree before targeting it. Literal-input APIs test text delivery, not the OS IME. Clipboard, dialogs and platform commands are test doubles. Use the OS-specific shortcut (`cmd-k` on macOS, `ctrl-k` on Linux), matching SPEC.

`cx.run_until_parked()` drains runnable executor work; it is not a wait for arbitrary std threads/HTTP work, and it does not pump a real display vsync. Time control is `cx.executor().advance_clock(Duration::from_millis(...))`, followed by a drain/draw as needed. **Read implementation rather than wrapper comments in this version:** `TestAppContext::run_until_parked` uses TestDispatcher's tick loop; `BackgroundExecutor::run_until_parked` calls `TestScheduler::run`, which also drains runnable work without jumping to future timers. Its nearby comment claims automatic clock advancement, but that is not what `run()` does. `advance_clock` also drains runnable tasks while stepping through due timers, despite the executor wrapper's comment saying it does not run tasks. See [G/src/executor.rs:197](/tmp/wispydiff-research/gpui-pre-0.3.6/src/executor.rs:197), [G/src/platform/test/dispatcher.rs:76](/tmp/wispydiff-research/gpui-pre-0.3.6/src/platform/test/dispatcher.rs:76), [S/src/test_scheduler.rs:259](/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6/src/test_scheduler.rs:259), [S/src/test_scheduler.rs:450](/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6/src/test_scheduler.rs:450).

The macro's async `block_test` can advance to the next timer when parking is forbidden; this differs from explicitly draining with `run_until_parked`. GPUI fake timers do not change `std::time::Instant::now`, Tokio time, OS process time, or sleep. In this snapshot even `scheduler::Instant` is a re-export of `web_time::Instant`; fake **Clock** time is separate. Therefore advancing fake time is not enough to finish all motion geometry. Kit explicitly documents wall-time animation caveats. For deterministic layout tests use reduced motion; test timers/debounce separately. [S/src/test_scheduler.rs:93](/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6/src/test_scheduler.rs:93), [S/src/clock.rs:1](/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6/src/clock.rs:1), [B/src/motion.rs:1](/tmp/wispydiff-research/gpui-base-0.6.6/src/motion.rs:1), [K/TESTING.md:51](/tmp/wispydiff-research/gpui-kit-0.6.6/TESTING.md:51).

### Headless, layout, paint, selectors, visibility

**Yes, ordinary GPUI view tests can run on Linux CI with neither DISPLAY nor WAYLAND_DISPLAY.** TestAppContext constructs TestPlatform, which constructs TestWindow; there is no native X11/Wayland surface. This is a runtime statement, not a promise that compiling needs no Linux development packages. Default text is NoopTextSystem with synthetic font/glyph metrics; default atlas is HeadlessAtlas, and there is no renderer. [G/src/app/test_context.rs:129](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/test_context.rs:129), [G/src/platform/test/platform.rs:122](/tmp/wispydiff-research/gpui-pre-0.3.6/src/platform/test/platform.rs:122), [G/src/platform/test/window.rs:63](/tmp/wispydiff-research/gpui-pre-0.3.6/src/platform/test/window.rs:63), [G/src/platform.rs:1215](/tmp/wispydiff-research/gpui-pre-0.3.6/src/platform.rs:1215).

**Layout and paint traversal do run.** Test-mode effect flushing draws dirty windows; `Window::draw(&mut self, cx: &mut App) -> ArenaClearNeeded` runs rendering/layout/prepaint/paint and builds a Scene, then swaps rendered/next frames. Do not call `skip_drawing()` for UI tests. Explicit fresh frame: `window.refresh(); window.draw(cx).clear(cx);`. The clear token matters for the element arena. Ordinary TestWindow drawing discards the scene unless a headless renderer was supplied: no real GPU rasterization, compositing, monitor presentation, or pixel oracle. [G/src/app.rs:1817](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app.rs:1817), [G/src/window.rs:3263](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:3263), [G/src/platform/test/window.rs:527](/tmp/wispydiff-research/gpui-pre-0.3.6/src/platform/test/window.rs:527).

`InteractiveElement::debug_selector(self, f: impl FnOnce() -> String) -> Self` records one selector string in test-support builds; it is a no-op otherwise (including normal release, but **not** release with test-support enabled). During `Interactivity::paint`, it records the element's bounds in the next frame; `VisualTestContext::debug_bounds` reads the last completed frame's map. This is an exact string lookup, not CSS/accessibility-tree querying. Duplicate selector strings overwrite one map entry. The public convenience getter accepts a static string; for per-row dynamic IDs prefer Kit snapshots or explicit row observations rather than leaking strings.

Selector presence proves paint traversal reached the element, **not actual visibility**: recording happens before the `Visibility::Hidden` check; bounds can be clipped/offscreen, transparent or occluded. Check bounds intersect the viewport/content mask and assert semantic state. For virtual lists, instrument row creation/prepaint with stable row IDs and collect `(row_id, bounds, data_generation, ready)` per completed frame. Assert a bounded rendered range, full viewport coverage and correct scroll-generation alignment; do not merely assert the backing Vec contains rows. [G/src/elements/div.rs:878](/tmp/wispydiff-research/gpui-pre-0.3.6/src/elements/div.rs:878), [G/src/elements/div.rs:2520](/tmp/wispydiff-research/gpui-pre-0.3.6/src/elements/div.rs:2520), [G/src/window.rs:1028](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:1028), [G/src/app/test_context.rs:982](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/test_context.rs:982).

Kit 0.6.6 adds a better application-level layer, [K/src/test.rs:31](/tmp/wispydiff-research/gpui-kit-0.6.6/src/test.rs:31):

```rust
use gpui_kit::test::{TestSupportExt, TestWindowExt, TestAppContextExt};
// Trait TestWindowExt, implemented for Window:
find(&self, id: impl Into<ElementId>) -> ElementSnapshot
try_find(&self, id: impl Into<ElementId>) -> Option<ElementSnapshot>
within(&mut self, id: impl Into<ElementId>) -> ScopedWindow<'_>
render_frame(&mut self, cx: &mut App)
click / right_click / double_click / hover(id, cx)
click_at(id, offset: Point<Pixels>, cx)
scroll(id, delta: ScrollDelta, cx)
drag(from: Point<Pixels>, to: Point<Pixels>, cx)
drag_to(from_id, to_id, cx)
press(&mut self, key: &str, cx: &mut App)
input(&mut self, text: &str, cx: &mut App)
// Trait TestAppContextExt, implemented for TestAppContext:
wait_for(&mut self, window: AnyWindowHandle, timeout: Duration,
         predicate: impl FnMut(&mut Window, &mut App) -> bool) -> impl Future<Output=()>
```

Register custom interactive elements with `.id(...).test_support()` **before** `.track_focus(&handle)`; many Kit components already register their underlying controls. Scoped IDs follow existing GPUI identity scopes, including unobserved parents; ambiguous IDs fail with registered paths. Helpers render frames and dispatch native events; clicking still relies on real hit testing. `input` dispatches Unicode characters to existing focus; it neither focuses a control nor replaces its value. `wait_for` must run outside an active window borrow, redraws and polls each 10 ms of executor time.

`ElementSnapshot::{bounds,visible,path,role,label,value,focused,disabled,checked,selected,expanded,...}` is immutable; re-query after changes. Optional state `None` means unreported. `label`/`value` are accessibility properties, not extracted painted text. `visible` checks own visibility/opacity and intersection with content mask + viewport; it cannot prove pixels or infer an unobserved ancestor's opacity, nor does it prove no occluding overlay. [B/src/test_support.rs:65](/tmp/wispydiff-research/gpui-base-0.6.6/src/test_support.rs:65), [B/src/test_support.rs:345](/tmp/wispydiff-research/gpui-base-0.6.6/src/test_support.rs:345), [K/TESTING.md:17](/tmp/wispydiff-research/gpui-kit-0.6.6/TESTING.md:17).

For real glyph geometry without a display, use `HeadlessAppContext::with_platform(text_system: Arc<dyn PlatformTextSystem>, asset_source: Arc<dyn AssetSource>, renderer_factory: impl Fn() -> Option<Box<dyn PlatformHeadlessRenderer>> + 'static)`. `TestAppContext::build_with_text_system` and `TestApp::with_text_system` also exist. These let Linux tests use a real font system with pinned bundled fonts. `P::current_headless_renderer()` returns Metal on macOS and **None elsewhere in this version**, so portable headless layout does not imply portable screenshot support. Do not confuse `VisualTestContext` (a window-scoped TestAppContext) with `VisualTestAppContext` (macOS-only real offscreen window/capture support, main-thread constraints). [G/src/app/headless_app_context.rs:50](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/headless_app_context.rs:50), [P/src/gpui_platform.rs:101](/tmp/wispydiff-research/gpui-pre-platform-0.3.6/src/gpui_platform.rs:101), [G/src/app.rs:70](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app.rs:70).

## 2. How Longbridge tests components and whole views

The published packages contain both small builder/model tests and true rendered interaction tests:

| Source | Pattern worth copying |
|---|---|
| [C/src/setting/tests.rs:50](/tmp/wispydiff-research/gpui-component-0.6.6/src/setting/tests.rs:50) | `SettingsHost` owns component state inside Root; fixed size/font; draw; search; click navigation; inspect selected indices and debug bounds for present/absent settings. |
| [C/src/button/button.rs:1396](/tmp/wispydiff-research/gpui-component-0.6.6/src/button/button.rs:1396) | Real pointer and key-down/key-up activation, callbacks exactly once, loading/disabled blocking. |
| [C/src/accordion.rs:410](/tmp/wispydiff-research/gpui-component-0.6.6/src/accordion.rs:410) | Rendered geometry regression between headers, panels and following items. |
| [K/tests/ui.rs:60](/tmp/wispydiff-research/gpui-kit-0.6.6/tests/ui.rs:60) | Whole Profile view under Root: focus input, type Unicode, backspace, click Save, inspect bounds/accessibility and owning entity result. |
| [K/tests/collections.rs:96](/tmp/wispydiff-research/gpui-kit-0.6.6/tests/collections.rs:96) | Whole Table/Tree views: virtual row 150 initially absent; keyboard moves selection, scrolls row 32 into viewport, old rows disappear; wheel changes rendered lifetime. |
| [K/tests/overlays.rs:91](/tmp/wispydiff-research/gpui-kit-0.6.6/tests/overlays.rs:91) | Modal form validation/save, notification, deferred waits, Escape dismissal/focus restoration. Other suites cover menus, date pickers, Dock drag/zoom and lifecycle isolation. |
| [K/tests/rendering.rs:1](/tmp/wispydiff-research/gpui-kit-0.6.6/tests/rendering.rs:1) | Separate main-thread, `harness=false`, opt-in macOS Metal pixel tests; compare actual pixels to catch a missing check icon or transparent input text despite correct state. Linux/Windows explicitly skip this target. |

This is enough precedent for an in-process WispyDiff integration suite that mounts the **actual application root**, not a replacement widget per test. It is not external packaged-application automation and does not validate installers, OS focus, real clipboard/IME, window-manager behavior or platform rendering.

## 3. Blocking core work, Rayon, Tokio/reqwest and SQLite

`BackgroundExecutor::spawn<R>(&self, future: impl Future<Output=R> + Send + 'static) -> Task<R>` (`R: Send + 'static`) runs futures on production workers, but on the deterministic test scheduler's thread in tests. `Context<T>::spawn` runs a foreground async function receiving `(WeakEntity<T>, &mut AsyncApp)`; store its Task or detach it. Update views via the weak entity after awaiting work; call `cx.notify()` on state changes. [G/src/executor.rs:104](/tmp/wispydiff-research/gpui-pre-0.3.6/src/executor.rs:104), [G/src/app/context.rs:225](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/context.rs:225), [G/examples/testing.rs:286](/tmp/wispydiff-research/gpui-pre-0.3.6/examples/testing.rs:286).

Production bridge for synchronous core work can be this shape (service/result names illustrative):

```rust
let work = cx.background_executor().spawn(async move {
    service.prepared_rows(request) // owned inputs; synchronous Git/SQLite/Rayon work
});
cx.spawn(async move |this, cx| {
    let result = work.await;
    let _ = this.update(cx, |view, cx| {
        // Check request/view generation before applying a possibly stale completion.
        view.apply_rows(result);
        cx.notify();
    });
}).detach();
```

`spawn(async move { blocking_call() })` is not Tokio `spawn_blocking`: that poll occupies a worker until it returns, and in deterministic tests it occupies the test thread. It is acceptable for bounded existing core operations on worker threads, but long Git processes, lock contention and Rayon joins still block that worker. Bound concurrency; consider a dedicated blocking pool/Tokio `spawn_blocking` for long work. Do not hold UI borrows or application-wide locks across work/await. Keep RepoStore's per-repository write serialization; do not serialize all reads. Cancellation of a Task does not interrupt a synchronous call already running; generation checks are still necessary.

Real synchronous Git/temp-file/SQLite calls can run in a deterministic test; they simply block and their duration is not controlled. Rayon uses real OS threads; internal scheduling is not deterministic. A Rayon join can complete while blocking the test thread; it is dangerous if the joined work waits for the same GPUI executor/UI to run. `allow_parking` cannot preempt a blocked task poll or fix that dependency cycle.

GPUI is an executor, **not a Tokio reactor**. Wispy core's `github.rs` uses async reqwest; polling it directly on GPUI does not establish Tokio IO/time context. Run those futures on an owned multi-thread Tokio runtime with IO/time enabled, and bridge results back using an executor-neutral future/channel (`futures::channel::oneshot` is demonstrated upstream). Use runtime `spawn` for async HTTP and `spawn_blocking` for synchronous core work. Awaiting a spawned Tokio JoinHandle is possible outside Tokio; keep the runtime alive and shut it down outside UI borrows. Do not call a fresh runtime's `block_on` on the GPUI UI thread. This is an integration design recommendation, not a GPUI-provided Tokio adapter.

Real external completion is supported when opted in:

```rust
#[gpui_kit::test]
async fn receives_external_completion(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || { let _ = tx.send(42); });
    assert_eq!(rx.await.unwrap(), 42);
    cx.executor().forbid_parking();
}
```

This follows [G/examples/testing.rs:321](/tmp/wispydiff-research/gpui-pre-0.3.6/examples/testing.rs:321). It needs `futures` as a dependency. Default parking is forbidden: awaiting an externally completed future with no GPUI work can panic. `cx.executor().allow_parking()` or `cx.dispatcher.allow_parking()` allows the scheduler thread to park and receive cross-thread wakeups; it does **not** move GPUI futures onto real workers. In this pinned scheduler, allowed parking advances fake Clock by elapsed real time, stops auto-jumping timers in `block_until`, and has a 15-second hard limit per park operation. Always await an actual completion/readiness signal or bounded predicate; `run_until_parked()` alone can return before IO completes. [S/src/test_scheduler.rs:93](/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6/src/test_scheduler.rs:93), [S/src/test_scheduler.rs:488](/tmp/wispydiff-research/gpui-pre-scheduler-0.3.6/src/test_scheduler.rs:488).

Recommended split: deterministic tests inject prepared results/controlled channels at the app's service/worker boundary and exercise races via seeds; real-IO integration tests use a real Tokio runtime, temp SQLite/repo, fake local GitHub, parking enabled, and explicit completion assertions. Construct services under test directly per project rules. Keep process-level timeouts too: scheduler timeouts cannot stop a synchronous deadlock. Mock HTTP in GPUI's own FakeHttpClient does not intercept reqwest; use existing wiremock/fixture HTTP service for Wispy core.

## 4. Performance evidence and real-window runner

### Preserve the existing contract

SPEC.md: cached inbox open to first file visible **<150 ms**; range switch and mode toggle **<100 ms**; ~500 files/~50k changed lines; ~20k-line full file at 60 fps; usages <50 ms; streamed grep first hit <300 ms. Current perf.e2e.ts checks 5 cached-open samples and median <budget with every sample <2×budget; range/mode/usages/grep also use repeated samples. Scroll gates: p95 frame interval <=20 ms; fraction >33 ms <=2%; blank-frame fraction <=5%. It covers full diff, flick down/up, 20k file unified/split, repeated file jumps, file/global toggles and precomputed ranges. Preserve those scenarios and gates, rather than reducing the suite to average FPS.

The current harness waits for prefetch readiness, warms ranges, measures reopen separately from first open, and rejects measurements when the window becomes hidden. Preserve these distinctions. SPEC/Justfile explicitly keep performance gates **local only** because shared CI hosts fluctuate; CI should continue to enforce correctness, with real-window software-renderer timing reported separately.

### Useful built-in instrumentation, already in the pinned package

Enable a proposed app `perf-script` feature forwarding `gpui-kit/profiler`; keep `gpui-kit/test-support` out of the release perf binary. In G:

| Signature / source | Meaning |
|---|---|
| `Window::on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static)`; [window.rs:2610](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:2610) | Requests platform frame demand; callbacks can drive bounded scripted input. |
| `Window::request_animation_frame(&self)`; [window.rs:2630](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:2630) | Schedules notification of the current view on the next frame. It does not provide a frame timestamp. |
| `Window::frame_duration_snapshot(&self) -> profiler::FrameDurationSnapshot`; [window.rs:3475](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:3475) | Histograms for draw duration, dirty-to-present duration, intervals between submitted animation frames. Feature `profiler`. |
| `Window::input_latency_snapshot(&self) -> profiler::InputLatencySnapshot` | Input-to-submission latency histogram and event coalescing, useful diagnostics alongside app marks. |
| `profiler::set_trace_enabled(bool) -> bool`; `FrameTimingCollector::new()`; `collect_unseen(&mut self) -> Vec<FrameEvent>`; [profiler.rs:707](/tmp/wispydiff-research/gpui-pre-0.3.6/src/profiler.rs:707), [profiler.rs:1225](/tmp/wispydiff-research/gpui-pre-0.3.6/src/profiler.rs:1225) | Raw Draw/Present events with window IDs and monotonic timestamps; tracing must be enabled for these events. Drain regularly to avoid ring-buffer loss. |
| `FrameTiming::{draw_start,draw_end,dirty_at}`; `PresentTiming::{present_start,present_end,animation_interval}`; [profiler.rs:788](/tmp/wispydiff-research/gpui-pre-0.3.6/src/profiler.rs:788) | CPU traversal and platform submission timestamps. These are not GPU-completion or actual scanout timestamps. |

**Important timing trap:** despite the method's documentation saying “after the current frame is rendered,” this snapshot's platform request-frame callback drains `on_next_frame` callbacks **before** that turn's `window.draw()` and `window.present()`. A mark emitted merely because the model loaded, or in that callback before inspecting completed output, can undercount first-visible latency. [G/src/window.rs:1780](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:1780).

Use app-owned `(request_generation, rendered_generation, viewport, row coverage)` observations produced by the actual render/prepaint/paint path. A paint observation means the Scene contains the correct ready rows, not that submission finished. Pair that frame's generation with the subsequent profiler Present event for the same window; end latency at `present_end`. Collect from the following callback/foreground turn so the preceding submission exists. Retain readiness from the last completed frame; never infer completion solely from the current model. Separate CPU draw/submission numbers from physical display latency. Aggregate histograms work without detailed tracing, but fresh collector/session boundaries make per-scenario attribution clearer than cumulative histograms.

For scrolling, chain `on_next_frame` callbacks, dispatch `ScrollWheelEvent` through `window.dispatch_event(event.to_platform_input(), cx)` (trait `InputEvent`) at the viewport position or invoke the same public scroll command used by input, then request another frame. Record callback intervals as event-loop cadence and Present-event intervals as submitted-frame cadence; they are different metrics. Verify every driven step caused the intended scroll delta/frame and reject zero/missing samples. Do not use a fast CPU loop over 600 `draw()` calls as “60 fps scrolling.”

For blank/stale-frame checks, intersect painted ready-row rectangles with viewport/clip, preserve structural exceptions such as headers/gap rows/empty-state views, and check coverage at the same nine vertical sample positions as the current flick tests (or stronger union coverage). Include row/request generation and expected scroll position so a fully populated but stale frame fails. This observation should be cheap, feature-gated and work in release without debug selectors. Pixel/screenshot checks on a real renderer remain useful to catch invisible text or styling that geometry cannot.

### Zed precedent

Current upstream checkout Z contains Criterion-backed `#[gpui::bench]`, `BenchAppContext::{bench_iter,bench_task,bench_renderer,bench_renderer_session}`, and frame/foreground-work reporting. [Z/crates/benchmarks/benches/editor_render.rs:140](/tmp/wispydiff-research/zed/crates/benchmarks/benches/editor_render.rs:140) builds a full Editor entity and calls `bench_renderer(editor, |editor, window, cx| ...)`, alternately moving up/down. Other cases render text/styles/multibuffers. [Z/crates/gpui/benches/focus_cleanup.rs:7](/tmp/wispydiff-research/zed/crates/gpui/benches/focus_cleanup.rs:7) uses inputs/FPS and explicitly requires optimized builds. [Z/script/gpui-bench-compare:1](/tmp/wispydiff-research/zed/script/gpui-bench-compare:1) alternates baseline/candidate runs, retains raw samples and compares foreground instructions/wall timings. Immutable upstream source: [editor_render.rs](https://github.com/zed-industries/zed/blob/aff99ecc3249cfa95e720c4d6c1eb4a72105a3ee/crates/benchmarks/benches/editor_render.rs).

The pinned G also already has these bench APIs behind `bench-support` (`bench` alias), with real ThreadedDispatcher workers and platform text shaping: [G/src/app/bench_context.rs:29](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/bench_context.rs:29), [G/src/app/bench_context.rs:779](/tmp/wispydiff-research/gpui-pre-0.3.6/src/app/bench_context.rs:779). They are useful additional micro/integration benchmarks. Kit 0.6.6 does not forward a `bench-support` feature; using this would need a matching renamed direct gpui-pre dependency/platform/criterion setup. Its headless renderer factory is Metal-only in the inspected platform release; Linux presentation discards scenes. `bench_renderer_session` documents an **unpaced** work/render policy, not OS event-loop emulation. Therefore these benches complement, rather than replace, the real-window 60 fps gate. Do not infer Linux GPU submission from the compare script's generic prose when the platform implementation returns None.

### Linux real windows: Xvfb, Weston, renderer

The pinned L **uses wgpu**, not blade: both X11Window and WaylandWindow create WgpuRenderer. WgpuContext enables Vulkan and GL backends, prefers non-CPU adapters then Vulkan over GL, and tests surface compatibility. CPU adapters are accepted by initial `WgpuContext::new`; software rejection is used by a recovery path, not initial selection. `ZED_ALLOW_EMULATED_GPU=1` from old blade instructions is not consulted in these renderer sources. Do not rely on `WGPU_BACKEND` selecting a backend either: this snapshot constructs an explicit `VULKAN | GL` instance rather than parsing that variable. [L/src/linux/x11/window.rs:13](/tmp/wispydiff-research/gpui-pre-linux-0.3.6/src/linux/x11/window.rs:13), [L/src/linux/wayland/window.rs:45](/tmp/wispydiff-research/gpui-pre-linux-0.3.6/src/linux/wayland/window.rs:45), [W/src/wgpu_context.rs:289](/tmp/wispydiff-research/gpui-pre-wgpu-0.3.6/src/wgpu_context.rs:289), [W/src/wgpu_context.rs:334](/tmp/wispydiff-research/gpui-pre-wgpu-0.3.6/src/wgpu_context.rs:334), [W/src/wgpu_renderer.rs:259](/tmp/wispydiff-research/gpui-pre-wgpu-0.3.6/src/wgpu_renderer.rs:259).

**Feasibility:** a real Linux GPUI window can target a virtual X11 server or Wayland compositor, provided the selected graphics adapter can configure/present to that surface. Source inspection supports both paths; these exact recipes still need a smoke run with pinned distro/driver versions. Xvfb supplies an X display, not a GPU. Use Mesa's lavapipe Vulkan ICD (or GL llvmpipe fallback where supported). Mesa documents [VK_DRIVER_FILES](https://docs.mesa3d.org/envvars.html#vk-driver-files) and [the lvp ICD filename](https://docs.mesa3d.org/install.html); the distro's actual path varies. `LIBGL_ALWAYS_SOFTWARE` selects GL software rendering, not the Vulkan ICD. Check logged selected adapter/backend and surface-configuration success, not just process launch.

Proposed X11 smoke command after building a future perf binary and preparing fixture environment:

```sh
# Illustrative Linux x86_64 ICD path; discover the installed path on the runner.
env -u WAYLAND_DISPLAY -u ZED_HEADLESS \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json \
  GPUI_X11_SCALE_FACTOR=1 \
  xvfb-run -a -s '-screen 0 1440x900x24' \
  target/release/wispydiff --perf-script worst-case --json /tmp/wispy-perf.json
```

Those CLI flags are a **proposed app interface**, not existing commands. Ensure window activation (possibly a lightweight window manager) and verify the app is actually drawing; Xvfb alone does not guarantee focus. GPUI throttles some inactive animation frames to ~30 fps. `guess_compositor()` picks nonempty WAYLAND_DISPLAY before DISPLAY; presence of ZED_HEADLESS forces the non-rendering path. Unsetting these explicitly prevents accidentally measuring that path. [G/src/platform.rs:158](/tmp/wispydiff-research/gpui-pre-0.3.6/src/platform.rs:158), [G/src/window.rs:1740](/tmp/wispydiff-research/gpui-pre-0.3.6/src/window.rs:1740).

Weston explicitly supports a headless backend for tests and software/GPU compositor renderers. A modern candidate is `weston --backend=headless --renderer=pixman --socket=wispy-test --width=1440 --height=900 --idle-time=0`, with a private mode-0700 XDG_RUNTIME_DIR and client WAYLAND_DISPLAY=wispy-test; check `weston --help` for the installed release (`headless-backend.so`/`--use-pixman` are older spellings). Wait for the socket before launch and clean up the owned process. [Official Weston documentation](https://wayland.pages.freedesktop.org/weston/toc/running-weston.html). The compositor's Pixman renderer and the application's Vulkan/GL renderer are separate: Pixman does not remove GPUI's graphics-device/surface requirements. Validate shared-memory/dmabuf and presentation support with the chosen Mesa stack. Cage can be a wlroots-based alternative, but “run cage” alone is not a proven headless setup; a supported headless backend/output/renderer must be configured for that installed wlroots version. Prefer one pinned Weston/Xvfb recipe first.

`gpui_kit::platform::headless()`/`ZED_HEADLESS` is a different facility: L's HeadlessWindow has `fn draw(&self, _scene: &Scene) {}`. It can run background application work without a display; it cannot establish graphics or 60 fps budgets. [L/src/linux/headless/window.rs:221](/tmp/wispydiff-research/gpui-pre-linux-0.3.6/src/linux/headless/window.rs:221).

Virtual-display + software rendering is appropriate for real-platform smoke/visual regressions, with renderer-qualified performance trends if desired. Binding 60 fps/latency gates should use the same local hardware/display configuration as the baseline (or dedicated hardware runners), fixed window size/scale/fonts/refresh rate, optimized build, active window, no concurrent benchmark, warmed shaders/cache as specified. Record CPU/GPU/driver/compositor/build/fixture metadata and reject inactive/hidden/zero-frame runs. CPU software-renderer numbers are not interchangeable with GPU desktop numbers.

## 5. Concrete architecture and migration

| Layer | Test mechanism | Coverage |
|---|---|---|
| wispy-core | Existing Rust tests, direct service construction, throwaway git, SQLite, wiremock | Diff/attribution/rows/highlighting, cache/checkpoints/rebases, GitHub protocol, posting/retry; unchanged authority. |
| UI state and service bridge | Ordinary Rust tests + seeded GPUI entity tests | Selection/range/navigation, request-generation cancellation, stale results, persistence, loading/error/offline states. Keep view-independent logic out of widgets. |
| Actual GPUI application root | `#[gpui_kit::test]`, Root, deterministic prepared data/controlled completion, real keymaps/input/hit testing | Port functional suites: palette/inbox/ranges, focus/shortcuts, viewed undo, modes/wrap/anchor preservation, filters/tree, comments/drag, navigation history, async panel behavior. Assert entity outcomes **and** rendered bounds/row identity/viewport coverage. |
| Root + real services | Async GPUI tests, real worker/Tokio runtime, `allow_parking`, temp dirs, fixture server/fake CLIs | Cached open/offline persistence, background refresh, generation races, actual Git/SQLite/HTTP integration, review submission and assistant/MCP orchestration. Smaller suite; external work is not seed-deterministic. |
| Native platform/renderer | Real app smoke under Xvfb/Weston on Linux, macOS main-thread renderer harness where useful | Launch/quit, fonts/icons, focus/clipboard/IME/manual checks, screenshots; no assumption that state snapshots prove pixels. |
| Binding perf | Feature-gated release app script + external process runner and JSON assertions | All current SPEC/perf.e2e scenarios, real frame submission/cadence, no blank/stale rows, existing threshold policy. Local gate; separate CI software-renderer results. |

Use a fresh TestAppContext/window per functional test; avoid preserving WebdriverIO's accidental shared-process dependence. Give each real-service test its own data/cache/database/origin (and fake API endpoint/log); use unique ports or dynamically allocated ones, own shutdown handles, and no global env mutation across concurrent tests. Existing `wispy-fixtures generate --out DIR --seed 42` and `serve --fixture DIR --port PORT` are reusable subprocesses today (the crate is a binary with private generation/server modules, not yet an importable test library). Fake clone URLs already point to the temporary local origin. Continue using fake assistant executables/opener, never real CLIs/token/network or user working copies.

### Minimal functional test skeleton using the real published API

This deliberately self-contained review host illustrates mounting Root, component input, pointer activation and both state/layout assertions. It follows K/tests/ui.rs; replacing ReviewHost with the actual Wispy root gives the intended integration-test boundary. No imagined `test.click(selector)` or DOM is involved. Add the dev dependency above and put this in a test target; do not import the facade with `*`.

```rust
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, Render, SharedString,
    TestAppContext, Window, div, prelude::*, px, size,
    component::{Root, button::Button, input::{Input, InputState}},
    test::{TestSupportExt, TestWindowExt},
};

struct ReviewHost {
    input: Entity<InputState>,
    draft: Option<SharedString>,
}

impl Render for ReviewHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.draft.clone().unwrap_or_else(|| "No draft".into());
        div().size_full().flex().flex_col().gap_4().p_4()
            .child(Input::new(&self.input).id("comment").w(px(300.)))
            .child(Button::new("save-draft").label("Save draft")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.draft = Some(this.input.read(cx).value());
                    cx.notify();
                })))
            .child(div().id("draft-status").test_support()
                .aria_label(status.clone()).child(status))
    }
}

#[gpui_kit::test]
fn saves_draft_through_the_view(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut owner = None;
    let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
        let view = cx.new(|cx| ReviewHost {
            input: cx.new(|cx| InputState::new(window, cx)),
            draft: None,
        });
        owner = Some(view.clone());
        Root::new(view, window, cx)
    });
    let owner = owner.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("comment", cx);
        window.input("Please keep this line 中文", cx);
        assert_eq!(window.find("comment").focused(), Some(true));
        window.click("save-draft", cx);
        let status = window.find("draft-status");
        assert!(status.visible());
        assert_eq!(status.label(), Some("Please keep this line 中文"));
        assert!(status.bounds().top() >= window.find("save-draft").bounds().bottom());
    }).unwrap();
    assert_eq!(owner.read_with(cx, |view, _| view.draft.clone()).as_deref(),
               Some("Please keep this line 中文"));
}
```

Actual app tests should additionally drive registered key bindings (`window.press("ctrl-k", cx)` / `cmd-k`), click stack chips with explicit modifiers where required, wait on asynchronous ready generations, scroll the real virtual list and assert that offscreen row IDs disappear. Never satisfy a functional test solely by calling its handler directly. Direct action dispatch remains useful for tests specifically about actions rather than shortcut routing.

### Proposed release perf runner contract

1. An external Rust/shell runner builds optimized app + fixture generator, generates an isolated seed-42 origin, starts fake GitHub, assigns WISPY_DATA_DIR/API/TOKEN/fake-CLI settings, and launches one real-window app with `--perf-script worst-case --json PATH`. Own all processes and enforce a wall-clock timeout/nonzero failures. No WebDriver server is needed.
2. Script initialization uses production services/settings/fonts/assets. Wait for explicit prefetch and all 10 contiguous ranges of the 4-PR stack; warm shader/text pipelines outside timed windows. Record the first prefetched process open separately. Repeat cached inbox opens and alternate away/reopen, as today; do not pre-open the target during each timed sample's setup.
3. Drive the same actions/input/list-scroll APIs as users. Start request timing immediately before dispatch; carry a sample/generation ID through loading, row preparation and paint. End only when correct ready viewport rows have been drawn **and submitted**. Run range/mode toggles, usages and grep with their own visible-generation marks. Use std::time::Instant for elapsed release timing.
4. Run paced scroll/flick/held-key scenarios for the same frame counts/speeds. Collect raw frame timestamps/durations plus row coverage, sample counts, scroll offsets and largest per-frame movement. Keep callbacks scheduled during animation so present-interval histograms have valid animation samples; reject inactive/throttled/hidden runs. Preserve baseline thresholds and median/worst-sample policy.
5. Write structured JSON once outside the hot loop (or buffer compact raw records): schema version, revision, fixture seed/counts, hardware/renderer, size/scale/refresh, prefetch/cache status, each latency sample, draw/submission/callback intervals, blank/stale counts, movement, errors. Return a failing exit code on threshold/coverage/timeout errors. Avoid stdout logging per frame; use stderr diagnostics and a final machine-readable result/path.
6. Keep `just bench` semantics as the binding local check. CI runs headless correctness and a small native-renderer smoke; optional dedicated hardware jobs compare equivalent renderer metadata. Add Criterion core/render benchmarks only for regression diagnosis; they do not redefine SPEC's latency or smooth-scroll contract.

Suggested first implementation order: headless root mount + palette/input/keymap test; worker bridge + one fixture-backed cached-open test; virtual-list coverage/anchor tests; then release perf script before claiming the GPUI rewrite meets SPEC. SPEC's Tauri/WebdriverIO architecture/testing language would need an explicit update when the rewrite is accepted; this research does not change that contract.
