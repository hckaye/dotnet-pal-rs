#!/usr/bin/env bash
# Conformance of the watches group: the Linux provider and the C host-table
# provider, each held against an inotify instance the test keeps for itself while
# it changes a scratch directory; the host table is also checked malformed (every
# required callback withheld in turn, and the capability bit) and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/watches
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/watches.c "$root/release/libdotnet_pal_standalone.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/watches/watches-linux
timeout 60s artifacts/watches/watches-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-watches --target-dir "$root/host-watches"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/watches.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/watches_host.c "$root/host-watches/release/libdotnet_pal_standalone.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/watches/watches-host
for fault in 0 1 2 3 4 5 6 7; do timeout 60s artifacts/watches/watches-host "$fault"; done
echo "WATCHES GROUP PASS changes to files and directories on Linux and the host table"
