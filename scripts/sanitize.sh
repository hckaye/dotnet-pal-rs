#!/usr/bin/env bash
# Instrument BOTH Rust and C/C++ boundary code. This is not a sanitized .NET build.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# == 1 && ( "$1" == address || "$1" == thread ) ]] || { echo 'usage: sanitize.sh address|thread' >&2; exit 2; }
sanitizer=$1
triple=x86_64-unknown-linux-gnu
rust=nightly-2025-03-15
out="$PWD/artifacts/$sanitizer"
mkdir -p "$out"
ulimit -c 0
export ASAN_OPTIONS=detect_leaks=1:detect_stack_use_after_return=1:halt_on_error=1:handle_segv=0:handle_sigbus=0
# Use Clang's sanitizer runtime for the mixed-language final link.
export TSAN_OPTIONS=halt_on_error=1:exitcode=66:handle_segv=0:handle_sigbus=0
export RUSTFLAGS="-Zsanitizer=$sanitizer -Zexternal-clangrt -Cdebuginfo=1 -Cforce-frame-pointers=yes"
common=(-O1 -g -fno-omit-frame-pointer -fsanitize="$sanitizer" -Wall -Wextra -Werror -Iinclude -Inative)
for backend in linux host-runtime linear linear-heap; do
  cargo "+$rust" build -Zbuild-std=core,compiler_builtins --release --no-default-features \
    --features "$backend" --target "$triple" --target-dir "target/$sanitizer-$backend"
  lib="target/$sanitizer-$backend/$triple/release/libdotnet_pal_rs.a"
  if [[ "$backend" == linear-heap ]]; then
    clang -std=c11 "${common[@]}" tests/linear_heap.c "$lib" -lpthread -ldl -lm -o "$out/linear-heap"
    timeout 120s "$out/linear-heap"
  elif [[ "$backend" == linear ]]; then
    for suite in linear linear_threads; do
      clang -std=c11 "${common[@]}" "tests/$suite.c" "$lib" -Wl,--gc-sections -lpthread -ldl -lm -o "$out/$suite"
      timeout 120s "$out/$suite"
    done
    clang++ -std=c++17 "${common[@]}" tests/gc_linear.cpp "$lib" -Wl,--gc-sections -lpthread -ldl -lm -o "$out/gc-linear"
    timeout 120s "$out/gc-linear"
  else
    providers=()
    if [[ "$backend" == host-runtime ]]; then
      providers=(tests/host_backend.c tests/services_host.c tests/kernel_host.c tests/runtime_host.c)
    fi
    if [[ "$backend" == linux ]]; then
      clang++ -std=c++17 "${common[@]}" tests/unwind_lock.cpp "$lib" -lpthread -ldl -lm -o "$out/unwind-lock"
      timeout 120s "$out/unwind-lock"
    fi
    for suite in abi services kernel runtime; do
      clang -std=c11 "${common[@]}" "tests/$suite.c" "${providers[@]}" "$lib" \
        -Wl,--gc-sections -lpthread -ldl -lm -o "$out/$backend-$suite"
      timeout 120s "$out/$backend-$suite"
    done
  fi
done
echo "MIXED RUST/C $sanitizer SANITIZER PASS (boundary contracts; not .NET runtime instrumentation)"
