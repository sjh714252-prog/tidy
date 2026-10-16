#!/usr/bin/env bash
# Headless M3 GUI test (Linux/cloud): runs the dev app under Xvfb with a
# throw-away config/DB, Vite on :1420, then drives the UI with Playwright
# through the dev bridge. Needs: libwebkit2gtk-4.1-dev, xvfb, `npm i --no-save playwright`.
#   S=/tmp/tidy-e2e bash e2e/run-headless.sh
set -euo pipefail
S="${S:-/tmp/tidy-e2e}"; T="$S/t"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
pkill -9 -x tidy-desktop 2>/dev/null || true; sleep 1
rm -rf "$T" "$HOME/Documents/Tidy" "$HOME/.local/share/tidy"; mkdir -p "$T/바탕 화면"
(cd "$HERE/src-tauri" && cargo build)
pgrep -f "vite --port 1420" >/dev/null || (cd "$HERE" && nohup npx vite --port 1420 >"$S/vite.log" 2>&1 &)
(cd "$HERE/src-tauri" && TIDY_CONFIG="$T/config.toml" WEBKIT_DISABLE_COMPOSITING_MODE=1 nohup xvfb-run -a target/debug/tidy-desktop >"$S/app.log" 2>&1 &)
sleep 6
cd "$HERE" && T="$T" S="$S" node e2e/m3.mjs
