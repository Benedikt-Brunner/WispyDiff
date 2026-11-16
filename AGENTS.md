# WispyDiff

Personal desktop app (macOS, Linux) for reviewing stacked GitHub PRs. **SPEC.md is the source of truth** for
behaviour, performance budgets and milestones — read it before changing anything user-facing.

## Layout
- `crates/wispy-core` — all logic (git, GitHub, diff parsing, highlighting, row model, SQLite cache). No UI dependency.
- `crates/wispy-fixtures` — seeded synthetic stack generator + fake GitHub API (`generate`, `serve`).
- `crates/wispydiff` — the native app on GPUI (gpui-kit). `backend.rs` runs the core on an owned Tokio
  runtime (async calls return futures the UI awaits); `session.rs` holds what's being reviewed; `app.rs` is
  the window root (custom title bar, overlays, global keys); `diff/` is the diff viewer (rows, comments,
  file list, panels); `logic/` holds pure helpers. GPUI API notes: `GPUI-NOTES.md`, `TESTING-NOTES.md`.
- `testing/fake-cli` — fake `claude` / `codex` CLIs and browser opener for tests.

## Commands
- `just test` — core tests and the app's headless UI tests.
- `just bench` — performance budgets on this machine (release build, real window — keep it visible). Run it before merging anything touching rendering or the core.
- `just dev` / `just fast` — run the app (`fast`: optimized, quick to link).
- `just install` / `just auto-update [enable|disable|status]` — build `main` into the local installation, once or whenever it moves (`scripts/auto-update.sh`; builds in its own clone, never this checkout).

## Rules
- Performance budgets in SPEC.md are binding; `wispydiff --perf` (`just bench`) enforces them. Heavy work happens in Rust, precomputed per SHA and cached; the UI reads the in-memory row model and only renders visible rows (GPUI `list` / `uniform_list` — never render a whole long list).
- Core tests use real throwaway git repos (`tests/support`) and wiremock for GitHub. Construct services under test directly in each test — never through factory helpers (fixture builders are fine).
- UI tests (`crates/wispydiff/tests/ui_*.rs`) mount the real app root headless (`#[gpui_kit::test]`) against a fixture server per test (`Backend::with_github`), and drive it with real keystrokes and clicks. Assert on app state; read-only accessors for them live in `src/diff/testing_*.rs`.
- Shortcuts are GPUI key bindings with `secondary-` (⌘ on macOS, Ctrl on Linux), never `cmd-`/`ctrl-` directly; labels use `ui::key_label`. Single-letter keys bind in a `… && !Input` context so they never fire while typing.
- gpui-kit's text inputs bind Enter, Tab, Escape, ↑/↓ themselves: a container that needs one first uses `capture_action` and must call `cx.stop_propagation()` (capture handlers don't stop on their own).
- Sizes are CSS pixels at 100% zoom via `ui::u(px)` (rems): zoom scales the rem size, so the whole window scales. Colours come from `ui::palette(cx)` (themes in `theme.rs`).
- git always runs through `wispy_core::git::Git` (isolated from the user's global git config).
- Never touch the user's own working copies; the app only uses its blobless clones under the data dir.
- Env overrides (tests only): `WISPY_DATA_DIR`, `WISPY_GITHUB_API`, `WISPY_GITHUB_TOKEN`, `WISPY_PREFETCH_INTERVAL`, `WISPY_CLAUDE_BIN` / `WISPY_CODEX_BIN` (+ `WISPY_FAKE_LOG`), `WISPY_OPENER` (browser opener). Debugging: `WISPY_DEBUG_TIMING` prints timing marks, `WISPY_SCRIPT` replays keystrokes/clicks inside the app (see `script.rs`).
- Tests never call the real Claude Code / Codex CLIs: `testing/fake-cli/{claude,codex}` speak their JSON event formats, call the app's review tools (MCP, `crates/wispy-core/src/mcp.rs`) at the URL in their args, and log how they were invoked and the tool calls.
- git writes (repo setup, fetches) are serialized per repo in `RepoStore`; reads stay concurrent.
