#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/wasm
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features wasi-clock --target wasm32-wasip1 --target-dir target/wasi-clock
"${CLANG:-clang}" --target=wasm32-wasip1 -std=c11 -O2 -ffreestanding -fno-builtin \
  -Wall -Wextra -Werror -Iinclude -nostdlib tests/wasi_clock.c tests/freestanding_memory.c \
  target/wasi-clock/wasm32-wasip1/release/libdotnet_pal_standalone.a \
  -Wl,--no-entry -Wl,--export=pal_clock_test -Wl,--export-memory \
  -o artifacts/wasm/wasi-clock.wasm
timeout 60s node tests/wasi_clock.mjs artifacts/wasm/wasi-clock.wasm
