#!/usr/bin/env bash
# Publish the console probe against the source-built runtime AND the boundary's
# System.Native (native/system_native_*.c) instead of the SDK's, run it, and
# show from the link map that every SystemNative_* symbol came from the boundary.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
case "$(uname -m)" in x86_64) rid=linux-x64;; aarch64) rid=linux-arm64;; *) exit 2;; esac
[[ "$(uname -s)" == Linux ]]
overlay="$root/artifacts/source-sdk"
[[ -f "$overlay/libRuntime.WorkstationGC.a" ]] || { echo 'run scripts/source-runtime.sh first: the console probe links the source-built runtime' >&2; exit 1; }
mkdir -p artifacts/console-probe
bash scripts/system-native.sh
# The framework native directory with only System.Native replaced.
native_dir=$(dirname "$(readlink -f "$overlay/libSystem.Globalization.Native.a")")
framework="$root/artifacts/console-probe/native"
rm -rf "$framework"; mkdir -p "$framework"
cp -as "$native_dir/." "$framework/"
rm "$framework/libSystem.Native.a"
cp artifacts/system-native/libSystem.Native.a "$framework/libSystem.Native.a"
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux
rm -rf samples/ConsoleProbe/obj samples/ConsoleProbe/bin
dotnet publish samples/ConsoleProbe/ConsoleProbe.csproj -c Release -r "$rid" \
  "-p:IlcSdkPath=$overlay/" "-p:IlcFrameworkNativePath=$framework/" \
  "-p:PalLinkMap=$root/artifacts/console-probe/link.map" -o artifacts/console-probe
CONSOLE_PROBE_VALUE=from-the-boundary timeout 120s artifacts/console-probe/ConsoleProbe alpha beta | tee artifacts/console-probe/run.log
grep -q '^CONSOLE PROBE PASS$' artifacts/console-probe/run.log
grep -q 'environment CONSOLE_PROBE_VALUE=from-the-boundary' artifacts/console-probe/run.log
# Every SystemNative_ definition in the image comes from the boundary's object.
python3 - <<'PY'
import re
from pathlib import Path
text = Path('artifacts/console-probe/link.map').read_text(errors='replace')
# GNU ld map: an input section line names the symbol, the next line names the object that supplied it.
providers = {name: origin for name, origin in re.findall(r'\.text\.(SystemNative_\w+)\s*\n\s*0x[0-9a-f]+\s+0x[0-9a-f]+\s+(\S+)', text)}
if not providers: raise SystemExit('no SystemNative sections in the link map: the map format changed')
foreign = sorted(name for name, origin in providers.items() if not re.search(r'libSystem\.Native\.a\(system_native_(pal|io|net|sys|proc)\.o\)', origin))
print(f"SystemNative symbols linked: {len(providers)}; from the boundary objects: {len(providers) - len(foreign)}")
if foreign: raise SystemExit('SystemNative symbols not provided by native/system_native_*.c: ' + ', '.join(foreign))
PY
python3 scripts/audit_dependencies.py --runtime "$overlay/libRuntime.WorkstationGC.a" --runtime "$overlay/libaotminipal.a" \
  --runtime artifacts/system-native/libSystem.Native.a --pal target/release/libdotnet_pal_standalone.a \
  --binary artifacts/console-probe/ConsoleProbe --output artifacts/console-probe/dependency-inventory.log --require-isolated
echo "CONSOLE PROBE PASS rid=$rid: runtime, minipal and System.Native all reach the OS only through the boundary"
