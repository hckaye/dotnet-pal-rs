#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features linear --target-dir target/linear
c++ -std=c++17 -O2 -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native tests/gc_linear.cpp \
  target/linear/release/libdotnet_pal_standalone.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/gc-linear
timeout 90s artifacts/gc-linear
echo 'LINEAR GC ADAPTER CONTRACT PASS (explicit eager-storage semantics)'
