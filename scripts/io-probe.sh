#!/usr/bin/env bash
# Publish the I/O probe (files, sockets, hardware faults) against the source-built
# runtime and the boundary's System.Native, run it on Linux, and show from the link
# map that every SystemNative_* symbol came from the boundary, including the file
# and socket entry points the probe reaches.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
case "$(uname -m)" in x86_64) rid=linux-x64;; aarch64) rid=linux-arm64;; *) exit 2;; esac
[[ "$(uname -s)" == Linux ]]
overlay="$root/artifacts/source-sdk"
[[ -f "$overlay/libRuntime.WorkstationGC.a" ]] || { echo 'run scripts/source-runtime.sh first: the I/O probe links the source-built runtime' >&2; exit 1; }
mkdir -p artifacts/io-probe
bash scripts/system-native.sh
# The framework native directory with only System.Native replaced.
native_dir=$(dirname "$(readlink -f "$overlay/libSystem.Globalization.Native.a")")
framework="$root/artifacts/io-probe/native"
rm -rf "$framework"; mkdir -p "$framework"
cp -as "$native_dir/." "$framework/"
rm "$framework/libSystem.Native.a"
cp artifacts/system-native/libSystem.Native.a "$framework/libSystem.Native.a"
cargo rustc --lib --crate-type staticlib --release --features linux
rm -rf samples/IoProbe/obj samples/IoProbe/bin
dotnet publish samples/IoProbe/IoProbe.csproj -c Release -r "$rid" '-p:ProbeExpect=files%3Bsockets' \
  "-p:IlcSdkPath=$overlay/" "-p:IlcFrameworkNativePath=$framework/" \
  "-p:PalLinkMap=$root/artifacts/io-probe/link.map" -o artifacts/io-probe
timeout 300s artifacts/io-probe/IoProbe | tee artifacts/io-probe/run.log
grep -q '^IO PROBE PASS$' artifacts/io-probe/run.log
python3 - <<'PY'
import re
from pathlib import Path
text = Path('artifacts/io-probe/link.map').read_text(errors='replace')
# GNU ld map: an input section line names the symbol, the next line names the object that supplied it.
providers = {name: origin for name, origin in re.findall(r'\.text\.(SystemNative_\w+)\s*\n\s*0x[0-9a-f]+\s+0x[0-9a-f]+\s+(\S+)', text)}
if not providers: raise SystemExit('no SystemNative sections in the link map: the map format changed')
foreign = sorted(name for name, origin in providers.items() if not re.search(r'libSystem\.Native\.a\(system_native_(pal|io|net|sys|proc)\.o\)', origin))
print(f"SystemNative symbols linked: {len(providers)}; from the boundary objects: {len(providers) - len(foreign)}")
if foreign: raise SystemExit('SystemNative symbols not provided by native/system_native_*.c: ' + ', '.join(foreign))
for needed in ('SystemNative_Open', 'SystemNative_PWrite', 'SystemNative_ReadDir', 'SystemNative_Socket', 'SystemNative_WaitForSocketEvents'):
    if needed not in providers: raise SystemExit(needed + ' is not in the image: the probe no longer reaches it')
PY
python3 scripts/audit_dependencies.py --runtime "$overlay/libRuntime.WorkstationGC.a" --runtime "$overlay/libaotminipal.a" \
  --runtime artifacts/system-native/libSystem.Native.a --pal target/release/libdotnet_pal_rs.a \
  --binary artifacts/io-probe/IoProbe --output artifacts/io-probe/dependency-inventory.log --require-isolated
echo "IO PROBE PASS rid=$rid: files, sockets and hardware faults of the BCL reach the OS only through the boundary"
