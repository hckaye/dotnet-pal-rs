#!/usr/bin/env bash
# Conformance of the priority group: the Linux provider and the C host-table
# provider, each checked against getpriority and the stat files of procfs for this
# process, a child and a child with several threads; the host table is also
# checked malformed in three ways and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/priority
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/priority.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/priority/priority-linux
timeout 60s artifacts/priority/priority-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-priority --target-dir "$root/host-priority"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/priority.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/priority_host.c "$root/host-priority/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/priority/priority-host
for fault in 0 1 2 3 4; do timeout 60s artifacts/priority/priority-host "$fault"; done
echo "PRIORITY GROUP PASS the priority of this process with its threads, of a child and of a child with threads on Linux and the host table"
