#!/usr/bin/env bash
# Conformance of the processes group: the Linux provider (with pidfd_open and
# with it refused, as before Linux 5.3) and the C host-table provider start the
# same children and carry the same pipe traffic; the host table is also checked
# malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/processes
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/processes.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/processes/processes-linux
timeout 60s artifacts/processes/processes-linux
timeout 60s artifacts/processes/processes-linux nopidfd
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-processes --target-dir "$root/host-processes"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/processes.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/processes_host.c "$root/host-processes/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/processes/processes-host
for fault in 0 1 2; do timeout 60s artifacts/processes/processes-host "$fault"; done
echo "PROCESSES GROUP PASS spawn, pipes, timed waits and termination on Linux and the host table"
