#!/usr/bin/env bash
# Conformance of the topology, process, image and streams groups: the Linux
# providers and the C host-table providers, each checked against the kernel's own answers.
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/platform
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux
for group in topology process image streams; do
  cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude "tests/$group.c" target/release/libdotnet_pal_standalone.a \
    -Wl,--gc-sections -Wl,--build-id=sha1 -lpthread -ldl -lm -o "artifacts/platform/$group-linux"
  timeout 60s "artifacts/platform/$group-linux"
done
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features \
  --features host-runtime,host-topology,host-process,host-image,host-streams --target-dir target/host-platform
providers="tests/host_backend.c tests/services_host.c tests/kernel_host.c tests/runtime_host.c tests/topology_host.c tests/process_host.c tests/image_host.c tests/streams_host.c"
for group in topology process image streams; do
  # shellcheck disable=SC2086
  cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST "tests/$group.c" $providers \
    target/host-platform/release/libdotnet_pal_standalone.a -Wl,--gc-sections -Wl,--build-id=sha1 -lpthread -ldl -lm -o "artifacts/platform/$group-host"
  for fault in 0 1 2; do timeout 60s "artifacts/platform/$group-host" "$fault"; done
done
echo "PLATFORM GROUPS PASS topology, process, image and streams on Linux and host tables"
