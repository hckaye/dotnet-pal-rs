#!/usr/bin/env bash
# Conformance of the files group: the Linux provider and the C host-table
# provider, each checked against what the kernel reports for a scratch directory;
# the host table is also checked malformed, misbehaving and without the optional
# callbacks.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/files
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/files.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/files/files-linux
timeout 60s artifacts/files/files-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-files --target-dir "$root/host-files"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/files.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/files_host.c "$root/host-files/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/files/files-host
for fault in 0 1 2 3; do timeout 60s artifacts/files/files-host "$fault"; done
echo "FILES GROUP PASS files and directories on Linux and the host table"
