#!/usr/bin/env bash
# Builds the port archive, compiles the freestanding C/C++ test and links the
# bootable image. No libc and no CRT are linked: the only definitions in the
# image are the port's, the test's, and the compiler builtins Rust ships.
set -euo pipefail
cd "$(dirname "$0")"
root="$(cd ../.. && pwd)"
clang="${CLANG:-clang}"
lld="${LD:-ld.lld}"
mkdir -p artifacts

cargo build --manifest-path Cargo.toml --target aarch64-unknown-none --release

flags=(--target=aarch64-unknown-none-elf -ffreestanding -fno-builtin -nostdlib -O2 -march=armv8-a
       -mno-outline-atomics -fno-pic -ftls-model=local-exec -fno-stack-protector
       -Wall -Wextra -Werror "-I$root/include")
"$clang" "${flags[@]}" -std=c11 -c tests/table.c -o artifacts/table.o
"$clang" "${flags[@]}" -std=c++17 -fno-exceptions -fno-rtti -c tests/ctor.cpp -o artifacts/ctor.o
"$clang" "${flags[@]}" -c tests/registers.S -o artifacts/registers.o

# libgcc supplies the outline atomics and integer helpers the compiler may still
# call for; it is used only when the image is left with such an undefined symbol.
extra=()
if [[ -n "${LIBGCC:-}" ]]; then
    extra+=("$LIBGCC")
fi
"$lld" -nostdlib -static -z norelro --gc-sections --eh-frame-hdr --fatal-warnings --orphan-handling=error -T link.ld \
    -Map artifacts/table.map -o artifacts/table.elf \
    artifacts/table.o artifacts/ctor.o artifacts/registers.o \
    target/aarch64-unknown-none/release/libbaremetal_aarch64.a "${extra[@]}"
echo "linked artifacts/table.elf"
