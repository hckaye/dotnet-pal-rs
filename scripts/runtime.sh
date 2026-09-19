#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts/runtime
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/runtime.c target/release/libdotnet_pal_standalone.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/runtime/linux
timeout 60s artifacts/runtime/linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-runtime --target-dir target/host-runtime
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_HOST_TEST tests/runtime.c tests/runtime_host.c tests/host_backend.c tests/services_host.c tests/kernel_host.c target/host-runtime/release/libdotnet_pal_standalone.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/runtime/host
for fault in {0..9}; do timeout 60s artifacts/runtime/host "$fault"; done

cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/minipal_entropy.c crates/dotnet-pal-build/native/minipal_entropy_adapter.c -o artifacts/runtime/minipal-entropy
timeout 10s artifacts/runtime/minipal-entropy
