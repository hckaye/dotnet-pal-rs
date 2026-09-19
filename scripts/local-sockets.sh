#!/usr/bin/env bash
# Conformance of the local_sockets group: the Linux provider and the C host-table
# providers bind and connect Unix domain sockets of the sockets group by path, each
# checked against what the kernel says about the same descriptors; the host table
# is also checked malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/local-sockets
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/local_sockets.c "$root/release/libdotnet_pal_standalone.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/local-sockets/local-sockets-linux
timeout 60s artifacts/local-sockets/local-sockets-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-sockets,host-local-sockets --target-dir "$root/host-local-sockets"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/local_sockets.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/sockets_host.c tests/local_sockets_host.c "$root/host-local-sockets/release/libdotnet_pal_standalone.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/local-sockets/local-sockets-host
for fault in 0 1 2; do timeout 60s artifacts/local-sockets/local-sockets-host "$fault"; done
echo "LOCAL SOCKETS GROUP PASS bind, connect, paths and the peer's user on Linux and the host table"
