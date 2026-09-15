#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -m)" in
  x86_64) rid=linux-x64 ;;
  aarch64) rid=linux-arm64 ;;
  *) echo "Only Linux x64/ARM64 are configured" >&2; exit 1 ;;
esac
[[ "$(uname -s)" == Linux ]]
[[ "$(dotnet --version)" == 10.0.100 ]] || { echo "Use the pinned SDK 10.0.100" >&2; exit 1; }
mkdir -p artifacts
clang++ -std=c++17 -O2 -fPIC -ffunction-sections -fdata-sections -Wall -Wextra -Werror \
  -Iinclude -Inative -c integration/dotnet10/gc_wrap.cpp -o artifacts/gc_wrap.o
python3 integration/dotnet10/symbols.py --props artifacts/wrap.props
project=samples/GcProbe/GcProbe.csproj
dotnet restore "$project" -r "$rid"
python3 integration/dotnet10/symbols.py --check-runtime \
  "${NUGET_PACKAGES:-$HOME/.nuget/packages}/runtime.$rid.microsoft.dotnet.ilcompiler/10.0.0/sdk"
cargo build --release
# Clear environment configuration that could silently select a different GC path.
export DOTNET_GCServer=0 DOTNET_GCLargePages=0
export COMPlus_gcServer=0 COMPlus_GCLargePages=0
export DOTNET_GCHeapHardLimit=0x20000000

# Baseline links the same observer and Rust archive but does NOT wrap any GC symbol.
dotnet publish "$project" -r "$rid" -c Release -p:PalWrap=false -o artifacts/baseline
./artifacts/baseline/GcProbe baseline
# Force relink: properties imported only by the linker are not reliable incremental inputs.
rm -rf samples/GcProbe/obj/Release samples/GcProbe/bin/Release
dotnet publish "$project" -r "$rid" -c Release -p:PalWrap=true -o artifacts/wrapped
./artifacts/wrapped/GcProbe wrapped

# Same managed program and same NativeAOT adapter, but replace Linux Rust backend
# with the no_std host-callback backend. Linux C callbacks stand in for an SDK here.
cargo build --release --no-default-features --features host --target-dir target/host
cc -std=c11 -O2 -fPIC -Iinclude -c tests/host_backend.c -o artifacts/host_backend.o
rm -rf samples/GcProbe/obj/Release samples/GcProbe/bin/Release
dotnet publish "$project" -r "$rid" -c Release -p:PalWrap=true \
  -p:PalLib="$PWD/target/host/release/libdotnet_pal_rs.a" \
  -p:PalHostObject="$PWD/artifacts/host_backend.o" -o artifacts/host-wrapped
./artifacts/host-wrapped/GcProbe wrapped
