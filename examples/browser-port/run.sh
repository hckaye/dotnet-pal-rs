#!/usr/bin/env bash
# Browser port: C -> Rust boundary whose OS services are JavaScript imports.
# Executes the same module and probe in Node and in a real headless browser.
# This is the PAL <-> browser host connection; the managed application under
# app/ is a separate step (see README.md in this directory).
set -euo pipefail
cd "$(dirname "$0")"
root="$(cd ../.. && pwd)"
mkdir -p artifacts
node --test tests/browser_host.test.mjs
cc="${CLANG:-clang}"
common=(-std=c11 -O2 -ffreestanding -fno-builtin -Wall -Wextra -Werror "-I$root/include" -nostdlib
        "$root/tests/freestanding_memory.c" -Wl,--no-entry -Wl,--export=pal_browser_test -Wl,--export-memory -Wl,--fatal-warnings)
build() { # variant target
  local variant=$1 target=$2 flags=() define=() features=() name="$1-$2"
  [[ "$target" != wasm32v1-none ]] || flags+=(-mcpu=mvp)
  if [[ "$variant" == grow ]]; then
    define=(-DPAL_BROWSER_HEAP=1)
    features=(--no-default-features --features grow)
    flags+=(-Wl,--max-memory=6291456) # a real engine limit for the grow-failure test
  fi
  cargo build -p browser-port --release ${features[@]+"${features[@]}"} --target "$target" --target-dir "$root/target/browser-port-$variant"
  "$cc" --target=wasm32-unknown-unknown ${flags[@]+"${flags[@]}"} ${define[@]+"${define[@]}"} "${common[@]}" \
    tests/browser.c "$root/target/browser-port-$variant/$target/release/libbrowser_port.a" -o "artifacts/$name.wasm"
}
for target in wasm32-unknown-unknown wasm32v1-none; do
  build arena "$target"
  timeout 60s node tests/browser.mjs "artifacts/arena-$target.wasm"
  build grow "$target"
  timeout 60s node tests/browser.mjs "artifacts/grow-$target.wasm" --heap
done

# Real browser execution of the wasm32-unknown-unknown modules.
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
for variant in arena grow; do
  flag=(); [[ "$variant" != grow ]] || flag=(--heap)
  page="artifacts/$variant.html"
  node page.mjs "artifacts/$variant-wasm32-unknown-unknown.wasm" "$page" ${flag[@]+"${flag[@]}"}
  profile="$(mktemp -d)"
  timeout 120s "$browser" --headless=new --disable-gpu --no-sandbox --no-first-run --disable-background-networking \
    --disable-component-update --virtual-time-budget=10000 --user-data-dir="$profile" \
    --dump-dom "file://$PWD/$page" > "artifacts/$variant.dom.html" 2> "artifacts/$variant.browser.log"
  rm -rf "$profile"
  result="$(grep -o 'BROWSER \(PASS\|FAIL\).*' "artifacts/$variant.dom.html" | head -1 | sed 's#</pre>.*##; s/&quot;/"/g')"
  echo "$result"
  [[ "$result" == "BROWSER PASS "* ]] || { echo "browser execution failed for $variant; see artifacts/$variant.dom.html" >&2; exit 1; }
  [[ "$result" == *'"unexpectedConsoleErrors":[]'* ]] || { echo 'unexpected console errors in the page' >&2; exit 1; }
done
echo 'BROWSER PORT PASS: Node and real browser executed the same C -> Rust -> JS boundary for both storage variants'
