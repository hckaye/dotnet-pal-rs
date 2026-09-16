#!/usr/bin/env bash
# Source-rebuilt managed execution with demand-backed, budgeted linear storage.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
: "${WASI_SDK_PATH:?}"
[[ -f artifacts/llvm/source-manifest.json && -f artifacts/llvm/source-sdk/libPortableRuntime.a ]]
cargo build --release --no-default-features --features wasi-dispatch,linear-gc-small,linear-heap \
  --target wasm32-wasip1 --target-dir target/wasi-dynamic
"$WASI_SDK_PATH/bin/clang" --target=wasm32-unknown-wasip1 -std=c11 -O2 -ffunction-sections -fdata-sections \
  -Wall -Wextra -Werror -Iinclude -c native/linear_heap_posix.c -o artifacts/llvm/linear_heap.o
rm -rf samples/LlvmGcProbe/obj/Release samples/LlvmGcProbe/bin/Release
MSBuildEnableWorkloadResolver=false dotnet publish samples/LlvmGcProbe/LlvmGcProbe.csproj \
  -r wasi-wasm -c Release -p:IlcLlvmTarget=wasm32-unknown-wasip1 -p:PalWrap=false \
  "-p:IlcFrameworkPath=$root/artifacts/llvm/bcl/" "-p:IlcSdkPath=$root/artifacts/llvm/source-sdk/" \
  "-p:PalSourceManifest=$root/artifacts/llvm/source-manifest.json" \
  "-p:PalObserverObject=$root/artifacts/llvm/baseline.o" "-p:PalBridgeObject=$root/artifacts/llvm/wasi_bridge.o" \
  "-p:PalHeapObject=$root/artifacts/llvm/linear_heap.o" \
  "-p:PalLib=$root/target/wasi-dynamic/wasm32-wasip1/release/libdotnet_pal_rs.a" \
  -o artifacts/llvm/dynamic 2>&1 | tee artifacts/llvm/dynamic-publish.log
"$WASI_SDK_PATH/bin/llvm-nm" artifacts/llvm/dynamic/LlvmGcProbe.wasm > artifacts/llvm/dynamic-symbols.txt
if grep -q '__wrap_' artifacts/llvm/dynamic-symbols.txt; then echo 'unexpected linker wrapping' >&2; exit 1; fi
PAL_REQUIRE_MEMORY_GROWTH=1 timeout 120s node integration/llvm-wasi/run.mjs \
  artifacts/llvm/dynamic/LlvmGcProbe.wasm isolated 2>&1 | tee artifacts/llvm/dynamic-run.log
echo 'DYNAMIC MANAGED WASI PASS demand-backed GC/BCL storage, real memory growth, one audited OS import'
