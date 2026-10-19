#!/bin/bash
# Starts / stops the local mail servers used by the integration tests.
#   Dovecot (IMAP)  127.0.0.1:31143 plaintext
#   tls_front.py    127.0.0.1:31993 IMAP over TLS, 127.0.0.1:31144 IMAP with STARTTLS
#   smtp_sink.py    127.0.0.1:31587 (plain + STARTTLS), 127.0.0.1:31465 (TLS)
# Any user name logs in with password "test". Data lives in .testserver/ (git-ignored).
#
# Usage: Tools/testserver/run.sh start|stop|restart|status
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# Unix socket paths on macOS are limited to 104 bytes: keep ROOT short.
ROOT="${LITEMAIL_IT_ROOT:-$(cd "$HERE/../.." && pwd)/.testserver}"
IMAP_PORT=31143
IMAPS_PORT=31993
IMAP_STARTTLS_PORT=31144
SMTP_PORT=31587
SMTPS_PORT=31465

wait_port() {
  for _ in $(seq 1 50); do
    if nc -z 127.0.0.1 "$1" 2>/dev/null; then return 0; fi
    sleep 0.1
  done
  echo "port $1 did not open" >&2
  return 1
}

start() {
  stop >/dev/null 2>&1 || true
  rm -rf "$ROOT"
  mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/home"
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$ROOT/key.pem" -out "$ROOT/cert.pem" \
    -days 3650 -subj "/CN=localhost" >/dev/null 2>&1
  sed -e "s#@ROOT@#$ROOT#g" -e "s#@USER@#$(id -un)#g" -e "s#@GROUP@#$(id -gn)#g" \
      -e "s#@UID@#$(id -u)#g" -e "s#@GID@#$(id -g)#g" \
      -e "s#@IMAP_PORT@#$IMAP_PORT#g" \
      "$HERE/dovecot.conf.in" > "$ROOT/dovecot.conf"
  dovecot -c "$ROOT/dovecot.conf"
  python3 "$HERE/smtp_sink.py" --root "$ROOT" --port "$SMTP_PORT" --tls-port "$SMTPS_PORT" \
    --cert "$ROOT/cert.pem" --key "$ROOT/key.pem" > "$ROOT/smtp.log" 2>&1 &
  echo $! > "$ROOT/smtp.pid"
  python3 "$HERE/tls_front.py" --upstream "$IMAP_PORT" --tls-port "$IMAPS_PORT" \
    --starttls-port "$IMAP_STARTTLS_PORT" --cert "$ROOT/cert.pem" --key "$ROOT/key.pem" > "$ROOT/tls.log" 2>&1 &
  echo $! > "$ROOT/tls.pid"
  wait_port "$IMAP_PORT"; wait_port "$IMAPS_PORT"; wait_port "$IMAP_STARTTLS_PORT"
  wait_port "$SMTP_PORT"; wait_port "$SMTPS_PORT"
  # Dovecot's auth process needs a moment after the listener opens.
  for _ in $(seq 1 50); do
    # Capture first: with pipefail, `grep -q` exiting early makes nc fail with SIGPIPE.
    out="$(printf 'a LOGIN probe@localhost test\r\nb LOGOUT\r\n' | nc -w 2 127.0.0.1 "$IMAP_PORT" || true)"
    if [[ "$out" == *"a OK"* ]]; then
      echo "test servers ready in $ROOT"
      return 0
    fi
    sleep 0.2
  done
  echo "dovecot did not accept logins; see $ROOT/dovecot.log" >&2
  return 1
}

stop() {
  if [ -f "$ROOT/run/master.pid" ]; then kill "$(cat "$ROOT/run/master.pid")" 2>/dev/null || true; fi
  if [ -f "$ROOT/smtp.pid" ]; then kill "$(cat "$ROOT/smtp.pid")" 2>/dev/null || true; fi
  if [ -f "$ROOT/tls.pid" ]; then kill "$(cat "$ROOT/tls.pid")" 2>/dev/null || true; fi
  pkill -f "dovecot -c $ROOT/dovecot.conf" 2>/dev/null || true
  # Wait for every Dovecot child (imap, imap-login, anvil, auth) to exit before the
  # data directory is wiped; otherwise a new instance fights the old one over sockets.
  for _ in $(seq 1 50); do
    if ! pgrep -f "$ROOT" >/dev/null 2>&1; then break; fi
    sleep 0.1
  done
  pkill -9 -f "$ROOT" 2>/dev/null || true
  echo "test servers stopped"
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  restart) stop; start ;;
  status)
    nc -z 127.0.0.1 $IMAP_PORT && echo "imap up" || echo "imap down"
    nc -z 127.0.0.1 $SMTP_PORT && echo "smtp up" || echo "smtp down" ;;
  *) echo "usage: $0 start|stop|restart|status" >&2; exit 2 ;;
esac
