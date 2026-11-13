# GPUI spike reference — gpui-kit 0.6.6

Source-checked on 2026-11-13. This is Longbridge **gpui-kit**, not gpui-ui-kit.
Scope: a window + label only; no core integration or product features.

## Versions, build and source map

- Dependency: `gpui-kit = "~0.6.6"` (latest 0.6.x; published on crates.io, no git dependency).
  Default features: `component`, `assets`. Kit re-exports GPUI at its root,
  Base at `gpui_kit::base`, Component at `gpui_kit::component`, assets at `gpui_kit::assets`.
- [K/Cargo.toml:354](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-kit-0.6.6/Cargo.toml:354) pins `gpui-pre = "=0.3.6"`; the desktop
  `gpui-pre-platform = "=0.3.6"` enables `font-kit`, `x11`, `wayland`, `runtime_shaders`.
  Resolved Base, Component, Component macros and Kit assets: **0.6.6**; all GPUI pre crates: **0.3.6**.
  Cargo.lock is the reproducibility record; do not substitute upstream GPUI 0.2 or a random Zed revision.
  Kit package VCS metadata identifies commit `9765ae2c9a5eccfa13891248a445991e6f6a09d8`
  (`gpui-kit-0.6.6/.cargo_vcs_info.json`, path `crates/kit`). GPUI is pinned by **registry version**,
  not by git rev; the gpui-pre 0.3.6 archive contains no .cargo_vcs_info.json to identify an upstream SHA.
- Rust 1.94 fails in [G/profiler.rs:473](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/profiler.rs:473) and :494 (`std::hint::cold_path`).
  That function became stable in **1.95** ([Rust docs](https://doc.rust-lang.org/stable/std/hint/fn.cold_path.html)).
  New crate declares `rust-version = "1.95"` for that requirement; only **1.100** was verified,
  not a complete transitive-dependency MSRV audit.
- **Build passed** with Rust/Cargo **1.100.0** (default stable upgraded from 1.94, per authorization;
  duplicate versioned toolchain removed). Initial 1.94 attempt: **137.90 s**, failed on cold_path.
  Fresh 1.100 compilation: **187.10 s**, then failed at link on missing xkbcommon-X11 dev library.
  After user-local package extraction, successful incremental link: **1.78 s** (Cargo: 1.74 s).
  Timings use `/usr/bin/time -p`, workspace dev settings (dependencies opt-level=2), 20 logical CPUs.
- **Missing required apt package: `libxkbcommon-x11-dev`** (candidate 1.13.1-1). Runtime
  `libxkbcommon-x11-0` already installed, but `libxkbcommon-x11.so` link stub absent.
  No sudo/system package installation was used. Downloaded dev + matching runtime .debs and extracted
  under `/tmp/wispydiff-sysdeps`; build command from workspace root:

  ```sh
  LIBRARY_PATH=/tmp/wispydiff-sysdeps/usr/lib/x86_64-linux-gnu cargo build -p wispydiff
  ```

  To reproduce without installing system packages:

  ```sh
  mkdir -p /tmp/wispydiff-apt
  (cd /tmp/wispydiff-apt && apt download libxkbcommon-x11-dev libxkbcommon-x11-0)
  for package in /tmp/wispydiff-apt/*.deb; do
      dpkg-deb -x "$package" /tmp/wispydiff-sysdeps
  done
  ```

  A clean bare build needs that apt package installed or the local LIBRARY_PATH above; the temporary
  extraction is not part of this repository and does not survive deleting /tmp. The binary uses the
  already-installed runtime .so.0 and needs no custom LD_LIBRARY_PATH.
- Present native build dependencies checked: xkbcommon **1.13.1**, Wayland client **1.24.0**,
  fontconfig **2.17.1**, X11 **1.8.13**, FreeType/link libraries; Vulkan loader and Mesa Vulkan drivers
  installed. `vulkan.pc` is absent (`libvulkan-dev`), **not a build blocker** in this configuration.
  Additional conventional apt names if provisioning a clean machine: `libxkbcommon-dev`,
  `libwayland-dev`, `libfontconfig-dev`, `libfreetype-dev`, `libxcb1-dev`; runtime `libvulkan1`,
  `mesa-vulkan-drivers`. These additional packages are not claimed missing here.
- Logs: [1.94 attempt](/tmp/wispydiff-build.log), [1.100 compilation](/tmp/wispydiff-build-1.100.log),
  [successful link](/tmp/wispydiff-build-final.log), [native smoke run](/tmp/wispydiff-run.log).
  The binary stayed alive for an eight-second Wayland smoke run, then timeout terminated it
  (exit 124, expected). No panic; Mesa printed an experimental Xe KMD warning twice. This checks
  native startup, not pixel correctness. A subsequent bare cached `cargo build -p wispydiff` passed
  in **0.33 s** ([log](/tmp/wispydiff-build-bare.log)); the local path is still needed on a fresh link.
- Registry source root on this machine:
  `/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.
  Links below expand these abbreviations: **K** = `gpui-kit-0.6.6/src`,
  **C** = `gpui-component-0.6.6/src`, **B** = `gpui-base-0.6.6/src`,
  **G** = `gpui-pre-0.3.6/src`, **L** = `gpui-pre-linux-0.3.6/src`,
  **P** = `gpui-pre-platform-0.3.6/src`. Cargo.toml links are at the crate root.
  Signatures below omit bodies and sometimes generic bounds; paths point to the complete definition.

## a. Bootstrap, themes, appearance

[K/lib.rs:134](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-kit-0.6.6/src/lib.rs:134) contains the Component bootstrap; the scaffold follows it:

```rust
use gpui_kit::{component::Root, prelude::*, *};
gpui_kit::application().run(|cx| {
    gpui_kit::init(cx); // exactly once, before constructing controls
    cx.spawn(async move |cx| {
        cx.open_window(WindowOptions::default(), |window, cx| {
            let view = cx.new(|_| Hello);
            cx.new(|cx| Root::new(view, window, cx))
        }).expect("open window");
    }).detach();
});
```

- `gpui_kit::init(cx: &mut App)` initializes enabled layers (Component also initializes Base).
  `application() -> Application` uses the OS platform, [P/gpui_platform.rs:13](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-platform-0.3.6/src/gpui_platform.rs:13).
  `Root::new(view: impl Into<AnyView>, window: &mut Window, cx: &mut Context<Self>) -> Self`, [C/root.rs:100](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/root.rs:100).
  Root must remain the **window's actual root entity**, since WindowExt downcasts it.
  `WindowOptions` provides bounds/titlebar/window options; use native window bounds rather than DOM concepts.
- Root owns focus/tab handling, selection, tooltip/native-menu overlays and Linux client borders.
  **Dialogs, sheets and notifications still need explicit layers in the application's render**:
  `.children(Root::render_sheet_layer(window, cx))`,
  `.children(Root::render_dialog_layer(window, cx))`,
  `.children(Root::render_notification_layer(window, cx))` ([C/root.rs:185](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/root.rs:185), :215, :242).
  Place them after the main content in a relative, full-size container. Root's own render does not add these.
- Theme access: `ActiveTheme::theme(&self) -> &Theme` on App (contexts deref to App);
  `Theme::global(cx: &App) -> &Theme`, `global_mut(cx: &mut App) -> &mut Theme`.
  `ThemeMode` has **Light/Dark only**, no System variant ([C/theme/mod.rs:701](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/theme/mod.rs:701)).
  Init starts Light; following OS is application policy:
  `Theme::sync_system_appearance(window: Option<&mut Window>, cx: &mut App)`;
  subscribe with `cx.observe_window_appearance(window, callback) -> Subscription` ([G/app/context.rs:457](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app/context.rs:457)),
  retain the subscription, and only sync while the saved preference is System.
- Custom palette: `ThemeConfig { name, mode, colors: ThemeConfigColors, highlight, font_size,
  font_family, mono_font_family, mono_font_size, radius, radius_lg, shadow, ..Default::default() }`.
  Colors in `ThemeConfigColors` are optional string values; missing colors inherit Light/Dark defaults
  ([C/theme/schema.rs:38](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/theme/schema.rs:38), :253, :1060). `highlight` uses `HighlightThemeStyle`.
  `ThemeSet { name, author, url, themes: Vec<ThemeConfig> }` supports paired variants.
  `ThemeRegistry::global_mut(cx).load_themes_from_str(content: &str) -> anyhow::Result<()>`;
  `.themes() -> &HashMap<SharedString, Rc<ThemeConfig>>` ([C/theme/registry.rs:121](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/theme/registry.rs:121), :151).
  Registry loading alone does not activate a palette.

```rust
use std::rc::Rc;
use gpui_kit::component::theme::{Theme, ThemeConfig, ThemeConfigColors, ThemeMode};
let custom_light = Rc::new(ThemeConfig {
    name: "Wispy Light".into(), mode: ThemeMode::Light,
    colors: ThemeConfigColors {
        background: Some("#fdf6e3".into()), foreground: Some("#586e75".into()),
        ..Default::default()
    }, ..Default::default()
});
// Construct custom_dark the same way with mode: ThemeMode::Dark.
Theme::global_mut(cx).light_theme = custom_light;
Theme::global_mut(cx).dark_theme = custom_dark;
Theme::change(ThemeMode::Dark, Some(window), cx);
```

- `Theme::change(mode: impl Into<ThemeMode>, window: Option<&mut Window>, cx: &mut App)`
  applies the selected config, updates the Base theme and TextView defaults; refreshes the supplied window
  ([C/theme/mod.rs:261](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/theme/mod.rs:261)). Theme is **App-global**, not per-window. For all-window repaint request `cx.refresh_windows()`.
  Direct mutations require `Theme::sync_base(cx)` and repaint ([C/theme/mod.rs:367](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/theme/mod.rs:367)).
  New theme surface also has `semantic_tokens`, `apply_semantic_config`, `apply_semantic_config_str`;
  application-owned styling should prefer semantic tokens over extending legacy per-control fields.
- Native chrome: `cx.set_window_appearance(Option<WindowAppearance>)` ([G/app.rs:1508](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app.rs:1508)):
  Some(Light/Dark) overrides macOS NSApplication; None follows OS. **No-op on other platforms**.
  `Theme::change` does not call it. `window.appearance()` reads OS/effective appearance;
  `window.set_background_appearance(...)` controls opaque/transparent/vibrant background, not light/dark.
- Assets feature makes the bundle available; it is not automatically installed by `init`.
  If adding icons later: `application().with_assets(gpui_kit::assets::Assets)`
  (registry `gpui-kit-assets-0.6.6/src/lib.rs:17`). Label-only scaffold needs no icon assets.

## b. Virtualization and scrolling

**GPUI fixed-height list**, [G/elements/uniform_list.rs:22](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/uniform_list.rs:22):

```rust
pub fn uniform_list<R: IntoElement>(
    id: impl Into<ElementId>, item_count: usize,
    f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>,
) -> UniformList;
```

- All rows must share a height. One row is measured; callback also runs for measurement, so it is
  not a pure visibility notification. Give the list bounded height and clipped overflow.
- Persist `UniformListScrollHandle::new()` in view state; `.track_scroll(&handle)` on the list.
  `handle.scroll_to_item(ix, ScrollStrategy::{Top,Center,Bottom,Nearest})`;
  `scroll_to_item_strict(...)` forces placement even if visible;
  `scroll_to_item_with_offset(ix, strategy, offset: usize)` uses **item count**, not pixels.
  `logical_scroll_top_index() -> usize` returns the first row ([G/elements/uniform_list.rs:150](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/uniform_list.rs:150), :182, :222).
  `handle.0.borrow().base_handle` is its underlying ScrollHandle, with pixel offsets/bounds.
  Callback range is the actual requested render range; there is no direct public visible_range getter.

**GPUI variable-height list**, [G/elements/list.rs:24](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/list.rs:24), :314:

```rust
pub fn list(state: ListState,
    render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static) -> List;
ListState::new(item_count: usize, alignment: ListAlignment, overdraw: Pixels) -> ListState;
```

- Persist/clone `ListState`; backed by a SumTree of measured/unmeasured items. Use `ListAlignment::Top`.
  `overdraw` is extra pixels measured/rendered outside the viewport: can explicitly use one viewport
  for WispyDiff. Unknown row heights are estimated, so total scroll extent can change during measurement.
  `.with_uniform_item_height(height)` seeds estimates without rendering every row;
  `.measure_all()` renders/measures **all items initially**, costly for large diffs.
- `reset(count)`, `splice(old_range: Range<usize>, count)`, `remeasure()`, `remeasure_items(range)`
  maintain changes; resize/wrap/theme changes must invalidate stale heights. Reset drops scroll events
  until the next paint ([G/elements/list.rs:355](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/list.rs:355)–412, :503).
- `scroll_to(ListOffset { item_ix, offset_in_item: Pixels })`, `scroll_by(distance: Pixels)`,
  `scroll_to_reveal_item(ix)`, `scroll_to_end()`, `logical_scroll_top() -> ListOffset`.
  Positive `offset_in_item` consumes that much of the first item above the viewport.
  `bounds_for_item(ix) -> Option<Bounds<Pixels>>`, `viewport_bounds()`.
- Visibility: `set_scroll_handler(handler: impl FnMut(&ListScrollEvent, &mut Window, &mut App) + 'static)`;
  event has `visible_range: Range<usize>`, `count`, `is_scrolled`, `is_following_tail`
  ([G/elements/list.rs:172](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/list.rs:172), :552). Do not mutate/reborrow the same ListState from inside its callback
  without checking borrow lifetime; forward the range to application state/defer work. The handler is called from native scroll handling,
  not a general initial-layout/programmatic-scroll observer. `count` is total item count in the
  implementation (despite its doc comment saying visible count); for initial/programmatic visibility,
  combine logical_scroll_top/bounds with the application row-size model.
- `ListState` includes follow-tail controls (`FollowMode`, pause_following_tail) useful for assistant output.

**Base/Component VirtualList**, [C/virtual_list.rs:1](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/virtual_list.rs:1) re-exports [B/virtual_list.rs:139](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/virtual_list.rs:139):

```rust
pub fn v_virtual_list<R: IntoElement, V: Render>(
    view: Entity<V>, id: impl Into<ElementId>, item_sizes: Rc<Vec<Size<Pixels>>>,
    f: impl 'static + Fn(&mut V, Range<usize>, &mut Window, &mut Context<V>) -> Vec<R>,
) -> VirtualList;
// h_virtual_list has the same signature, using widths instead of heights.
```

- Sizes are supplied for **all rows**, only height matters vertically; the cross-axis size is measured
  from one item (default first; `.with_item_to_measure_index(index)` selects another).
  Good candidate for core-computed wrapped row heights. Persist the sizes Rc and list element ID.
- Prefix origins/sizes are cached, visible edges binary searched. Source compares
  `state.items_sizes != self.item_sizes` using Vec value equality, which can scan all rows if the Rc
  allocation changes; keep the same Rc for unchanged sizes. Changed arrays rebuild O(n) prefix sums.
  Cache rebuild tests only item_sizes, so changing only `gap` can leave stale prefix sizes
  ([B/virtual_list.rs:477](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/virtual_list.rs:477)). Prefer zero gap, include it in supplied heights.
- **Overdraw is one item after the viewport, not a screen on each side**, and has no public
  overdraw setting ([B/virtual_list.rs:383](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/virtual_list.rs:383)). A future renderer must address the SPEC's screen overdraw
  requirement; choosing this component alone does not satisfy it.
- `VirtualListScrollHandle::new()`, `.track_scroll(&handle)`,
  `handle.base_handle() -> &ScrollHandle`, `.scroll_to_item(ix, strategy)`, `.scroll_to_bottom()`.
  **Only Center is handled distinctly; every other ScrollStrategy is nearest/reveal**, despite
  sharing GPUI's enum ([B/virtual_list.rs:265](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/virtual_list.rs:265)). For exact top positioning use known prefix sums and
  the base handle's pixel offset. Visible/render range arrives in callback; no public range getter.

**Raw ScrollHandle**, [G/elements/div.rs:4258](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/div.rs:4258), :4394:
`offset() -> Point<Pixels>`, `set_offset(Point<Pixels>)`, `max_offset()`, `bounds()`,
`scroll_to_item(ix)`, `scroll_to_top_of_item(ix)`, `logical_scroll_top() -> (usize, Pixels)`.
Attach via `div().id(...).overflow_y_scroll().track_scroll(&handle)`.
Offsets become **negative** when scrolling down/right. `top_item`/`bottom_item` are indices into
actual child bounds; do not assume they are global virtual row indices. Mutating a scroll handle
alone is not a reliable repaint request: `cx.notify()` on the view or `window.refresh()`.

**Smooth programmatic scrolling:** these handle methods are immediate/deferred jumps, with no
`behavior: smooth` API. Interpolate offset each frame with elapsed time, set_offset / ListState::scroll_by,
then refresh; `window.on_next_frame(FnOnce(&mut Window,&mut App))`, `request_animation_frame()`
([G/window.rs:2610](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/window.rs:2610), :2630). Base has
`motion::transition<T>(id, target, policy: Transition, window, cx) -> T`
where `T: Interpolate + PartialEq + 'static` ([B/motion.rs:262](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/motion.rs:262)); called within keyed element rendering,
it handles transition state/frame requests, then feed its sampled value to the scroll handle.
Initialize at current offset before changing target; cancel on user wheel/key scroll; E-Ink uses a jump.
For ListState, keep an application pixel/prefix model if targets cross unknown heights.

**Table:** in 0.6.6 `Table` is the compositional/static table ([C/table/table.rs:40](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/table/table.rs:40)); the virtualized,
interactive control is **DataTable<D>**, `DataTable::new(&Entity<TableState<D>>)` ([C/table/data_table.rs:101](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/table/data_table.rs:101)).
`TableState::new(delegate, window, cx)`, `scroll_to_row(row_ix, cx)`, `scroll_to_col(col_ix, cx)`,
`visible_range() -> &TableVisibleRange` (`rows()`, `cols()` return `&Range<usize>`), [C/table/state.rs:267](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/table/state.rs:267), :574.
`TableDelegate` requires `columns_count(&self, &App) -> usize`, `rows_count(&self, &App) -> usize`,
`column(&self, usize, &App) -> Column`, `render_td(&mut self, row_ix, col_ix, &mut Window,
&mut Context<TableState<Self>>) -> impl IntoElement` ([C/table/delegate.rs:16](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/table/delegate.rs:16)).
It has sorting, fixed/resizable columns and selection, but code diff alignment/custom heights warrant
using list primitives. Some examples still show outdated `TableState::new(delegate, cx)` signatures.

## c. Code lines, syntax runs, word backgrounds, metrics

[G/elements/text.rs:399](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/text.rs:399), :433, :526; [G/style.rs:580](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/style.rs:580); [G/text_system.rs:1228](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/text_system.rs:1228):

```rust
StyledText::new(text: impl Into<SharedString>) -> StyledText;
.with_runs(runs: Vec<TextRun>) -> Self;
.with_highlights(highlights: impl IntoIterator<Item=(Range<usize>, HighlightStyle)>) -> Self;
.with_default_highlights(default_style: &TextStyle, highlights: impl IntoIterator<Item=(Range<usize>, HighlightStyle)>) -> Self;
TextRun { len: usize, font: Font, color: Hsla, background_color: Option<Hsla>,
          underline: Option<UnderlineStyle>, strikethrough: Option<StrikethroughStyle> }
HighlightStyle { color: Option<Hsla>, font_weight: Option<FontWeight>,
    font_style: Option<FontStyle>, background_color: Option<Hsla>,
    underline: Option<UnderlineStyle>, strikethrough: Option<StrikethroughStyle>, fade_out: Option<f32> }
```

- Lengths/ranges are **UTF-8 byte offsets**, on character boundaries. Runs must cover the whole string
  exactly; `with_runs` asserts/panics otherwise. Highlight ranges must be ordered, non-overlapping:
  `compute_runs` walks forward and does not merge overlapping syntax and changed-word spans.
  Split at the union of both sets of boundaries and emit one combined style per interval.
  This draws word backgrounds inside text without creating one div per token.
- `StyledText` inherits text style from its containing div. E.g.
  `div().font_family(cx.theme().mono_font_family.clone()).text_size(px(13.)).child(styled)`.
  Theme's Linux mono default is DejaVu Sans Mono, with installed-font fallback; explicitly chosen
  missing families do not get that policy ([C/theme/mod.rs:129](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/theme/mod.rs:129)).
- `window.text_system().shape_line(text: SharedString, font_size: Pixels, runs: &[TextRun],
  force_width: Option<Pixels>) -> ShapedLine` ([G/text_system.rs:638](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/text_system.rs:638));
  `.width() -> Pixels` on ShapedLine ([G/text_system/line.rs:88](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/text_system/line.rs:88)).
  Single line only (no newline). `shape_text(text, font_size, runs, wrap_width: Option<Pixels>,
  line_clamp: Option<usize>) -> Result<SmallVec<[WrappedLine;1]>>` handles multiple/wrapped lines (:750).
  Measure with the same font/runs/size used to paint. `StyledText::layout()` exposes TextLayout for
  index↔pixel mapping after layout. Cache measurements by font/theme/zoom/width.
- `.whitespace_nowrap()` vs `.whitespace_normal()` ([G/styled.rs:75](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/styled.rs:75)). The default wrapper is
  **word-boundary based with character fallback for oversized words**, not unconditional break-anywhere
  ([G/text_system/line_wrapper.rs:40](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/text_system/line_wrapper.rs:40), :107). No `word_break: break-all` switch was found in this version's
  Styled/TextStyle APIs. For SPEC's exact any-character wrap, pre-split display lines using measured
  character/cell widths and render those segments (preserve UTF-8/grapheme boundaries and run offsets).
- Plain StyledText/TextStyle has no configurable tab stop. Do not expect a literal tab glyph to implement
  source-file indentation. Expand tabs to the next display-column stop before shaping, keeping a mapping
  to original byte offsets. Editor has tab support (`EditorState::tab_size(TabSize)`), a separate path.

## d. Text editing, Enter/Tab/focus/IME

[C/input/mod.rs:19](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/input/mod.rs:19) re-exports Base's editing states; these are **three distinct mode types**, not the old
`InputState::multi_line` API:

```rust
let state = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true));
Textarea::new(&state); // Input::new(&Entity<InputState>), Editor::new(&Entity<EditorState>)
```

[B/input/base/state.rs:8958](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/input/base/state.rs:8958), :9163, :9230 define constructors
`new(window: &mut Window, cx: &mut Context<Self>) -> Self` for each mode.
Textarea: `rows(usize)`, `auto_grow(min_rows,max_rows)`, `set_rows(...,cx)`, `set_auto_grow(...,cx)`;
multiline states: `set_soft_wrap(bool, window, cx)`; Editor: `language(impl Into<SharedString>)`,
line numbers, syntax/LSP, indentation, search; defaults soft tabs=2 and soft wrap.
Editor docs describe ~50K-line capacity; this is not a measured WispyDiff performance guarantee.
Tree-sitter and each language grammar are opt-in Kit features; no reason to duplicate core highlighting
for read-only diff rows. `EditorState` without a highlighter is usable for plain code.

Common state methods ([B/input/base/state.rs:897](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/input/base/state.rs:897), :954, :973, :1197, :1255):

```rust
value(&self) -> SharedString;          // snapshots the Rope; avoid each render for large buffers
text(&self) -> &Rope;
set_value(&mut self, value: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>);
replace_all(&mut self, text: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>);
insert(&mut self, text: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>);
replace(&mut self, text: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>);
focus(&self, window: &mut Window, cx: &mut Context<Self>);
```

- Call through `state.update(cx, |state,cx| ...)`; Focusable also exposes the FocusHandle.
  `set_value` resets undo, selection, scroll, and suppresses Change events. `replace_all` preserves undo.
  `insert` inserts at caret without replacing a selection; `replace` replaces the selected range.
- `InputEvent::{Change, PressEnter { secondary: bool, shift: bool }, Focus, Blur}`
  ([B/input/base/state.rs:122](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/input/base/state.rs:122)). Subscribe using `cx.subscribe(&state, |this,state,event,cx| ...)`
  or `subscribe_in` when needing Window; retain subscription.
  Enter is bound in context `Input` to `Enter { secondary:false,shift:false }`, Shift+Enter to
  `{false,true}`, secondary+Enter to `{true,false}` (:151).
- `submit_on_enter(true)` / `set_submit_on_enter(bool,cx)` implements Enter-submit, Shift-newline
  (:1100, :1887). **When false, even secondary+Enter inserts a newline before emitting PressEnter**:
  `insert_newline = is_multi_line && (!submit_on_enter || action.shift)` ignores secondary.
  For code/suggestion regions that must insert normal Enter but submit secondary+Enter, capture
  `input::Enter` above the editor; handle only secondary and `cx.stop_propagation()`, otherwise
  `cx.propagate()`. Do not submit on every PressEnter: it also fires after newline insertion.
- Override Tab with ancestor `.capture_action(|action: &input::IndentInline, window,cx| ...)`;
  likewise OutdentInline. Normal `.on_action` bubbles after the editor consumed it, so is too late.
  Alternatively bind a custom action in a more specific application+Input context.
  Editor `tab_size(TabSize { tab_size: usize, hard_tabs: bool })` / `set_tab_size(TabSize,cx)` ([B/input/editor/indent.rs:495](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/input/editor/indent.rs:495)).
  `WindowExt::focused_input(cx) -> Option<AnyInputState>` covers all concrete editors, [C/window_ext.rs:101](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/window_ext.rs:101).
- IME is implemented through `EntityInputHandler` on the generic state
  ([B/input/base/state.rs:3667](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/input/base/state.rs:3667), `unmark_text`, `replace_text_in_range`,
  `replace_and_mark_text_in_range`) and `window.handle_input` during paint
  ([B/input/base/element.rs:2226](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/input/base/element.rs:2226)). UTF-16 IME ranges are converted by the engine;
  do not replace text entry with raw key-to-character handling.
- Each Input/Textarea/Editor has
  `on_paste(handler: impl Fn(&ClipboardItem,&mut Window,&mut App)->bool + 'static) -> Self`
  ([C/input/input.rs:355](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/input/input.rs:355), [C/input/textarea.rs:123](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/input/textarea.rs:123), [C/input/editor.rs:119](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/input/editor.rs:119)).
  Return true to consume an image paste and insert attachment Markdown yourself.

## e. Markdown / HTML

[C/text/compat.rs:46](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/text/compat.rs:46), :56, :67, :136 wraps Base rich text:

```rust
TextView::new(&Entity<TextViewState>) -> Self;
TextView::markdown(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self;
TextView::html(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self;
.on_link_click(f: impl Fn(&SharedString,&ClickEvent,&mut Window,&mut App) + Send + Sync + 'static);
```

- `.selectable(bool)`, `.scrollable(bool)`, `.selection_format(SelectionFormat)`, `.max_lines(usize)`.
  For streaming, retain `cx.new(|cx| TextViewState::markdown("",cx))`, then
  `set_text(&str,cx)` or `push_str(&str,cx)` ([B/text/state.rs:144](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/text/state.rs:144), :309, :343);
  `.stream_fade(true)` is optional. Parsing/update tasks are already background/foreground-channel based.
  Markdown extension/plugin hooks and custom block/inline rendering are exposed in [C/text/compat.rs:143](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/text/compat.rs:143).
- HTML is parsed by html5ever into the library's own AST; **not a webview, CSS engine or GitHub renderer**.
  [B/text/format/html.rs:338](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/text/format/html.rs:338)–643 explicitly implements headings, paragraphs/containers, br,
  ordered/unordered lists, list items, tables, blockquotes, images, strong/bold, em/italic,
  del/s, u, inline code, links and mark background. Unknown inline tags recurse into children.
  `style`/`script` blocks discarded; CSS class styling/layout and scripts are not executed.
- **details/summary are only generic block containers** (`BLOCK_ELEMENTS`, :18; generic case :594).
  No disclosure node/state/toggle; body always rendered. Interactive GitHub `<details>` requires an
  application conversion/custom component. `<hr>` has no dedicated rule and produces no visible rule; `<pre>` is a generic block here, not the Markdown
  fenced-code renderer; do not promise GitHub code-block fidelity from bodyHTML.
- `<img src alt title width height>` supported; inline style width/height supports px and percentages
  (:183, :386). Clickable linked images preserve their link. Markdown has image nodes too.
  Remote images require a real HTTP client: `Application::with_http_client(Arc<dyn HttpClient>)`
  ([G/app.rs:215](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app.rs:215)); default application starts with **NullHttpClient** (:179), so the minimal scaffold
  will not fetch remote images by itself. Local `wispy-image:` references need application resolution
  or a custom inline/block renderer; they are not a built-in attachment protocol.
- Link clicks default to opening the URL; `.on_link_click` replaces that behavior, suitable for
  intercepting repo file references and calling `cx.open_url(url)` for external links.
  Comment hard line breaks and suggestion fences need GitHub-specific formatting/extension decisions.

## f. Keyboard and mouse

- Define `gpui_kit::actions!(wispy, [OpenPalette]);` (Kit's macro fixes facade crate paths).
  `cx.bind_keys([KeyBinding::new("secondary-k", OpenPalette, Some("WispyDiff"))]);`
  focusable ancestor: `.key_context("WispyDiff").track_focus(&focus_handle)
  .on_action(cx.listener(Self::open_palette))`. KeyContext supports identifiers/key=value and
  predicate expressions (G/keymap/context.rs, G/keymap/binding.rs); scope shortcuts to avoid Input conflicts.
- `secondary` means **cmd on macOS / ctrl elsewhere**, including Linux; raw check
  `event.keystroke.modifiers.secondary()` ([G/platform/keystroke.rs:118](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/platform/keystroke.rs:118), :483).
  `cmd`/`platform` modifier on Linux represents Super: do not bind the app's portable shortcuts to it.
- `InteractiveElement::on_key_down`, `capture_key_down`, `on_key_up`, `on_modifiers_changed`
  take `Fn(&Event,&mut Window,&mut App)+'static` ([G/elements/div.rs:1127](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/elements/div.rs:1127), :1139, :1176).
  KeyDownEvent carries `keystroke`, `is_held`, `prefer_character_input`; use repeated key down for
  held navigation, honor `keystroke.is_ime_in_progress()`. `capture_action` receives capture phase;
  `.on_action` receives bubble phase. Handled actions stop by default; `cx.propagate()` forwards them.
- **Double Shift**: track false→true transitions of `ModifiersChangedEvent.modifiers.shift`,
  require an intervening release, compare `background_executor().now()` timestamps; cancel on any
  non-modifier key/other modifier. Pure Shift isn't reliably a normal KeyDown key across backends.
  No built-in double-shift gesture was found. This is an implementation recipe, not an existing API.
- Mouse navigation: `.on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), handler)`
  and Forward ([G/interactive.rs:453](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/interactive.rs:453)). Native backends map their extra buttons; no browser history exists.
- Cmd/Ctrl+scroll: ScrollWheelEvent has `delta: ScrollDelta`, `modifiers`, `touch_phase`
  ([G/interactive.rs:522](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/interactive.rs:522)). Check `.secondary()`, convert line vs pixel deltas, update app zoom and
  stop propagation before the list scroll handler. For guaranteed capture use a small custom wrapper
  registering `window.on_mouse_event::<ScrollWheelEvent>(..., DispatchPhase, ...)` **during paint**
  ([G/window.rs:5254](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/window.rs:5254)), rather than registering low-level handlers during Render.

## g. Clipboard and URLs

[G/app.rs:1518](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app.rs:1518); [G/platform.rs:2693](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/platform.rs:2693), :2777, :3046:

```rust
App::read_from_clipboard(&self) -> Option<ClipboardItem>;
App::read_from_clipboard_async(&self) -> Task<Result<Option<ClipboardItem>,ClipboardReadError>>;
ClipboardItem::entries(&self) -> &[ClipboardEntry];
ClipboardEntry::{String(ClipboardString), Image(Image), ExternalPaths(ExternalPaths)};
Image::format(&self) -> ImageFormat;
Image::bytes(&self) -> &[u8];
App::open_url(&self, url: &str);
```

Read each image's **encoded format + bytes**, not RenderImage BGRA pixels. Accept/check PNG/JPEG/GIF/WebP,
size and local persistence in the app. Linux X11 offers those MIME types ([L/linux/x11/clipboard.rs:83](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-linux-0.3.6/src/linux/x11/clipboard.rs:83));
Wayland returns Image entries ([L/linux/wayland/clipboard.rs:133](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-linux-0.3.6/src/linux/wayland/clipboard.rs:133)). The control paste hook avoids losing an
image when converting clipboard to text. Desktop sync clipboard reads may block on another app;
X11 clipboard has a 4s timeout (:104), Wayland uses PIPE_READ_TIMEOUT. Avoid polling clipboard per frame.

## h. Background/async work and streams

[G/gpui.rs:237](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/gpui.rs:237), [G/app.rs:2036](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app.rs:2036), [G/app/context.rs:230](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app/context.rs:230), [G/app/entity_map.rs:799](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app/entity_map.rs:799):

```rust
cx.background_spawn(future: impl Future<Output=R> + Send + 'static) -> Task<R>; // R: Send+'static
App::spawn<AsyncFn,R>(&self, f: AsyncFn) -> Task<R>;
// AsyncFn: AsyncFnOnce(&mut AsyncApp)->R+'static
Context<T>::spawn<AsyncFn,R>(&self, f: AsyncFn) -> Task<R>;
// AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp)->R+'static
WeakEntity<T>::update<C: AppContext,R>(&self, cx: &mut C,
    update: impl FnOnce(&mut T,&mut Context<T>)->R) -> Result<R>;
```

```rust
let work = cx.background_spawn(async move { /* CPU/blocking work; owned Send data */ 42 });
cx.spawn(async move |weak, cx| {
    let result = work.await;
    let _ = weak.update(cx, |this, cx| { this.result = result; cx.notify(); });
}).detach();
```

- `spawn` is on the **UI thread**; an async block does not make CPU work background.
  Only Send data crosses worker boundary; Entity/App/Window remain UI-owned.
  Keep Task in view state for cancellation-on-drop or `.detach()` for intentional independent work;
  dropping a task cancels it. WeakEntity updates may fail normally when a view closes.
- `cx.background_executor().timer(Duration) -> Task<()>` and `.now()` are testable timers
  ([G/executor.rs:175](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/executor.rs:175), :183). `cx.spawn_in(window, async move |weak,cx| ...)` gives AsyncWindow,
  whose `.update(|window,cx|...) -> Result<R>` is window-safe ([G/app/context.rs:672](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app/context.rs:672),
  [G/app/async_context.rs:309](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/app/async_context.rs:309)). Don't hold mutable entity borrows across await.
- Streaming: `gpui_kit::base::async_util::{unbounded,Sender,Receiver}` re-exports smol channels
  on native ([B/async_util.rs:4](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/async_util.rs:4)); background producer, foreground receiver loop, batch tokens/results
  then one `weak.update`/`cx.notify` per chunk. Use a bounded async channel/backpressure for large
  streams if needed (unbounded is not a memory budget). TextViewState already uses this pattern
  ([B/text/state.rs:158](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/text/state.rs:158)). GPUI executor is not a Tokio reactor: core code using Tokio IO/timers must
  run on a retained Tokio runtime, then bridge results via channels; merely polling it in GPUI spawn
  does not supply a Tokio runtime. No core runtime changes made by this scaffold.

## i. Components / layout inventory

All paths below are available through `gpui_kit::component` unless stated otherwise.

| Need | Exact entry point / source |
|---|---|
| Drag split panes | `h_resizable(id)` / `v_resizable(id) -> ResizablePanelGroup`, `resizable_panel() -> ResizablePanel`; `.with_state(&Entity<ResizableState>)`, `.child(panel)`. Panel `.size(Pixels)`, `.size_range(Range<Pixels>)`; [B/resizable/mod.rs:17](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/resizable/mod.rs:17), [B/resizable/panel.rs:67](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/resizable/panel.rs:67), :283. |
| Persist / reset pane sizes | Group `.on_resize(Fn(&Entity<ResizableState>, &mut Window, &mut App))`; `state.sizes() -> &Vec<Pixels>`, `state.resize_panel(ix,size,window,cx)` emits Resized. Default minimum 100px. Built-in resize handle has no double-click reset; attach custom handle UI/appearance with click_count logic, call resize_panel to restore saved defaults. [B/resizable/mod.rs:56](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/resizable/mod.rs:56), :69; [B/resizable/resize_handle.rs:149](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/resizable/resize_handle.rs:149). |
| Sidebar | C/sidebar/mod.rs has styled Sidebar; split pane primitive is enough for a file list. DockArea/Panel APIs exist for richer docking, not needed for fixed sidebars. |
| Modal / sheets | `WindowExt::open_dialog<F>(&mut self,cx:&mut App,build:F)` where `F: Fn(Dialog,&mut Window,&mut App)->Dialog+'static`; `open_sheet`, `open_sheet_at(Placement,...)`, close counterparts. Render Root layers explicitly. [C/window_ext.rs:13](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/window_ext.rs:13). |
| Popover | `Popover::new(id).trigger(element).content(F).anchor(...)`; content `F: Fn(&mut Window,&mut App)->E`, `E: IntoElement`; `.open(bool)`, `.on_open_change(...)`. [C/popover.rs:132](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/popover.rs:132), :229. |
| Tooltip | `Tooltip::new(text).build(window,cx) -> AnyView`, use element `.tooltip(|window,cx|...)`; richer `Tooltip::element(builder)` and action/key labels. Root installs managed tooltip overlay. [C/tooltip.rs:43](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/tooltip.rs:43), :81. |
| Dropdown | `menu::DropdownMenu` extension `.dropdown_menu(...)`; `PopupMenu`, `Select::new(&Entity<SelectState<D>>)` for selection/search (delegate-backed). C/menu/dropdown_menu.rs; [C/select.rs:149](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/select.rs:149), :642. |
| Checkbox | `Checkbox::new(id).label(...).checked(bool).on_change(Fn(&bool,&mut Window,&mut App))`; caller persists checked state. [C/checkbox.rs:86](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/checkbox.rs:86), :102. |
| Tree | `TreeState::new(cx:&mut App)`, `.items(Vec<TreeItem>)`, `TreeItem::new(id,label).children(...).expanded(bool)`; `tree(&Entity<TreeState>, render_item)` in Component (`render_item: Fn(usize,&TreeEntry,bool,&mut Window,&mut App)->ListItem`); flat entries use uniform virtualization; state `.scroll_to_item(ix,ScrollStrategy)`, `.set_selected_index`, `.set_selected_item`. [B/tree.rs:99](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/tree.rs:99), :197, :260; [C/tree.rs:18](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/tree.rs:18). Merge directory chains/auto-fold-on-viewed are application policy. |
| Toast | `window.push_notification(Notification::success("Saved"),cx)`; Notification has `.id::<T>()`, `.autohide(bool)`, `.on_click`, `.on_close`, `.action`, optional system delivery. [C/notification.rs:203](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/notification.rs:203); [C/window_ext.rs:62](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/window_ext.rs:62); render notification layer. |
| Scrollbar | `gpui_kit::base::Scrollbar::vertical(&handle)` / horizontal / new; handles must impl `ScrollbarHandle+Clone` (ScrollHandle, UniformListScrollHandle, VirtualListScrollHandle, ListState supported). Position as overlay over scroll area; `ScrollbarMode::{Scrolling, Hover, Always}`, theme sync controls Base look. [B/scrollbar.rs:836](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6/src/scrollbar.rs:836); C/scroll/mod.rs re-exports. |

## j. Zoom / UI scale

- `window.set_rem_size(impl Into<Pixels>)`, `window.rem_size() -> Pixels` ([G/window.rs:2944](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/window.rs:2944)).
  **Root sets it from `cx.theme().font_size` every render**, [C/root.rs:582](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6/src/root.rs:582). Mutate theme font_size
  (with baseline+zoom), sync Base, repaint; setting rem once is overwritten on next render.
- This scales **rem-based** sizes only. Fixed Pixels (13px mono text, persisted sidebar widths,
  border sizes, precomputed row heights, some component constants) do not automatically scale.
  To meet whole-window zoom, keep explicit app scale and apply it to font sizes, pixel geometry,
  row metrics and scroll anchors. Recompute virtual sizes when font/wrap width changes.
- `window.scale_factor()` is device DPI; `set_scale_factor` exists **only under test-support/test**
  ([G/window.rs:2937](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/window.rs:2937)), not a production zoom API. `zoom_window()` is native window maximize/restore.
  No production universal affine/CSS zoom API found. Keyboard/secondary-wheel zoom is app policy.

## k. Linux observations / pitfalls

- Kit enables **both Wayland and X11**. `guess_compositor()` ([G/platform.rs:158](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6/src/platform.rs:158)) chooses nonempty
  WAYLAND_DISPLAY first, DISPLAY next, Headless otherwise; ZED_HEADLESS forces headless.
  [L/linux.rs:40](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-linux-0.3.6/src/linux.rs:40) directly constructs the chosen backend; there is no automatic connection-failure
  fallback to X11. For an explicit X11 diagnostic run, unset WAYLAND_DISPLAY and retain DISPLAY.
- Rendering uses gpui-pre-wgpu 0.3.6 / wgpu 29.0.4 with runtime shaders, not the older GPUI Blade
  instructions found online. Vulkan runtime/driver is needed for the Vulkan backend; no Vulkan SDK
  pkg-config requirement was encountered merely because `vulkan.pc` is absent.
- Wayland text-input-v3 manager is optional (`globals.bind(...).ok()`, [L/linux/wayland/client.rs:271](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-linux-0.3.6/src/linux/wayland/client.rs:271));
  IME behavior depends on compositor support. X11 uses its IME path; test non-Latin composition on both.
  Implemented support is source evidence, not proof every compositor/IME combination works.
- X11 source retains `TODO: Use scale factor from XSettings here` ([L/linux/x11/client.rs:2616](/home/tim/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-linux-0.3.6/src/linux/x11/client.rs:2616)).
  Validate fractional scaling, monitor moves and actual fonts on the target desktop.
- Root defaults to a Linux client-side border (`bordered(false)` opts out); native macOS appearance
  override is not a Linux theme switch. Linux native menus use a fallback overlay in this Component.
- Clipboard access can stall on another process; remote images need a configured HTTP client;
  installed mono font/byte-vs-cell mappings, programmatic scrolling, HTML disclosures and complete
  pixel scaling all require explicit application work. No claim yet that this scaffold meets SPEC
  performance budgets; no application rendering/core behavior was changed.
