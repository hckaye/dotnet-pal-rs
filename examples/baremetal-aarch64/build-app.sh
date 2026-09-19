#!/usr/bin/env bash
# Builds a managed probe as a bootable AArch64 image and runs it under QEMU: the
# ILCompiler object for linux-arm64, the source-built NativeAOT runtime and minipal
# (artifacts/source-sdk), the boundary's System.Native, the freestanding C runtime
# and this port, linked with no OS and no libc. The argument names the sample:
# ConsoleProbe (the default), IoProbe, which exercises files on the in-memory file
# system and null references through the fault vector and expects sockets to be absent,
# SystemProbe, which exercises links, modes, times, locks and the working directory
# on that file system and expects processes, notifications and module loading to be absent,
# or FacilitiesProbe, which watches and maps files of the in-memory file system, reads it
# as a drive, and expects network information and local sockets to be absent.
# Run inside the Linux container from the repository root:
#   docker run --rm -v "$PWD:/work" -v dotnet-pal-nuget:/nuget -w /work dotnet-pal-baremetal \
#     bash examples/baremetal-aarch64/build-app.sh IoProbe
set -euo pipefail
sample="${1:-ConsoleProbe}"
case "$sample" in
  ConsoleProbe) pass='^CONSOLE PROBE PASS'; extra=();;
  IoProbe) pass='^IO PROBE PASS'; extra=(-p:ProbeExpect=files);;
  SystemProbe) pass='^SYSTEM PROBE PASS'; extra=('-p:ProbeExpect=links%3Bbaremetal');;
  FacilitiesProbe) pass='^FACILITIES PROBE PASS'; extra=('-p:ProbeExpect=watches%3Bmappings%3Bvolumes%3Bnoentropy');;
  *) echo 'usage: build-app.sh [ConsoleProbe|IoProbe|SystemProbe|FacilitiesProbe]' >&2; exit 2;;
esac
cd "$(dirname "$0")"
root="$(cd ../.. && pwd)"
here="$PWD"
clang="${CLANG:-clang}"; lld="${LD:-ld.lld}"; ar="${AR:-llvm-ar}"
sdk="$root/artifacts/source-sdk"
[[ -f "$sdk/libRuntime.WorkstationGC.a" && -f "$sdk/libaotminipal.a" ]] || { echo 'run scripts/source-runtime.sh first: the image links the source-built runtime and minipal' >&2; exit 1; }
out="$here/artifacts/app-$sample"
mkdir -p "$out"

# 1. The port, with its C math library and the CRT exit hooks.
cargo build --manifest-path Cargo.toml --target aarch64-unknown-none --release ${PAL_TRACE:+--features trace}

# 2. The freestanding C runtime contract (string, formatting, heap over the boundary, C++ ABI).
free="$root/crates/dotnet-pal-build/native/freestanding"
common=(--target=aarch64-unknown-none-elf -ffreestanding -fno-builtin -nostdlib -O2 -Wall -Wextra -Werror "-I$root/include" "-I$free")
objects=()
for unit in string strtol printf scanf crt heap; do
  "$clang" -std=c11 "${common[@]}" -c "$free/$unit.c" -o "$out/free-$unit.o"; objects+=("$out/free-$unit.o")
done
"$clang" -std=c++17 -fno-exceptions -fno-rtti -nostdinc++ "${common[@]}" -c "$free/cxxabi.cpp" -o "$out/free-cxxabi.o"; objects+=("$out/free-cxxabi.o")
rm -f "$out/libfreestanding.a"; "$ar" rcs "$out/libfreestanding.a" "${objects[@]}"

# 3. The BCL native layer over the boundary. It is compiled against the Linux
#    headers because the errno values and the runtime archive follow that ABI.
native=()
for unit in pal io net sys proc; do
  "$clang" -std=c11 -O2 -ffunction-sections -fdata-sections -fno-stack-protector -mno-outline-atomics -Wall -Wextra -Werror \
    "-I$root/include" "-I$root/crates/dotnet-pal-build/native" -c "$root/crates/dotnet-pal-build/native/system_native_$unit.c" -o "$out/system_native_$unit.o"
  native+=("$out/system_native_$unit.o")
done
rm -f "$out/libSystem.Native.a"; "$ar" rcs "$out/libSystem.Native.a" "${native[@]}"

# 4. The managed program compiled by ILCompiler for linux-arm64. The publish also
#    links a Linux executable we do not use; the object is what the image needs.
(cd "$root" && cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux >/dev/null)
framework="$out/native-overlay"
native_dir=$(dirname "$(readlink -f "$sdk/libSystem.Globalization.Native.a")")
rm -rf "$framework"; mkdir -p "$framework"; cp -as "$native_dir/." "$framework/"
rm "$framework/libSystem.Native.a"; cp "$out/libSystem.Native.a" "$framework/libSystem.Native.a"
rm -rf "$root/samples/$sample/obj" "$root/samples/$sample/bin"
(cd "$root" && dotnet publish "samples/$sample/$sample.csproj" -c Release -r linux-arm64 -p:PalSmallMachine=true "${extra[@]}" \
  "-p:IlcSdkPath=$sdk/" "-p:IlcFrameworkNativePath=$framework/" -o "$out/linux-publish" > "$out/publish.log" 2>&1) || { tail -30 "$out/publish.log"; exit 1; }
managed="$root/samples/$sample/obj/Release/net10.0/linux-arm64/native/$sample.o"
[[ -f "$managed" ]]

# 5. The image. libgcc supplies the outline atomics the runtime archive was built
#    with; its getauxval constructor stays out because the CRT defines the flag.
libgcc="$(ls /usr/lib/gcc/aarch64-linux-gnu/*/libgcc.a | head -n 1)"
# No RELRO: there is no loader to protect anything after relocation.
# Every input section must land inside the image: an orphan after __heap_start
# would be handed out again by the region allocator.
"$lld" -nostdlib -static -z norelro --gc-sections --eh-frame-hdr --fatal-warnings --orphan-handling=error --error-limit=0 -T link.ld -Map "$out/app.map" -o "$out/app.elf" \
  "$managed" "$sdk/libbootstrapper.o" "$sdk/libRuntime.WorkstationGC.a" "$sdk/libeventpipe-disabled.a" "$sdk/libstandalonegc-disabled.a" \
  "$sdk/libaotminipal.a" "$out/libSystem.Native.a" "$out/libfreestanding.a" \
  target/aarch64-unknown-none/release/libbaremetal_aarch64.a "$libgcc"
grep -q 'lse-init' "$out/app.map" && { echo 'libgcc lse-init.o (getauxval) was linked' >&2; exit 1; }
echo "linked $out/app.elf ($(stat -c %s "$out/app.elf") bytes)"

# 6. Run. The probe prints its PASS line and exits 0 through semihosting.
set +e
timeout 300 qemu-system-aarch64 -M virt -cpu cortex-a72 -m 1024 -nographic -semihosting -nic none -kernel "$out/app.elf" > "$out/qemu.log" 2>&1
status=$?
set -e
tail -n 40 "$out/qemu.log"
[[ $status == 0 ]] || { echo "qemu exit=$status" >&2; exit 1; }
grep -q "$pass" "$out/qemu.log"
echo "BARE-METAL MANAGED PASS: $sample ran under QEMU with no OS and no libc"
