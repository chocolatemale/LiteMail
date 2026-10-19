#!/bin/bash
# Runs every test: Rust unit tests, Rust integration tests against local Dovecot + SMTP,
# and the Swift tests. Usage: scripts/test.sh [cargo test filter]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

"$ROOT/Tools/testserver/run.sh" start
trap '"$ROOT/Tools/testserver/run.sh" stop >/dev/null' EXIT
export LITEMAIL_IT_ROOT="$ROOT/.testserver"

cd "$ROOT/core"
cargo test --release "$@" -- --test-threads=4

if [ -d "$ROOT/app" ] && [ -z "${1:-}" ]; then
  cd "$ROOT/app"
  swift test 2>&1 | tail -20
fi
