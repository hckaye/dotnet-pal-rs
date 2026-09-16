#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/context
cargo build --release
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/context.c target/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/context/linux
timeout 60s artifacts/context/linux
cargo build --release --no-default-features --features host-context --target-dir target/host-context
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_CONTEXT_HOST tests/context.c tests/context_host.c tests/runtime_host.c tests/host_backend.c tests/services_host.c tests/kernel_host.c     target/host-context/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/context/host
for fault in {0..5};do timeout 60s artifacts/context/host "$fault";done
