# WispyDiff — Spec

A distraction-free, keyboard-first desktop app (macOS, Linux) for reviewing **stacked GitHub PRs**, written by humans or agents. Personal use.

## Platform & architecture
- **Tauri 2.** Rust backend (git, GitHub sync, SQLite, tree-sitter, assistant CLIs) + React/TS/Vite frontend.
- Personal, macOS and Linux, unsigned, no telemetry. Nothing hard-coded: CLIs via `PATH`, token via `gh auth token`, data in `~/Library/Application Support/WispyDiff` (Linux: `~/.local/share/WispyDiff`).
- Shortcuts are written with `⌘`; on Linux it is `Ctrl` (Super belongs to the desktop), and the UI labels say so.
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
- **Inbox** home screen (shown at launch; `⌘I` or the logo returns to it): review-requested + authored-by-me PRs (GitHub search), **grouped into stacks** (a PR whose base is another inbox PR's head continues its chain). `j`/`k` + Enter to open, `r` to refresh. Each group shows whether it's "offline ready" and its unposted drafts. `Cmd+K` opens any PR by URL or `owner/repo#N`.
- **Stacks** are discovered via `baseRefName` chains. Only the bottom PR targets the default branch; stacks merge bottom-up. Forks never join a stack; if several PRs branch off one head, the oldest is followed. A PR not based on the latest head of the PR below is flagged ⚠.
- **Contiguous range selection only.** Opening a PR shows that PR alone. In the stack bar, click selects one PR, shift-click extends to a range; `[`/`]` move the selection down/up the stack, `{`/`}` extend it. Combined diff = base of lowest selected PR → head of highest.
- Every range of the open stack is **precomputed in the background** (focused PR, then whole stack, then the rest), so switching is a cache read.
- **Per-PR color** used everywhere (gutter bar, file list, chips). Each line is attributed to the **last PR that touched it**; hover shows its history ("added in #2, modified in #3").

## Reading
- Two modes, toggled per file (`s`) with a global default (`S`, remembered): **unified** and **full-file side-by-side** (whole base file vs whole head file). The two sides are aligned line for line — changes sit opposite what they replaced, with hatched filler where one side has more lines — so a single scroll keeps them in sync and no connector bands are needed. Side-by-side rows always span the viewport 50/50; horizontal scrolling moves the code in both halves together. No hunk-only split view.
- **Line wrap** (`z`, or `> Wrap long lines`; global, remembered, default off) in both modes: long lines break at the window edge (at any character) instead of scrolling sideways. The core reports each row's display width (tabs, wide characters), so the frontend lays out wrapped rows' heights for the whole diff without rendering them, and scrolling stays virtualized.
- Collapsed by default: lockfiles, minified files and source maps, snapshots, `linguist-generated` (from the head's `.gitattributes`). `e` collapses/expands the current file. "Hide whitespace changes" (`w`, default off) recomputes the range with `-w`. Per-repo local ignore list via the palette (`> ignore <glob>`, `> stop ignoring …`; gitignore-style globs).
- `⌘K` palette: PR URLs / `owner/repo#N`, recent PRs, and `>` commands.
- Keyboard-first (`?` or `⌘/` lists every shortcut):
  - Moving: `↑/↓` scroll (held: continuously); `⇧↑/⇧↓` or `n/p` next/previous file (its header goes to the top); `j/k` hunks (the view glides there; holding the key scrolls through the changes and settles on one when released); `⌘B` file list.
  - Stack: `Tab`/`⇧Tab` next/previous PR on its own (wrapping); `[ ]` move the range, `{ }` extend it; `w` whitespace; `M` mark reviewed; `d` since checkpoint.
  - Files: `s`/`S` side-by-side (file/all); `z` line wrap; `Space` or `e` collapse/expand without touching the viewed mark — collapsing moves on to the next file that isn't collapsed; `v` viewed — marking moves on to the next file not yet viewed, so `v v v` ticks off consecutive files. A file reached by `n`/`p`/the file list/`v`/collapsing stays the current file while the view stays put, even when it is one of the last files and can't scroll to the top (so a collapsed file above it never takes over).
  - Review: `c` comment, `⌘`-click / `u` usages, `/` search repo, `a` assistant; `Esc` closes the assistant first, then other panels, then an open comment box.
  - Anywhere: `⌘K` palette, `⌘I` inbox, `⌘T` theme.
- **Themes** (`⌘T`, or `> Change theme…`): System (follows the OS light/dark), Light, Dark, Solarized Light/Dark, Nord, Dracula, Sepia, E-Ink (black on white, syntax by weight instead of colour, no shadows, jumps land instantly instead of gliding). Moving through the list previews; Enter keeps, Esc reverts. Remembered locally; the native window follows the theme's light/dark.
- Scrolling never shows blank rows: rows for a new scroll position render before the frame paints, a screen of rows is drawn beyond each edge, and rows two screens ahead are fetched.

## Offline
- **Every inbox stack is prefetched** in the background (at launch, every 5 minutes, and on window focus): the full stack is discovered and fetched, each inbox PR alone and the whole stack are precomputed (highlighted full files included), and review threads are cached. Stacks of PRs that left the inbox because they were merged/closed are evicted unless they have unposted drafts.
- **New commits on the open PR**: opening a cached stack shows it at once and checks GitHub; each background refresh (launch, every 5 minutes, window focus) checks the open stack again. If any PR in it moved, a "New commits pushed · load latest" button appears in the title bar; nothing changes under the reader until it is clicked (drafts are re-mapped, `d` shows what's new).

## Progress tracking
- **Viewed marks** per file (`v`), keyed by the file's diff content (paths + changed/context text, not line numbers or SHAs), so they survive rebases that don't change the file. Viewed files collapse and get a ✓ in the file list.
- **Local checkpoints**: "Mark as reviewed" (`M`) records every PR head (and diff base) in the range, local only, never touches GitHub. Submitting a review also creates one for that PR. The header's picker lists the checkpoints covering the range.
- **"Only changes since checkpoint"** (`d`): the checkpoint's version of the range is replayed onto the current base (`git merge-tree --merge-base`), and that tree is diffed against the current head — so changes a rebase brought in cancel out and only the author's edits remain. If replaying conflicts, a raw head-to-head diff is shown with a warning.

## Code intelligence
- **Name-based tree-sitter** symbol matching over **full contents of touched files** at the range head (`⌘`-click a name, or `u` for the name under the mouse). The panel lists definitions, uses in changed lines, and uses in unchanged code. Jump in-tool (a line outside the hunks switches its file to side-by-side and flashes); files outside the diff open in a read-only file view. The index is built per view in the background right after it opens.
- **Streamed `git grep`** over the whole repo (`/`, or "Search the whole repo" from usages). The first search of a head downloads that tree's missing blobs in one batch (the clone is blobless); hits stream to the panel as they're found. Prefetching downloads every inbox stack head's files, so search works offline; when blobs can't be downloaded, the files already present are searched and the panel says how many weren't.
- Languages: PHP, TS/JS, Vue SFC (HTML with `<script>`/`<style>` in their languages), Twig (HTML with Twig tags tokenized on top), HTML, CSS (+ highlight-only JSON, YAML, SQL).

## Comments
- **Stored locally**, anchored to the reviewed SHA. Kinds: single line, multi-line range, file-level, per-PR summary.
- **Auto-routed to the PR that introduced the line.**
- Comments on lines GitHub won't accept, and outdated comments that can't be re-anchored, are posted as **file-level comments with permalink + quoted snippet** (badge shown while writing).
- Outdated handling: when the head moved, re-map to the new head if the line still exists unchanged; otherwise flag "outdated — re-anchor or post as file comment".
- Existing GitHub threads shown inline and cached; **offline replies and resolves** are queued.
- **Submit sheet**: per PR choose verdict (Comment / Approve / Request changes, default Comment) + optional summary, then **Submit all** creates one GitHub review per PR — or **Submit #N** (⌘↵ in its summary) sends just that PR, leaving the other PRs' drafts, verdicts and summaries in place. Failures keep drafts local with the error; nothing is double-posted.
- Writing: click a line's gutter (or `c` on the hovered line) for a line comment, drag across gutters for a range, "Comment" on a file header (or `c` with no hovered line) for a file comment. `⌘↵` saves, `Esc` cancels. On new-side lines, "Suggest change" (`⌘G`) inserts a GitHub ```` ```suggestion ```` block holding the commented lines to edit (not offered when it would post as a file comment). Threads show inline with Reply / Resolve (queued until submit); resolved threads collapse.
- Mechanics: line numbers are tracked per PR (the attribution walk records each line's number in its PR's own diff), so a comment on a lower PR's line in a combined view lands on that PR. Line comments go in the review; file comments (GitHub has none inside reviews) and replies are posted individually. Everything sent carries a hidden `<!-- wispydiff:… -->` marker and each draft's status is persisted around its request; after a network failure the next submit looks for the marker on GitHub instead of resending. Submitting first re-fetches the stack, so drafts are planned against the PRs' current heads.

## Assistant
- `a` opens the side panel; context is **the selected range, or exactly the selected lines** (select code with the mouse first) + path + PR label + their line numbers in that PR. The first prompt carries the range's diff (truncated at ~120 KB; the CLI can read the rest from the checkout); follow-ups resume the CLI session and send only the question.
- **Claude Code** (`claude -p --output-format stream-json --model --effort`) or **Codex** (`codex exec --json -m -c model_reasoning_effort=…`). Selection remembered globally, overridable per question.
- Runs in a **read-only worktree** at the range head (a `git worktree` of the app's clone, blobs fetched in one batch first). Read-only is enforced by the CLIs: Claude with only `Read,Grep,Glob` allowed and `--permission-mode dontAsk`; Codex with `sandbox_mode="read-only"`.
- **Side-panel resumable threads** with gutter markers for selection threads (click to reopen), local history per stack, "turn answer into draft comment" (a line draft on the selected lines, or the top PR's review-summary draft for whole-range threads). CLI failures (auth, unavailable model) are shown in the thread. Offline: read-only.

## Testing
- **Rust core**: tests against real throwaway git repos and a fake local GitHub HTTP server. Services under test are constructed directly in each test, never via factory methods.
- **Functional e2e** smoke tests and **performance e2e** tests via **WebdriverIO against the real Tauri build** (the service's embedded WebDriver server supports macOS and Linux/WebKitGTK), asserting each budget above via `performance.mark` + rAF frame timing.
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
