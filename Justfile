# WispyDiff — everything runs locally; there is no CI.

default:
    @just --list

# Run the app with hot reload.
dev:
    pnpm tauri dev

# Build the app bundle (target/release/bundle/macos/WispyDiff.app).
build:
    pnpm tauri build

# Core tests, frontend type-check, and functional e2e tests.
test: test-core typecheck e2e-functional

test-core:
    cargo test --workspace

typecheck:
    pnpm exec tsc --noEmit
    pnpm exec tsc --noEmit -p e2e

# Performance budgets (SPEC.md) against an optimized build on this machine.
bench: build-e2e
    cargo run --release -q -p wispy-core --example core_timings -- e2e/.fixtures/stack/origin
    pnpm exec wdio run e2e/wdio.conf.ts --spec e2e/specs/perf.e2e.ts

e2e-functional: build-e2e
    pnpm exec wdio run e2e/wdio.conf.ts --spec e2e/specs/open.e2e.ts

# Release build with the embedded WebDriver server, plus the fixture generator.
build-e2e:
    cargo build --release -p wispy-fixtures
    ./target/release/wispy-fixtures generate --out e2e/.fixtures/stack --seed 42 > /dev/null
    pnpm tauri build --features e2e --no-bundle --config src-tauri/tauri.e2e.conf.json
