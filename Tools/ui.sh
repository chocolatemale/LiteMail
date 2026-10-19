#!/bin/bash
# Sends automation commands to a running LiteMail (started with LITEMAIL_AUTOMATION) and prints the result.
#   Tools/ui.sh "smart all" "message 0" "snapshot inbox"
set -euo pipefail
DIR="${LITEMAIL_AUTOMATION:-$(cd "$(dirname "$0")/.." && pwd)/.testserver/ui}"
rm -f "$DIR/result.txt"
printf '%s\n' "$@" > "$DIR/command.txt.tmp" && mv "$DIR/command.txt.tmp" "$DIR/command.txt"
for _ in $(seq 1 600); do
  if [ -f "$DIR/result.txt" ]; then cat "$DIR/result.txt"; exit 0; fi
  sleep 0.1
done
echo "timeout waiting for LiteMail" >&2; exit 1
