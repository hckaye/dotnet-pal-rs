#!/usr/bin/env bash
# Conformance of the accounts group: the Linux provider and the C host-table
# provider, each checked against what the C library says about the same passwd
# and group databases and the same process; the host table is also checked
# malformed, misbehaving and silent.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/accounts
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/accounts.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/accounts/accounts-linux
timeout 60s artifacts/accounts/accounts-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-accounts --target-dir "$root/host-accounts"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/accounts.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/accounts_host.c "$root/host-accounts/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/accounts/accounts-host
for fault in 0 1 2 3; do timeout 60s artifacts/accounts/accounts-host "$fault"; done
echo "ACCOUNTS GROUP PASS accounts by id and by name, the groups of this process and of an account on Linux and the host table"
