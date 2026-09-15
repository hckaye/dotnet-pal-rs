#!/usr/bin/env bash
# Pinned LLVM/WASIp1 GC-to-Rust integration with a zero-call negative control.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
mkdir -p artifacts/llvm
exec > >(tee artifacts/llvm/driver.log) 2>&1
: "${WASI_SDK_PATH:?provide the verified WASI SDK}"
: "${NUGET_PACKAGES:?provide an isolated package cache}"
export MSBuildEnableWorkloadResolver=false
project=samples/LlvmGcProbe/LlvmGcProbe.csproj
cargo build --release --no-default-features --features linear-gc,wasi-clock --target wasm32-wasip1
cc="$WASI_SDK_PATH/bin/clang"; cxx="$WASI_SDK_PATH/bin/clang++"
# This PURE formatter must use the P2 SDK's error constants, matching the published
# System.Native archive. It performs no OS calls or P2 imports. The P1 SDK has no
# netdb.h. The final module's import audit still rejects every non-P1 dependency.
"$cc" --target=wasm32-unknown-wasip2 -std=c11 -O2 -Wall -Wextra -Werror -c integration/llvm-wasi/p1_error_text.c -o artifacts/llvm/p1_error_text.o
for mode in baseline wrapped; do
  extra=()
  [[ "$mode" != baseline ]] || extra=(-DDOTNET_PAL_OBSERVER_ONLY)
  "$cxx" --target=wasm32-unknown-wasip1 -std=c++17 -O2 -fno-exceptions -fno-rtti \
    -ffunction-sections -fdata-sections -Wall -Wextra -Werror -Iinclude -Inative \
    "${extra[@]}" -c integration/llvm-wasi/gc_linear_wrap.cpp -o "artifacts/llvm/$mode.o"
done
python3 integration/llvm-wasi/symbols.py --wrapper artifacts/llvm/wrapped.o --props artifacts/llvm/wrap.props --nm "$WASI_SDK_PATH/bin/llvm-nm"
dotnet restore "$project" -r wasi-wasm
python3 integration/llvm-wasi/check_packages.py
for mode in baseline wrapped; do
  wrapping=false
  [[ "$mode" != wrapped ]] || wrapping=true
  rm -rf samples/LlvmGcProbe/obj/Release samples/LlvmGcProbe/bin/Release
  dotnet publish "$project" -r wasi-wasm -c Release -p:IlcLlvmTarget=wasm32-unknown-wasip1 \
    "-p:PalWrap=$wrapping" "-p:PalObserverObject=$root/artifacts/llvm/$mode.o" \
    -o "artifacts/llvm/$mode" 2>&1 | tee "artifacts/llvm/$mode-build.log"
  timeout 120s node integration/llvm-wasi/run.mjs "artifacts/llvm/$mode/LlvmGcProbe.wasm" "$mode" \
    | tee "artifacts/llvm/$mode-run.log"
done
