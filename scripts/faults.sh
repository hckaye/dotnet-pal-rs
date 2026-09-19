#!/usr/bin/env bash
# Conformance of the faults group: real CPU faults reported through the C
# host-table provider. Linux proper has no Faults provider, so only the host build runs.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/faults
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-faults --target-dir target/host-faults
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/faults.c tests/faults_host.c tests/host_backend.c \
  target/host-faults/release/libdotnet_pal_standalone.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/faults/host
for fault in 0 1 2; do timeout 60s artifacts/faults/host "$fault"; done
echo "FAULTS GROUP PASS faults reported, edited and resumed through the host table on $(uname -m)"
