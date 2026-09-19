#!/usr/bin/env bash
# Conformance of the terminal group: the Linux provider and the C host-table
# provider, each checked against what the kernel reports for a pseudo-terminal
# the test puts behind its own standard streams; the host table is also checked
# malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/terminal
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/terminal.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/terminal/terminal-linux
timeout 60s artifacts/terminal/terminal-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-terminal --target-dir "$root/host-terminal"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/terminal.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/terminal_host.c "$root/host-terminal/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/terminal/terminal-host
for fault in 0 1 2; do timeout 60s artifacts/terminal/terminal-host "$fault"; done
echo "TERMINAL GROUP PASS window size, input modes, readiness and editing characters on Linux and the host table"
