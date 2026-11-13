# WispyDiff — CI runs `just test`; the performance budgets (`just bench`) only run locally.

default:
    @just --list

# Run the app (optimized core, quick builds).
dev *args:
    cargo run -p wispydiff -- {{args}}

# The app as it really feels, without waiting for the full release link (fat LTO).
fast *args:
    cargo run --profile fast -p wispydiff -- {{args}}

# Release build (macOS: also target/release/bundle/macos/WispyDiff.app).
build:
    cargo build --release -p wispydiff
    if [ "$(uname -s)" = Darwin ]; then scripts/bundle-macos.sh; fi

# Build the latest `main` and install it (macOS: /Applications/WispyDiff.app, Linux: ~/.local/bin/wispydiff).
install:
    scripts/auto-update.sh run --force

# Rebuild and reinstall in the background whenever `main` moves: enable | disable | status.
auto-update cmd="enable":
    scripts/auto-update.sh {{cmd}}

# Core tests and the app's headless UI tests (the real app, a fake GitHub, fake assistant CLIs).
test:
    cargo test --workspace

# Performance budgets (SPEC.md) against a release build on this machine; keep the window visible.
bench:
    cargo build --release -p wispydiff -p wispy-fixtures
    scripts/bench.sh release
