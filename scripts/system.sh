#!/usr/bin/env bash
# Conformance of the system group: the Linux provider and the C host-table
# provider, each checked against what the C library and the kernel say about the
# same process; the host table is also checked malformed, misbehaving and silent.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/system
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/system.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/system/system-linux
timeout 60s artifacts/system/system-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-system --target-dir "$root/host-system"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/system.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/system_host.c "$root/host-system/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/system/system-host
for fault in 0 1 2 3; do timeout 60s artifacts/system/system-host "$fault"; done
echo "SYSTEM GROUP PASS environment, texts, CPU time, uptime and user ids on Linux and the host table"
