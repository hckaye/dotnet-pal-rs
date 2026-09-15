#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/target/wasm-arena"
for target in wasm32-unknown-unknown wasm32v1-none; do
  cargo +1.85.1 build --manifest-path examples/wasm-arena/Cargo.toml --release --target "$target"
  node scripts/wasm.mjs "$CARGO_TARGET_DIR/$target/release/dotnet_pal_wasm_arena.wasm"
done
