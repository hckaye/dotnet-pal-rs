#!/usr/bin/env bash
# Explicit WASIp1 validation profile using the pinned experimental LLVM compiler.
# This overrides the compiler package's current WASIp2 default; imports are audited.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/llvm
: "${WASI_SDK_PATH:?provide the verified WASI SDK}"
: "${NUGET_PACKAGES:?provide an isolated package cache}"
export MSBuildEnableWorkloadResolver=false
project=samples/LlvmGcProbe/LlvmGcProbe.csproj
dotnet restore "$project" -r wasi-wasm
python3 integration/llvm-wasi/check_packages.py
dotnet publish "$project" -r wasi-wasm -c Release -p:IlcLlvmTarget=wasm32-unknown-wasip1 \
  -o artifacts/llvm/publish 2>&1 | tee artifacts/llvm/build.log
runtime=$(python3 -c 'from pathlib import Path; print(Path("artifacts/llvm/runtime-archive.txt").read_text(encoding="utf-8-sig").strip())')
"$WASI_SDK_PATH/bin/llvm-nm" --defined-only "$runtime" > artifacts/llvm/defined-symbols.txt
grep -E 'GCToOSInterface.*(Virtual|Performance|Stamp)' artifacts/llvm/defined-symbols.txt || true
node integration/llvm-wasi/run.mjs artifacts/llvm/publish/LlvmGcProbe.wasm | tee artifacts/llvm/run.log
