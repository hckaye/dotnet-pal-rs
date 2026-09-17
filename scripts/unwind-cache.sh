#!/usr/bin/env bash
# Requires the exact source checkout already patched by patch_runtime.py.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# == 1 ]] || { echo 'usage: unwind-cache.sh /patched/runtime' >&2; exit 2; }
u=$(realpath "$1")/src/native/external/llvm-libunwind
mkdir -p artifacts/support
cc -std=c11 -O2 -Wall -Wextra -Werror -Iinclude -c native/support_posix.c -o artifacts/support/posix.o
c++ -std=c++17 -O2 -DDOTNET_PAL_SUPPORT=1 -D_LIBUNWIND_IS_NATIVE_ONLY -Iinclude -Inative \
  -I"$u/include" -I"$u/src" tests/unwind_cache.cpp artifacts/support/posix.o -lpthread -ldl -o artifacts/support/unwind-cache
python3 -c 'import subprocess;subprocess.run(["artifacts/support/unwind-cache"],check=True,timeout=60)'
