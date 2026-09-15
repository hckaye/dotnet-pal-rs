#!/usr/bin/env bash
# Instrument BOTH Rust and C/C++ boundary code. This is not a sanitized .NET build.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# == 1 && "$1" == address ]] || { echo 'usage: sanitize.sh address' >&2; exit 2; }
triple=x86_64-unknown-linux-gnu
rust=nightly-2025-03-15
out="$PWD/artifacts/asan"
mkdir -p "$out"
ulimit -c 0
export ASAN_OPTIONS=detect_leaks=1:detect_stack_use_after_return=1:halt_on_error=1:handle_segv=0:handle_sigbus=0
# Use Clang's sanitizer runtime for the mixed-language final link.
export RUSTFLAGS='-Zsanitizer=address -Zexternal-clangrt -Cdebuginfo=1 -Cforce-frame-pointers=yes'
common=(-O1 -g -fno-omit-frame-pointer -fsanitize=address -Wall -Wextra -Werror -Iinclude -Inative)
for backend in linux host-kernel linear; do
  cargo "+$rust" build -Zbuild-std=core,compiler_builtins --release --no-default-features \
    --features "$backend" --target "$triple" --target-dir "target/asan-$backend"
  lib="target/asan-$backend/$triple/release/libdotnet_pal_rs.a"
  if [[ "$backend" == linear ]]; then
    for suite in linear linear_threads; do
      clang -std=c11 "${common[@]}" "tests/$suite.c" "$lib" -Wl,--gc-sections -lpthread -ldl -lm -o "$out/$suite"
      timeout 120s "$out/$suite"
    done
    clang++ -std=c++17 "${common[@]}" tests/gc_linear.cpp "$lib" -Wl,--gc-sections -lpthread -ldl -lm -o "$out/gc-linear"
    timeout 120s "$out/gc-linear"
  else
    providers=()
    if [[ "$backend" == host-kernel ]]; then
      providers=(tests/host_backend.c tests/services_host.c tests/kernel_host.c)
    fi
    for suite in abi services kernel; do
      clang -std=c11 "${common[@]}" "tests/$suite.c" "${providers[@]}" "$lib" \
        -Wl,--gc-sections -lpthread -ldl -lm -o "$out/$backend-$suite"
      timeout 120s "$out/$backend-$suite"
    done
  fi
done
echo 'MIXED RUST/C ADDRESS SANITIZER PASS (boundary contracts; not .NET runtime instrumentation)'
