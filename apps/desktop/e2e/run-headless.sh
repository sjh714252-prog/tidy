#!/usr/bin/env bash
# Headless GUI test (Linux/cloud): runs the dev app under Xvfb with a
# throw-away config/DB, Vite on :1420, then drives the UI with Playwright
# through the dev bridge. Needs: libwebkit2gtk-4.1-dev, xvfb, `npm i --no-save playwright`.
#   S=/tmp/tidy-e2e bash e2e/run-headless.sh [m3|m4|m5|m6|m7|m8|m9]   (default m3)
# Each run starts from a fresh config/DB. APP_ENV adds environment for the
# app, e.g. APP_ENV="TIDY_DEV_AUTOSTART=1" (m7 does this itself).
set -euo pipefail
SCRIPT="${1:-m3}"
[ "$SCRIPT" = m7 ] && APP_ENV="${APP_ENV:-TIDY_DEV_AUTOSTART=1}"
S="${S:-/tmp/tidy-e2e}"; T="$S/t"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
pkill -9 -x tidy-desktop 2>/dev/null || true; sleep 1
rm -rf "$T" "$HOME/Documents/Tidy" "$HOME/.local/share/tidy"; mkdir -p "$T/바탕 화면"
(cd "$HERE/src-tauri" && cargo build)
pgrep -f "vite --port 1420" >/dev/null || (cd "$HERE" && nohup npx vite --port 1420 >"$S/vite.log" 2>&1 &)
# A throw-away keychain service for the vault password (M9): never the real one.
(cd "$HERE/src-tauri" && env ${APP_ENV:-} TIDY_VAULT_KEYCHAIN="tidy-e2e-$$-$(date +%s)" TIDY_CONFIG="$T/config.toml" WEBKIT_DISABLE_COMPOSITING_MODE=1 nohup xvfb-run -a target/debug/tidy-desktop >"$S/app-$SCRIPT.log" 2>&1 &)
sleep 6
cd "$HERE" && T="$T" S="$S" node "e2e/$SCRIPT.mjs"
