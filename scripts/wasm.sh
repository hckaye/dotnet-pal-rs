#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/wasm
# This tests no_std boundary modules built for these targets, NOT WASI syscalls,
# .NET code generation, components, shared-memory threads, or a NativeAOT Wasm port.
for target in wasm32-unknown-unknown wasm32-wasip1 wasm32v1-none; do
  cargo build --release --no-default-features --features linear --target "$target"
  ctarget=wasm32-unknown-unknown
  [[ "$target" != wasm32-wasip1 ]] || ctarget=wasm32-wasi
  extra=()
  [[ "$target" != wasm32v1-none ]] || extra=(-mcpu=mvp)
  "${CLANG:-clang}" --target="$ctarget" "${extra[@]}" -std=c11 -O2 -ffreestanding -fno-builtin \
    -Wall -Wextra -Werror -Iinclude -nostdlib tests/linear.c tests/freestanding_memory.c \
    "target/$target/release/libdotnet_pal_rs.a" -Wl,--no-entry -Wl,--export=pal_test \
    -Wl,--export-memory -o "artifacts/wasm/$target.wasm"
  node tests/wasm.mjs "artifacts/wasm/$target.wasm"
done
