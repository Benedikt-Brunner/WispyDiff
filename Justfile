# WispyDiff — CI runs `just test`; the performance budgets (`just bench`) only run locally.

default:
    @just --list

# Run the app with hot reload.
dev:
    pnpm tauri dev

# Build the app bundle (macOS: bundle/macos/WispyDiff.app, Linux: bundle/{deb,appimage}).
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
bench: (build-e2e "release")
    cargo run --release -q -p wispy-core --example core_timings -- e2e/.fixtures/stack/origin
    WISPY_E2E_PERF=1 WISPY_E2E_PROFILE=release pnpm exec wdio run e2e/wdio.conf.ts --spec e2e/specs/perf.e2e.ts

# All functional specs (everything but the performance budgets).
e2e-functional: (build-e2e "e2e")
    pnpm exec wdio run e2e/wdio.conf.ts

# App build with the embedded WebDriver server, plus the fixture generator (always `e2e`). Functional
# specs use the quick-to-link `e2e` profile; the performance budgets need `release`.
build-e2e profile="e2e":
    cargo build --profile e2e -p wispy-fixtures
    ./target/e2e/wispy-fixtures generate --out e2e/.fixtures/stack --seed 42 > /dev/null
    pnpm tauri build --features e2e --no-bundle --config src-tauri/tauri.e2e.conf.json -- --profile {{profile}}
