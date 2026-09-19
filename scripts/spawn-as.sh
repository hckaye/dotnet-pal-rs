#!/usr/bin/env bash
# Conformance of the spawn_as group: the Linux provider and the C host-table
# provider start the same children under another identity and handle them
# through the processes group; the host table is also checked malformed and
# misbehaving. The main part needs root; as another user the test checks what a
# process without the privilege may and may not do, and says so.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/spawn-as
# PAL_TARGET_DIR moves both builds out of the way of another build in the same tree.
root="${PAL_TARGET_DIR:-target}"
cargo rustc --lib --crate-type staticlib --release --features linux --target-dir "$root"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/spawn_as.c "$root/release/libdotnet_pal_rs.a" \
  -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/spawn-as/spawn-as-linux
timeout 120s artifacts/spawn-as/spawn-as-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-runtime,host-processes,host-spawn-as --target-dir "$root/host-spawn-as"
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/spawn_as.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  tests/runtime_host.c tests/processes_host.c tests/spawn_as_host.c "$root/host-spawn-as/release/libdotnet_pal_rs.a" -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/spawn-as/spawn-as-host
for fault in 0 1 2; do timeout 120s artifacts/spawn-as/spawn-as-host "$fault"; done
# Once more as a user without the privilege, from a place any user can reach.
if [ "$(id -u)" = 0 ] && command -v setpriv >/dev/null; then
  scratch="$(mktemp -d)"; trap 'rm -rf "$scratch"' EXIT; chmod 0755 "$scratch"
  for binary in spawn-as-linux spawn-as-host; do
    install -m 0755 "artifacts/spawn-as/$binary" "$scratch/"
    timeout 120s setpriv --reuid 65534 --regid 65534 --groups 65534,100 "$scratch/$binary"
  done
fi
echo "SPAWN AS GROUP PASS children under another identity, handled through the processes group, on Linux and the host table"
