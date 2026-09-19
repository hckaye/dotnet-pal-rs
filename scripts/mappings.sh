#!/usr/bin/env bash
# Conformance of the mappings group: the Linux provider and the C host-table
# provider, each with the files group of the same table, checked against what the
# kernel lists for the process and answers to the same requests; the host table is
# also checked malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/mappings
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/mappings.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/mappings/mappings-linux
timeout 60s artifacts/mappings/mappings-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-files,host-mappings --target-dir "$root/host-mappings"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/mappings.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/files_host.c tests/mappings_host.c "$root/host-mappings/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/mappings/mappings-host
for fault in 0 1 2; do timeout 60s artifacts/mappings/mappings-host "$fault"; done
echo "MAPPINGS GROUP PASS shared and private file mappings on Linux and the host table"
