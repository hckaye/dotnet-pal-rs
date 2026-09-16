#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts
link_gc=-Wl,--gc-sections
[[ "$(uname -s)" != Darwin ]] || link_gc=-Wl,-dead_strip
cargo build --release --no-default-features --features linear-heap --target-dir target/dynamic-native
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/linear_heap.c \
  target/dynamic-native/release/libdotnet_pal_rs.a "$link_gc" -lpthread -ldl -lm -o artifacts/linear-heap
python3 -c 'import subprocess; subprocess.run(["artifacts/linear-heap"], check=True, timeout=60)'
