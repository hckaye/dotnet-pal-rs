#!/usr/bin/env bash
# Rebuild the NativeAOT native runtime at the audited pin, then execute WITHOUT --wrap.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
[[ $# == 1 ]] || { echo "usage: bash scripts/source-runtime.sh /clean/dotnet/runtime" >&2; exit 2; }
runtime=$(realpath "$1")
case "$(uname -m)" in x86_64) arch=x64;; aarch64) arch=arm64;; *) exit 2;; esac
[[ "$(uname -s)" == Linux ]]
mkdir -p artifacts
python3 integration/dotnet10/patch_runtime.py "$runtime" --check
python3 integration/dotnet10/patch_runtime.py "$runtime"
# Build the complete native NativeAOT component, not the compiler/BCL or CoreCLR.
# Published compiler/BCL and rebuilt native runtime are both pinned to v10.0.0.
"$runtime/src/coreclr/build-runtime.sh" -release -arch "$arch" -component nativeaot -ninja \
  -cmakeargs "-DDOTNET_PAL_ROOT=$root" 2>&1 | tee artifacts/source-build.log
mapfile -t archives < <(find "$runtime/artifacts/bin/coreclr" -name libRuntime.WorkstationGC.a -type f)
[[ ${#archives[@]} == 1 ]] || { printf 'Expected one rebuilt archive, got %s\n' "${archives[*]}" >&2; exit 1; }
rebuilt=${archives[0]}
# Independent proof that the archive calls our boundary, not just that the patch applied.
nm -u "$rebuilt" > artifacts/source-undefined.txt
 grep -q 'dotnet_pal_get_api' artifacts/source-undefined.txt
bash scripts/nativeaot.sh
# nativeaot.sh records the actual published SDK path selected by MSBuild.
original=$(cat artifacts/runtime-archive.txt)
overlay="$root/artifacts/source-sdk"
mkdir -p "$overlay"
# Fresh destination required; no modifications to the NuGet cache or source checkout.
[[ ! -e "$overlay/libRuntime.WorkstationGC.a" ]] || { echo 'Remove artifacts/source-sdk before rerunning' >&2; exit 1; }
cp -as "$(dirname "$original")/." "$overlay/"
rm "$overlay/libRuntime.WorkstationGC.a"
cp "$rebuilt" "$overlay/libRuntime.WorkstationGC.a"
clang++ -std=c++17 -O2 -fPIC -ffunction-sections -fdata-sections \
  -DDOTNET_PAL_OBSERVER_ONLY -Iinclude -Inative -c integration/dotnet10/gc_wrap.cpp -o artifacts/gc_observer.o
for backend in linux host; do
  pal="$root/target/release/libdotnet_pal_rs.a"
  host_args=()
  if [[ "$backend" == host ]]; then
    pal="$root/target/host/release/libdotnet_pal_rs.a"
    host_args=("-p:PalHostObject=$root/artifacts/host_backend.o")
  fi
  rm -rf samples/GcProbe/obj/Release samples/GcProbe/bin/Release
  dotnet publish samples/GcProbe/GcProbe.csproj -r "linux-$arch" -c Release \
    -p:PalWrap=false "-p:IlcSdkPath=$overlay/" "-p:PalLib=$pal" \
    "-p:PalObserverObject=$root/artifacts/gc_observer.o" "${host_args[@]}" \
    -o "artifacts/source-$backend"
  binary="artifacts/source-$backend/GcProbe"
  nm "$binary" > "artifacts/source-$backend-symbols.txt"
  if grep -q '__wrap__ZN15GCToOSInterface' "artifacts/source-$backend-symbols.txt"; then
    echo 'Unexpected --wrap helper in the source configuration' >&2; exit 1
  fi
  DOTNET_GCHeapHardLimit=0x20000000 DOTNET_GCServer=0 DOTNET_GCLargePages=0 \
    COMPlus_gcServer=0 COMPlus_GCLargePages=0 "$binary" wrapped
  echo "SOURCE RUNTIME PASS backend=$backend (no --wrap, native runtime rebuilt)"
done
