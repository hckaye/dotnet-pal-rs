#!/usr/bin/env bash
# Executes the built BrowserApp page in a real headless Chromium (after build-app.sh).
set -euo pipefail
cd "$(dirname "$0")"
[[ -f artifacts/app/index.html ]] || { echo 'run build-app.sh first (needs a Linux host or the container)' >&2; exit 1; }
find_browser() {
  local candidate
  for candidate in "${BROWSER_BIN:-}" google-chrome google-chrome-stable chromium chromium-browser chrome-headless-shell \
      "$HOME"/Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-*/chrome-headless-shell \
      "$HOME"/.cache/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-linux*/chrome-headless-shell \
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"; do
    [[ -n "$candidate" ]] || continue
    if [[ -x "$candidate" ]]; then echo "$candidate"; return 0; fi
    if command -v "$candidate" >/dev/null 2>&1; then command -v "$candidate"; return 0; fi
  done
  return 1
}
browser="$(find_browser)" || { echo 'no Chromium-based browser found; set BROWSER_BIN=/path/to/chrome' >&2; exit 1; }
echo "browser: $browser ($("$browser" --version 2>/dev/null | head -1))"
profile="$(mktemp -d)"
timeout 300s "$browser" --headless=new --disable-gpu --no-sandbox --no-first-run --disable-background-networking \
  --disable-component-update --virtual-time-budget=60000 --user-data-dir="$profile" \
  --dump-dom "file://$PWD/artifacts/app/index.html" > artifacts/app/dom.html 2> artifacts/app/browser.log
rm -rf "$profile"
result="$(grep -o 'MANAGED BROWSER \(PASS\|FAIL\).*' artifacts/app/dom.html | head -1 | sed 's#</pre>.*##; s/&quot;/"/g')"
echo "$result"
[[ "$result" == "MANAGED BROWSER PASS "* ]] || { echo 'managed browser execution failed; see artifacts/app/dom.html' >&2; exit 1; }
echo 'MANAGED BROWSER PASS: C# GC, exceptions, finalizers and BCL console/clock/entropy executed in a real browser through dotnet-pal-rs'
