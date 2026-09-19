#!/usr/bin/env bash
# The boundary's System.Native as an archive for a Linux publish:
# artifacts/system-native/libSystem.Native.a from native/system_native_{pal,io,net,sys,proc}.c.
# This archive is for an executable/initial-load image, not a late-loaded DSO.
# Match source-runtime.sh: static TLS access must not call __tls_get_addr.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/system-native
objects=()
for unit in pal io net sys proc; do
  clang -std=c11 -O2 -fPIC -ftls-model=initial-exec -ffunction-sections -fdata-sections -Wall -Wextra -Werror -Iinclude -Inative \
    -c "native/system_native_$unit.c" -o "artifacts/system-native/system_native_$unit.o"
  objects+=("artifacts/system-native/system_native_$unit.o")
done
rm -f artifacts/system-native/libSystem.Native.a
ar rcs artifacts/system-native/libSystem.Native.a "${objects[@]}"
