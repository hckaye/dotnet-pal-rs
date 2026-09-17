#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -m)" in
  x86_64) rid=linux-x64 ;;
  aarch64) rid=linux-arm64 ;;
  *) echo "Only Linux x64/ARM64 are configured" >&2; exit 1 ;;
esac
[[ "$(uname -s)" == Linux ]]
[[ "$(dotnet --version)" == 10.0.401 ]] || { echo "Use the pinned SDK 10.0.401" >&2; exit 1; }
mkdir -p artifacts
clang++ -std=c++17 -O2 -fPIC -ffunction-sections -fdata-sections -Wall -Wextra -Werror \
  -Iinclude -Inative -c integration/dotnet10/gc_wrap.cpp -o artifacts/gc_wrap.o
python3 integration/dotnet10/symbols.py --props artifacts/wrap.props
project=samples/GcProbe/GcProbe.csproj
dotnet restore "$project" -r "$rid"
cargo rustc --lib --crate-type staticlib --release --features linux
export DOTNET_GCServer=0 DOTNET_gcServer=0 DOTNET_GCLargePages=0
export COMPlus_gcServer=0 COMPlus_GCLargePages=0
# RhConfig at the pinned NativeAOT revision accepts hex digits, NOT a 0x prefix.
# Scope limits to the tested processes; do not constrain the compiler heap.
dotnet publish "$project" -r "$rid" -c Release -p:PalWrap=false -o artifacts/baseline
DOTNET_GCHeapHardLimit=20000000 timeout 120s ./artifacts/baseline/GcProbe baseline
rm -rf samples/GcProbe/obj/Release samples/GcProbe/bin/Release
dotnet publish "$project" -r "$rid" -c Release -p:PalWrap=true -o artifacts/wrapped
DOTNET_GCHeapHardLimit=20000000 timeout 120s ./artifacts/wrapped/GcProbe wrapped
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host --target-dir target/host
cc -std=c11 -O2 -fPIC -Iinclude -c tests/host_backend.c -o artifacts/host_backend.o
rm -rf samples/GcProbe/obj/Release samples/GcProbe/bin/Release
dotnet publish "$project" -r "$rid" -c Release -p:PalWrap=true \
  -p:PalLib="$PWD/target/host/release/libdotnet_pal_rs.a" \
  -p:PalHostObject="$PWD/artifacts/host_backend.o" -o artifacts/host-wrapped
DOTNET_GCHeapHardLimit=20000000 timeout 120s ./artifacts/host-wrapped/GcProbe wrapped
