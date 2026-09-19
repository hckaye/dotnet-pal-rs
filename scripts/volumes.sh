#!/usr/bin/env bash
# Conformance of the volumes group: the Linux provider and the C host-table
# provider, each checked against what getmntent lists and statvfs measures for
# the same system; the host table is also checked malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/volumes
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/volumes.c "$root/release/libdotnet_pal_standalone.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/volumes/volumes-linux
timeout 60s artifacts/volumes/volumes-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-volumes --target-dir "$root/host-volumes"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/volumes.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/volumes_host.c "$root/host-volumes/release/libdotnet_pal_standalone.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/volumes/volumes-host
for fault in 0 1 2; do timeout 60s artifacts/volumes/volumes-host "$fault"; done
echo "VOLUMES GROUP PASS mount points, space and format on Linux and the host table"
