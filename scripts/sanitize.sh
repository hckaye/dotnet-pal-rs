#!/usr/bin/env bash
# Instrument BOTH Rust and C/C++ boundary code. This is not a sanitized .NET build.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# == 1 && ( "$1" == address || "$1" == thread ) ]] || { echo 'usage: sanitize.sh address|thread' >&2; exit 2; }
sanitizer=$1
# The machine's own architecture: the sanitizer runtimes exist for x86-64 and AArch64 alike.
triple="$(uname -m)-unknown-linux-gnu"
rust=nightly-2026-09-01
out="$PWD/artifacts/$sanitizer"
mkdir -p "$out"
ulimit -c 0
export ASAN_OPTIONS=detect_leaks=1:detect_stack_use_after_return=1:halt_on_error=1:handle_segv=0:handle_sigbus=0
# Use Clang's sanitizer runtime for the mixed-language final link.
# The priority test starts threads in a forked child of a threaded parent, which the thread sanitizer refuses by default.
export TSAN_OPTIONS=halt_on_error=1:exitcode=66:handle_segv=0:handle_sigbus=0:die_after_fork=0
export RUSTFLAGS="-Zsanitizer=$sanitizer -Zexternal-clangrt -Cdebuginfo=1 -Cforce-frame-pointers=yes"
common=(-O1 -g -fno-omit-frame-pointer -fsanitize="$sanitizer" -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native)
for backend in linux host-runtime host-support linear linear-heap; do
  cargo "+$rust" rustc -p dotnet-pal-standalone -Zbuild-std=core,compiler_builtins --lib --crate-type staticlib --release --no-default-features \
    --features "$backend" --target "$triple" --target-dir "target/$sanitizer-$backend"
  lib="target/$sanitizer-$backend/$triple/release/libdotnet_pal_standalone.a"
  if [[ "$backend" == host-support ]]; then
    clang -std=c11 "${common[@]}" tests/support.c crates/dotnet-pal-posix/native/support_posix.c tests/host_backend.c "$lib" -Wl,--gc-sections -lpthread -ldl -lm -o "$out/support-host"
    timeout 120s "$out/support-host"
    clang -std=c11 "${common[@]}" tests/support_faults.c tests/host_backend.c "$lib" -Wl,--gc-sections -lpthread -ldl -lm -o "$out/support-faults"
    for mode in {1..12}; do timeout 30s "$out/support-faults" "$mode"; done
  elif [[ "$backend" == linear-heap ]]; then
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
    suites=(abi services kernel runtime)
    # The Linux providers of the groups behind System.Native run their conformance tests instrumented as well. Each test
    # compares the provider with the kernel, so every provider path is executed.
    if [[ "$backend" == linux ]]; then suites+=(support topology process image streams files sockets system notifications processes terminal watches mappings volumes network local_sockets accounts priority packets spawn_as); fi
    for suite in "${suites[@]}"; do
      clang -std=c11 "${common[@]}" "tests/$suite.c" "${providers[@]}" "$lib" \
        -Wl,--gc-sections -Wl,--build-id=sha1 -lpthread -ldl -lm -o "$out/$backend-$suite"
      timeout 120s "$out/$backend-$suite"
    done
  fi
done
clang++ -std=c++17 "${common[@]}" tests/thread_identity.cpp -pthread -o "$out/thread-identity"
timeout 60s "$out/thread-identity"
echo "MIXED RUST/C $sanitizer SANITIZER PASS (boundary contracts; not .NET runtime instrumentation)"
