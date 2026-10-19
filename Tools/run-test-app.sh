#!/bin/bash
# Launches build/app.noindex/LiteMail.app against a throwaway data directory with the automation harness on.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
pkill -f "LiteMail.app/Contents/MacOS/LiteMail" 2>/dev/null || true
sleep 0.5
DATA="${1:-$ROOT/.testserver/appdata}"
LANG_ARGS=()
if [ -n "${LITEMAIL_LANG:-}" ]; then LANG_ARGS=(-AppleLanguages "($LITEMAIL_LANG)"); fi
mkdir -p "$DATA" "$ROOT/.testserver/ui"
LITEMAIL_DATA_DIR="$DATA" LITEMAIL_AUTOMATION="$ROOT/.testserver/ui" LITEMAIL_TEST_SECRETS_FILE="$DATA/test-secrets.txt" \
  nohup "$ROOT/build/app.noindex/LiteMail.app/Contents/MacOS/LiteMail" ${LANG_ARGS[@]+"${LANG_ARGS[@]}"} > "$ROOT/.testserver/app.log" 2>&1 &
sleep 2
pgrep -f "LiteMail.app/Contents/MacOS/LiteMail" >/dev/null && echo "LiteMail running (data: $DATA)"
