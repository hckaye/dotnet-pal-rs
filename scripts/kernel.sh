#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts
cargo build --release
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/kernel.c target/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/kernel-linux
timeout 60s artifacts/kernel-linux
cargo build --release --no-default-features --features host-kernel --target-dir target/host-kernel
for suite in kernel kernel_faults; do
  provider=tests/kernel_host.c
  [[ "$suite" != kernel_faults ]] || provider=""
  cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude "tests/$suite.c" $provider tests/host_backend.c tests/services_host.c \
    target/host-kernel/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o "artifacts/$suite-host"
done
timeout 60s artifacts/kernel-host
for mode in {1..8}; do timeout 10s artifacts/kernel_faults-host "$mode"; done
c++ -std=c++17 -O2 -Wall -Wextra -Werror -Iinclude -Inative tests/kernel_adapter.cpp -o artifacts/kernel-adapter
timeout 10s artifacts/kernel-adapter

c++ -std=c++17 -O2 -Wall -Wextra -Werror -Iinclude -Inative tests/unwind_lock.cpp \
  target/release/libdotnet_pal_rs.a -Wl,--gc-sections -lpthread -ldl -lm -o artifacts/unwind-lock
timeout 60s artifacts/unwind-lock
