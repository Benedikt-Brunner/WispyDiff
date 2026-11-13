#!/usr/bin/env bash
# The performance budgets (SPEC.md) on this machine: the seeded worst-case fixture behind a fake
# GitHub, the release app driven through its `--perf` scenarios in a real window. Prints the JSON
# report and fails when a budget is missed.
#   scripts/bench.sh [profile]   (default: release; `just bench` builds it first)
#
# Keep the window visible: compositors throttle hidden windows, which isn't the app.
set -euo pipefail
cd "$(dirname "$0")/.."
PROFILE=${1:-release}
PORT=${WISPY_BENCH_PORT:-4611}
RUN=$(mktemp -d "${TMPDIR:-/tmp}/wispy-bench.XXXXXX")
FIXTURE=target/bench-fixture/stack

./target/$PROFILE/wispy-fixtures generate --out "$FIXTURE" --seed 42 > /dev/null
./target/$PROFILE/wispy-fixtures serve --fixture "$FIXTURE" --port "$PORT" > "$RUN/github.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2> /dev/null; rm -rf "$RUN"' EXIT
sleep 0.5

cargo run --release -q -p wispy-core --example core_timings -- "$FIXTURE/origin"
WISPY_DATA_DIR="$RUN/data" WISPY_GITHUB_API="http://127.0.0.1:$PORT" WISPY_GITHUB_TOKEN=fixture-token \
  ./target/$PROFILE/wispydiff --perf 'wispy/fixture#1'
