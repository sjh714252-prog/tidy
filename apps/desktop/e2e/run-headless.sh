#!/usr/bin/env bash
# Headless GUI test (Linux/cloud): runs the dev app under Xvfb with a
# throw-away config/DB, Vite on :1420, then drives the UI with Playwright
# through the dev bridge. Needs: libwebkit2gtk-4.1-dev, xvfb, `npm i --no-save playwright`.
#   S=/tmp/tidy-e2e bash e2e/run-headless.sh [m3|…|m14]   (default m3)
# Each run starts from a fresh config/DB. APP_ENV adds environment for the
# app, e.g. APP_ENV="TIDY_DEV_AUTOSTART=1" (m7 does this itself).
set -euo pipefail
SCRIPT="${1:-m3}"
[ "$SCRIPT" = m7 ] && APP_ENV="${APP_ENV:-TIDY_DEV_AUTOSTART=1}"
# m11: fake GitHub releases API (in m11.mjs) and record links instead of opening a browser.
[ "$SCRIPT" = m11 ] && APP_ENV="${APP_ENV:-TIDY_UPDATE_URL=http://127.0.0.1:18097/repos/sjh714252-prog/tidy/releases/latest TIDY_E2E_NO_OPEN=1}"
# m12: record "open folder" (고객사별 보기) and links instead of opening them.
[ "$SCRIPT" = m12 ] && APP_ENV="${APP_ENV:-TIDY_E2E_NO_OPEN=1}"
# m13: PathText clicks are recorded the same way.
[ "$SCRIPT" = m13 ] && APP_ENV="${APP_ENV:-TIDY_E2E_NO_OPEN=1}"
# m14: "휴지통" is a folder (TIDY_E2E_TRASH) and "기존 파일 열기"/"휴지통 열기" are recorded.
[ "$SCRIPT" = m14 ] && APP_ENV="${APP_ENV:-TIDY_E2E_NO_OPEN=1 TIDY_E2E_TRASH=${S:-/tmp/tidy-e2e}/t/휴지통}"
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
