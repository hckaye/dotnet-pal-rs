#!/usr/bin/env bash
# Conformance of the packets group: the Linux provider and the C host-table
# providers report where datagrams of the sockets group arrived, each checked
# against what the kernel attaches to the same traffic, and carry an ICMP echo
# over raw sockets when the process may open them; the host table is also
# checked malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/packets
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/packets.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/packets/packets-linux
timeout 60s artifacts/packets/packets-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-sockets,host-packets --target-dir "$root/host-packets"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/packets.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/sockets_host.c tests/packets_host.c "$root/host-packets/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/packets/packets-host
for fault in 0 1 2; do timeout 60s artifacts/packets/packets-host "$fault"; done
echo "PACKETS GROUP PASS destinations, interfaces, raw ICMP echo and the fragmentation switch on Linux and the host table"
