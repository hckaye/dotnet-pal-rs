#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/elf
cargo build --release
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude tests/elf.c target/release/libdotnet_pal_rs.a \
  -Wl,--gc-sections -Wl,--export-dynamic-symbol=dotnet_pal_elf_test_anchor -lpthread -ldl -lm -o artifacts/elf/linux
timeout 60s artifacts/elf/linux
cargo build --release --no-default-features --features host-elf --target-dir target/host-elf
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -DPAL_ELF_FAULT_HOST tests/elf.c tests/elf_host.c \
  tests/context_host.c tests/runtime_host.c tests/host_backend.c tests/services_host.c tests/kernel_host.c \
  target/host-elf/release/libdotnet_pal_rs.a -Wl,--gc-sections -Wl,--export-dynamic-symbol=dotnet_pal_elf_test_anchor -lpthread -ldl -lm -o artifacts/elf/host
for mode in {0..11}; do timeout 60s artifacts/elf/host "$mode"; done
