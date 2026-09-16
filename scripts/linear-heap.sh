#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts
cargo build --release --no-default-features --features linear-heap --target-dir target/dynamic-native
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/linear_heap.c \
  target/dynamic-native/release/libdotnet_pal_rs.a -lpthread -ldl -lm -o artifacts/linear-heap
python3 -c 'import subprocess; subprocess.run(["artifacts/linear-heap"], check=True, timeout=60)'
