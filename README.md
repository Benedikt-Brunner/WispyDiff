<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/wordmark-light.svg">
    <img src="assets/logo/wordmark-dark.svg" alt="WispyDiff" height="64">
  </picture>
</p>

A distraction-free, keyboard-first desktop app (macOS and Linux) for reviewing **stacked GitHub PRs**.

- Opens a whole stack of PRs and lets you review any contiguous range as one combined diff,
  with every line attributed to the PR that last touched it.
- Unified and full-file side-by-side views, syntax highlighting, usages lookup and repo-wide
  search, all computed locally in Rust and cached per commit, so switching ranges is instant.
- Draft comments offline and submit a review per PR; ask Claude Code or Codex about the code
  you're looking at.

It's a personal tool: unsigned, no telemetry. [SPEC.md](SPEC.md) describes the behaviour and
performance budgets in detail. Shortcuts are written with ⌘; on Linux use Ctrl instead.

## Requirements

- macOS or Linux, [Rust](https://rustup.rs), [Node](https://nodejs.org) with
  [pnpm](https://pnpm.io), and [just](https://github.com/casey/just)
- Git 2.44 or newer (older versions break repo search while offline)
- Linux only: the [Tauri system dependencies](https://v2.tauri.app/start/prerequisites/#linux)
  (WebKitGTK 4.1). On Debian/Ubuntu:
  ```sh
  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev \
    libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```
  Ubuntu 24.04 ships Git 2.43; get a newer one from the
  [git-core PPA](https://launchpad.net/~git-core/+archive/ubuntu/ppa).
- The [GitHub CLI](https://cli.github.com), signed in (`gh auth login`); WispyDiff reads its
  token via `gh auth token`
- Optional: the `claude` or `codex` CLI on your `PATH` for the assistant panel

## Getting started

```sh
pnpm install
just dev     # run with hot reload
just build   # macOS: target/release/bundle/macos/WispyDiff.app
             # Linux: target/release/bundle/{deb,appimage}/
```

WispyDiff keeps its own blobless clones under `~/Library/Application Support/WispyDiff` (macOS)
or `~/.local/share/WispyDiff` (Linux) and never touches your working copies.

## Development

```sh
just test    # core tests, type-check, functional e2e suites
just bench   # performance budgets from SPEC.md
```

There is no CI; everything runs locally. Tests use throwaway git repos and a fake GitHub API
(`crates/wispy-fixtures`), never the real GitHub, Claude Code or Codex.

## License

[MIT](LICENSE)
