# WispyDiff — Spec

A distraction-free, keyboard-first macOS desktop app for reviewing **stacked GitHub PRs**, written by humans or agents. Personal use.

## Platform & architecture
- **Tauri 2.** Rust backend (git, GitHub sync, SQLite, tree-sitter, assistant CLIs) + React/TS/Vite frontend.
- Personal, macOS only, unsigned, no telemetry. Nothing hard-coded: CLIs via `PATH`, token via `gh auth token`, data in `~/Library/Application Support/WispyDiff`.
- Any **github.com** repo. The app keeps its own **blobless partial clones** (`--filter=blob:none`), fetches `refs/pull/*/head`, and computes all diffs locally with git. GitHub API is used only for metadata, threads, and posting. The user's own working copies are never touched.

## Performance budget (binding)
| Scenario | Target |
|---|---|
| Open cached stack from inbox | first file visible **< 150 ms** |
| Range diff handled smoothly | **~500 files / 50,000 changed lines** |
| Largest single file (full-file mode) | **~20,000 lines** at 60 fps scroll |
| Switch PR range / toggle unified ↔ side-by-side | **< 100 ms** |
| Usages lookup | **< 50 ms** |
| `git grep` whole repo | streamed, first hits **< 300 ms** |

Diffs, highlighting and symbol indexes are **precomputed per SHA in Rust**, cached in SQLite, and served to the frontend as prepared line models **for the visible window only**.

## Navigation
- **Inbox** home screen: review-requested + authored-by-me PRs, **grouped into stacks**. `Cmd+K` opens any PR by URL or `owner/repo#N`.
- **Stacks** are discovered via `baseRefName` chains. Only the bottom PR targets the default branch; stacks merge bottom-up. Forks never join a stack; if several PRs branch off one head, the oldest is followed. A PR not based on the latest head of the PR below is flagged ⚠.
- **Contiguous range selection only.** Opening a PR shows that PR alone. In the stack bar, click selects one PR, shift-click extends to a range; `[`/`]` move the selection down/up the stack, `{`/`}` extend it. Combined diff = base of lowest selected PR → head of highest.
- Every range of the open stack is **precomputed in the background** (focused PR, then whole stack, then the rest), so switching is a cache read.
- **Per-PR color** used everywhere (gutter bar, file list, chips). Each line is attributed to the **last PR that touched it**; hover shows its history ("added in #2, modified in #3").

## Reading
- Two modes, toggled per file (`s`) with a global default (`S`, remembered): **unified** and **full-file side-by-side** (whole base file vs whole head file). The two sides are aligned line for line — changes sit opposite what they replaced, with hatched filler where one side has more lines — so a single scroll keeps them in sync and no connector bands are needed. Side-by-side rows always span the viewport 50/50; horizontal scrolling moves the code in both halves together. No hunk-only split view.
- Collapsed by default: lockfiles, minified files and source maps, snapshots, `linguist-generated` (from the head's `.gitattributes`). `e` collapses/expands the current file. "Hide whitespace changes" (`w`, default off) recomputes the range with `-w`. Per-repo local ignore list via the palette (`> ignore <glob>`, `> stop ignoring …`; gitignore-style globs).
- `⌘K` palette: PR URLs / `owner/repo#N`, recent PRs, and `>` commands.
- Keyboard-first: `j/k` hunks, `n/p` files, `[ ] { }` stack range, `s`/`S` side-by-side (file/all), `e` collapse, `w` whitespace, `⌘K` palette, `⌘B` file list, `c` comment, `v` viewed, `a` assistant.

## Offline
- **Every inbox stack is prefetched** in the background (interval + on window focus): git objects, full base/head files, threads, metadata. Evicted after merge/close unless unposted drafts exist.

## Progress tracking
- **Viewed marks** per file, content-keyed (survive rebases).
- **Local checkpoints**: "Mark as reviewed" records every PR head SHA in the range, local only, never touches GitHub. Submitting a review also creates one. Checkpoint history is browsable.
- **"Only changes since checkpoint"**: rebase-aware interdiff (range-diff style) so rebase-only movement is filtered out.

## Code intelligence
- **Name-based tree-sitter** symbol matching over **full contents of touched files** at the range head. Usages in changed lines are shown separately from unchanged ones. Jump in-tool; files outside the diff open in a read-only file view.
- **Streamed `git grep`** fallback over the whole repo.
- Languages: PHP, TS/JS, Vue SFC, Twig (+ highlight-only JSON, YAML, SQL).

## Comments
- **Stored locally**, anchored to the reviewed SHA. Kinds: single line, multi-line range, file-level, per-PR summary.
- **Auto-routed to the PR that introduced the line.**
- Comments on lines GitHub won't accept, and outdated comments that can't be re-anchored, are posted as **file-level comments with permalink + quoted snippet** (badge shown while writing).
- Outdated handling: when the head moved, re-map to the new head if the line still exists unchanged; otherwise flag "outdated — re-anchor or post as file comment".
- Existing GitHub threads shown inline and cached; **offline replies and resolves** are queued.
- **Submit sheet**: per PR choose verdict (Comment / Approve / Request changes, default Comment) + optional summary, then **Submit all** creates one GitHub review per PR. Failures keep drafts local with the error; nothing is double-posted.

## Assistant
- `a` / button opens a prompt; context is **the selected range or exactly the selected lines** (+ path + PR label).
- **Claude Code** (`claude -p --output-format stream-json --model --effort`) or **Codex** (`codex exec --json -m -c model_reasoning_effort=…`). Selection remembered globally, overridable per question.
- Runs in a **read-only worktree** at the range head.
- **Side-panel resumable threads** with gutter markers for selection threads, local history per PR, "turn answer into draft comment". Offline: read-only.

## Testing
- **Rust core**: tests against real throwaway git repos and a fake local GitHub HTTP server. Services under test are constructed directly in each test, never via factory methods.
- **Functional e2e** smoke tests and **performance e2e** tests via **WebdriverIO against the real Tauri build** (the service's embedded WebDriver server supports macOS), asserting each budget above via `performance.mark` + rAF frame timing.
- Fixtures: **seeded synthetic repo** (4-PR stack, 500 files / 50k changed lines across PHP/TS/Vue/Twig, one 20k-line file), plus an optional local worst-case PR that is never committed.
- **Local only** (`just test`, `just bench`), no CI.

## Milestones
1. Walking skeleton + performance check (`Cmd+K` → PR URL → blobless clone → unified single-PR diff, virtualized + highlighted)
2. Stacks, range slider, colors, attribution
3. Full-file side-by-side + noise handling
4. Comments, threads, offline drafts, submit sheet
5. Inbox + background prefetch
6. Viewed marks, checkpoints, since-checkpoint interdiffs
7. Code intelligence
8. Assistant
