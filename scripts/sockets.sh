#!/usr/bin/env bash
# Conformance of the sockets group: the Linux provider and the C host-table
# provider carry the same loopback traffic, readiness, wake channels, options
# and name resolution; the host table is also checked malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/sockets
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/sockets.c "$root/release/libdotnet_pal_standalone.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/sockets/sockets-linux
timeout 60s artifacts/sockets/sockets-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-sockets --target-dir "$root/host-sockets"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/sockets.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/sockets_host.c "$root/host-sockets/release/libdotnet_pal_standalone.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/sockets/sockets-host
for fault in 0 1 2; do timeout 60s artifacts/sockets/sockets-host "$fault"; done
echo "SOCKETS GROUP PASS TCP, UDP, poll, wake channels, options and name resolution on Linux and host tables"
