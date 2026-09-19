#!/usr/bin/env bash
# Execute a managed source-rebuilt WASIp1 profile with ONE physical host import.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
: "${WASI_SDK_PATH:?}"
[[ -f artifacts/llvm/source-manifest.json && -f artifacts/llvm/source-sdk/libPortableRuntime.a ]]
bash scripts/wasi-bridge.sh
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features wasi-dispatch,linear-gc-small --target wasm32-wasip1 --target-dir target/wasi-isolated
"$WASI_SDK_PATH/bin/clang" --target=wasm32-unknown-wasip1 -std=c11 -O2 -ffunction-sections -fdata-sections   -Wall -Wextra -Werror -Iinclude -c crates/dotnet-pal-build/native/wasi_bridge.c -o artifacts/llvm/wasi_bridge.o
rm -rf samples/LlvmGcProbe/obj/Release samples/LlvmGcProbe/bin/Release
MSBuildEnableWorkloadResolver=false dotnet publish samples/LlvmGcProbe/LlvmGcProbe.csproj   -r wasi-wasm -c Release -p:IlcLlvmTarget=wasm32-unknown-wasip1 -p:PalWrap=false \
  "-p:IlcFrameworkPath=$root/artifacts/llvm/bcl/"   "-p:IlcSdkPath=$root/artifacts/llvm/source-sdk/" "-p:PalSourceManifest=$root/artifacts/llvm/source-manifest.json"   "-p:PalObserverObject=$root/artifacts/llvm/baseline.o" "-p:PalBridgeObject=$root/artifacts/llvm/wasi_bridge.o"   "-p:PalLib=$root/target/wasi-isolated/wasm32-wasip1/release/libdotnet_pal_standalone.a" -o artifacts/llvm/isolated   2>&1 | tee artifacts/llvm/isolated-publish.log
"$WASI_SDK_PATH/bin/llvm-nm" artifacts/llvm/isolated/LlvmGcProbe.wasm > artifacts/llvm/isolated-symbols.txt
if grep -q '__wrap_' artifacts/llvm/isolated-symbols.txt; then echo 'isolated profile must not use linker wrapping' >&2; exit 1; fi
timeout 120s node integration/llvm-wasi/run.mjs artifacts/llvm/isolated/LlvmGcProbe.wasm isolated 2>&1 | tee artifacts/llvm/isolated-run.log
echo 'MANAGED OS ISOLATION PASS single Rust host boundary for runtime AND linked BCL OS imports (WASIp1 profile)'
