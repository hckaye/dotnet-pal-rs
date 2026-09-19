#!/usr/bin/env bash
# Conformance of the network group: the Linux provider and the C host-table
# provider, each checked against what the C library and the kernel say about the
# same machine, with multicast datagrams between sockets of the sockets group; the
# host table is also checked malformed, misbehaving and silent.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/network
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/network.c "$root/release/libdotnet_pal_standalone.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/network/network-linux
timeout 60s artifacts/network/network-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-sockets,host-network --target-dir "$root/host-network"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/network.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/sockets_host.c tests/network_host.c "$root/host-network/release/libdotnet_pal_standalone.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/network/network-host
for fault in 0 1 2 3; do timeout 60s artifacts/network/network-host "$fault"; done
echo "NETWORK GROUP PASS interfaces, addresses, reverse lookup and multicast membership on Linux and the host table"
