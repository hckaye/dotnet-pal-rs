#!/usr/bin/env bash
# Rebuild the LLVM native runtime and run C# with no --wrap indirection.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# == 1 ]] || { echo 'usage: llvm-source.sh /clean/pinned/runtimelab' >&2; exit 2; }
root="$PWD"; runtime=$(realpath "$1")
: "${WASI_SDK_PATH:?}"
: "${NUGET_PACKAGES:?}"
mkdir -p artifacts/llvm
python3 integration/llvm-wasi/patch_runtime.py "$runtime" --check
python3 integration/llvm-wasi/patch_runtime.py "$runtime"
if ! "$runtime/src/coreclr/build-runtime.sh" -release -os wasi -arch wasm -component nativeaot -ninja \
  -cmakeargs "-DDOTNET_PAL_ROOT=$root" > artifacts/llvm/source-build.log 2>&1; then
  tail -n 100 artifacts/llvm/source-build.log; exit 1
fi
tail -n 12 artifacts/llvm/source-build.log
mapfile -t archives < <(find "$runtime/artifacts/bin/coreclr" -name libPortableRuntime.a -type f)
[[ ${#archives[@]} == 1 ]] || { echo 'Expected exactly one rebuilt LLVM runtime' >&2; exit 1; }
rebuilt=${archives[0]}
"$WASI_SDK_PATH/bin/llvm-nm" -u "$rebuilt" > artifacts/llvm/source-undefined.txt
grep -q 'dotnet_pal_get_api' artifacts/llvm/source-undefined.txt
original=$(python3 -c 'from pathlib import Path; print(Path("artifacts/llvm/runtime-archive.txt").read_text(encoding="utf-8-sig").strip())')
overlay="$root/artifacts/llvm/source-sdk"
[[ ! -e "$overlay" ]] || { echo 'source SDK overlay already exists' >&2; exit 1; }
mkdir -p "$overlay"
cp -as "$(dirname "$original")/." "$overlay/"
rm "$overlay/libPortableRuntime.a"
cp "$rebuilt" "$overlay/libPortableRuntime.a"
python3 - "$overlay/libPortableRuntime.a" <<'PY'
import hashlib, json, sys
from pathlib import Path
archive=Path(sys.argv[1])
manifest={'runtime_revision':'9954350a58ede8b8eaaeb24112ca4f1e78cc527c',
 'adapter':'dotnet-pal-llvm-linear-v1','archive_sha256':hashlib.sha256(archive.read_bytes()).hexdigest()}
Path('artifacts/llvm/source-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
PY
rm -rf samples/LlvmGcProbe/obj/Release samples/LlvmGcProbe/bin/Release
MSBuildEnableWorkloadResolver=false dotnet publish samples/LlvmGcProbe/LlvmGcProbe.csproj \
  -r wasi-wasm -c Release -p:IlcLlvmTarget=wasm32-unknown-wasip1 -p:PalWrap=false \
  "-p:IlcSdkPath=$overlay/" "-p:PalSourceManifest=$root/artifacts/llvm/source-manifest.json" \
  "-p:PalObserverObject=$root/artifacts/llvm/baseline.o" -o artifacts/llvm/source \
  2>&1 | tee artifacts/llvm/source-publish.log
"$WASI_SDK_PATH/bin/llvm-nm" artifacts/llvm/source/LlvmGcProbe.wasm > artifacts/llvm/source-symbols.txt
if grep -q '__wrap__ZN15GCToOSInterface' artifacts/llvm/source-symbols.txt; then
  echo 'unexpected --wrap helpers in source configuration' >&2; exit 1
fi
timeout 120s node integration/llvm-wasi/run.mjs artifacts/llvm/source/LlvmGcProbe.wasm source 2>&1 \
  | tee artifacts/llvm/source-run.log
echo 'LLVM SOURCE RUNTIME PASS (compiled native runtime; no linker wrapping)'
