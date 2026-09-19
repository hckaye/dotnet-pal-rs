#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 integration/wasi/generate.py --check
node --test tests/wasi_host.test.mjs
mkdir -p artifacts/wasi-bridge
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features wasi-dispatch --target wasm32-wasip1 --target-dir target/wasi-dispatch
cc="${CLANG:-clang}"
args=(--target=wasm32-unknown-unknown -std=c11 -O2 -ffreestanding -fno-builtin -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native -nostdlib
      tests/wasi_bridge.c crates/dotnet-pal-build/native/wasi_bridge.c target/wasi-dispatch/wasm32-wasip1/release/libdotnet_pal_standalone.a
      -Wl,--no-entry -Wl,--fatal-warnings -Wl,--export=pal_bridge_test -Wl,--export-memory)
"$cc" "${args[@]}" tests/freestanding_memory.c -o artifacts/wasi-bridge/transport.wasm
timeout 60s node tests/wasi_bridge.mjs artifacts/wasi-bridge/transport.wasm
if [[ -n "${WASI_SDK_PATH:-}" ]]; then
  # Force references to ALL 45 SDK public functions. wasm-ld must type-check
  # their raw import calls against our definitions, including usually-dead code.
  mapfile -t force < <(python3 -c 'import json;print("\n".join("-Wl,--undefined=__wasi_"+o["name"] for o in json.load(open("integration/wasi/schema.json"))["operations"]))')
  "$WASI_SDK_PATH/bin/clang" "${args[@]}" "$WASI_SDK_PATH/share/wasi-sysroot/lib/wasm32-wasip1/libc.a"     "${force[@]}" -o artifacts/wasi-bridge/sdk-abi.wasm
  timeout 60s node tests/wasi_bridge.mjs artifacts/wasi-bridge/sdk-abi.wasm
  echo 'WASI SDK ABI AUDIT PASS all 45 raw imports, no linker wrapping, no direct WASI imports'
fi
