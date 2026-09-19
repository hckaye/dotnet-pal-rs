#!/usr/bin/env bash
# Conformance of the notifications group: the Linux provider and the C host-table
# provider, each driven with signals the kernel really delivers; the host table
# is also checked malformed and misbehaving.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/notifications
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/notifications.c "$root/release/libdotnet_pal_standalone.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/notifications/notifications-linux
timeout 60s artifacts/notifications/notifications-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-notifications --target-dir "$root/host-notifications"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/notifications.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/notifications_host.c "$root/host-notifications/release/libdotnet_pal_standalone.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/notifications/notifications-host
for fault in 0 1 2; do timeout 60s artifacts/notifications/notifications-host "$fault"; done
echo "NOTIFICATIONS GROUP PASS outside requests reported from a thread on Linux and the host table"
