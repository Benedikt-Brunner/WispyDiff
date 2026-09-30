# WispyDiff

Personal macOS app for reviewing stacked GitHub PRs. **SPEC.md is the source of truth** for
behaviour, performance budgets and milestones — read it before changing anything user-facing.

## Layout
- `crates/wispy-core` — all logic (git, GitHub, diff parsing, highlighting, row model, SQLite cache). No Tauri dependency.
- `crates/wispy-fixtures` — seeded synthetic stack generator + fake GitHub API (`generate`, `serve`).
- `src-tauri` — thin Tauri shell: commands in `commands.rs`, state in `state.rs`. Feature `e2e` embeds the WebDriver server.
- `src` — React/TS frontend. The owner doesn't read this code; keep it simple and fast.
- `e2e` — WebdriverIO suites against the real release build (`open.e2e.ts` functional, `perf.e2e.ts` budgets).

## Commands
- `just test` — core tests, type-check, all functional e2e specs.
- `just bench` — performance budgets on this machine. Run it before merging anything touching rendering or the core.
- `just dev` — run the app.

## Rules
- Performance budgets in SPEC.md are binding; `perf.e2e.ts` enforces them. Heavy work happens in Rust, precomputed per SHA and cached; the frontend only renders visible rows.
- Core tests use real throwaway git repos (`tests/support`) and wiremock for GitHub. Construct services under test directly in each test — never through factory helpers (fixture builders are fine).
- git always runs through `wispy_core::git::Git` (isolated from the user's global git config).
- Never touch the user's own working copies; the app only uses its blobless clones under the data dir.
- Env overrides (tests only): `WISPY_DATA_DIR`, `WISPY_GITHUB_API`, `WISPY_GITHUB_TOKEN`, `WISPY_PREFETCH_INTERVAL`.
- e2e: spec files share one app process (don't assume a fresh UI); the embedded macOS driver is slow/lossy for element lookups and typed spaces, so helpers go through `browser.execute`. Failing tests print frontend errors (`[failure] …`).
- git writes (repo setup, fetches) are serialized per repo in `RepoStore`; reads stay concurrent.
