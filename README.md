<p align="center"><img src="assets/logo/wordmark.svg" alt="WispyDiff" height="64"></p>

A distraction-free, keyboard-first macOS app for reviewing **stacked GitHub PRs**.

- Opens a whole stack of PRs and lets you review any contiguous range as one combined diff,
  with every line attributed to the PR that last touched it.
- Unified and full-file side-by-side views, syntax highlighting, usages lookup and repo-wide
  search, all computed locally in Rust and cached per commit, so switching ranges is instant.
- Draft comments offline and submit a review per PR; ask Claude Code or Codex about the code
  you're looking at.

It's a personal tool: macOS only, unsigned, no telemetry. [SPEC.md](SPEC.md) describes the
behaviour and performance budgets in detail.

## Requirements

- macOS, [Rust](https://rustup.rs), [Node](https://nodejs.org) with [pnpm](https://pnpm.io),
  and [just](https://github.com/casey/just)
- The [GitHub CLI](https://cli.github.com), signed in (`gh auth login`); WispyDiff reads its
  token via `gh auth token`
- Optional: the `claude` or `codex` CLI on your `PATH` for the assistant panel

## Getting started

```sh
pnpm install
just dev     # run with hot reload
just build   # target/release/bundle/macos/WispyDiff.app
```

WispyDiff keeps its own blobless clones under `~/Library/Application Support/WispyDiff` and
never touches your working copies.

## Development

```sh
just test    # core tests, type-check, functional e2e suites
just bench   # performance budgets from SPEC.md
```

There is no CI; everything runs locally. Tests use throwaway git repos and a fake GitHub API
(`crates/wispy-fixtures`), never the real GitHub, Claude Code or Codex.

## License

[MIT](LICENSE)
