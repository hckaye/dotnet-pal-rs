#!/usr/bin/env bash
# Builds the shim for the host with every symbol prefixed (fs_) and runs
# selftest.c against glibc. Run inside the Linux arm64 container. Objects go to
# artifacts/freestanding/selftest/.
set -euo pipefail
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$dir/../../../.." && pwd)"
out="${FREESTANDING_OUT:-$root/artifacts/freestanding}/selftest"
mkdir -p "$out"
cc="${CC:-clang}"; cxx="${CXX:-clang++}"
common=(-ffreestanding -fno-builtin -O2 -Wall -Wextra -Werror -DFREESTANDING_PREFIX=fs_ "-I$root/include" "-I$dir")
objects=()
for unit in string strtol printf scanf crt heap; do
  "$cc" -std=c11 "${common[@]}" -c "$dir/$unit.c" -o "$out/$unit.o"
  objects+=("$out/$unit.o")
done
"$cxx" -std=c++17 -fno-exceptions -fno-rtti -nostdinc++ "${common[@]}" -DFREESTANDING_SELFTEST -c "$dir/cxxabi.cpp" -o "$out/cxxabi.o"
objects+=("$out/cxxabi.o")
"$cc" -std=c11 -O1 -Wall -Wextra -Werror -Wno-format -Wno-format-extra-args "-I$root/include" "-I$dir" -c "$dir/selftest.c" -o "$out/selftest.o"
"$cc" -o "$out/selftest" "$out/selftest.o" "${objects[@]}" -lm
"$out/selftest"
