#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/wasm
cargo rustc --lib --crate-type staticlib --release --no-default-features --features wasi-runtime --target wasm32-wasip1 --target-dir target/wasi-runtime
"${CLANG:-clang}" --target=wasm32-wasip1 -std=c11 -O2 -ffreestanding -fno-builtin -Wall -Wextra -Werror -Iinclude -nostdlib   tests/wasi_runtime.c tests/freestanding_memory.c target/wasi-runtime/wasm32-wasip1/release/libdotnet_pal_rs.a   -Wl,--no-entry -Wl,--export=pal_wasi_runtime_test -Wl,--export-memory -o artifacts/wasm/wasi-runtime.wasm
timeout 60s node tests/wasi_runtime.mjs artifacts/wasm/wasi-runtime.wasm
