#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts
cargo rustc --lib --crate-type staticlib --release --features linux
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/machine.c target/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/machine-linux
timeout 60s artifacts/machine-linux
cargo rustc --lib --crate-type staticlib --release --features host-machine --target-dir target/host-machine
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/machine.c native/machine_linux.c tests/host_backend.c target/host-machine/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/machine-host
timeout 60s artifacts/machine-host
