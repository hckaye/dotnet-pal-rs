#!/usr/bin/env bash
# Publish the facilities probe (FileSystemWatcher, MemoryMappedFile, DriveInfo,
# NetworkInterface, reverse lookup, multicast and the keep-alive socket options)
# against the source-built runtime and the boundary's System.Native, run it on
# Linux, and show from the link map that every SystemNative_* symbol came from
# the boundary, including the entry points the probe reaches.
# PROBE_EXPECT overrides what the probe expects the port to provide.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
expect="${PROBE_EXPECT-watches;mappings;volumes;network;local;packets}"
case "$(uname -m)" in x86_64) rid=linux-x64;; aarch64) rid=linux-arm64;; *) exit 2;; esac
[[ "$(uname -s)" == Linux ]]
overlay="$root/artifacts/source-sdk"
[[ -f "$overlay/libRuntime.WorkstationGC.a" ]] || { echo 'run scripts/source-runtime.sh first: the facilities probe links the source-built runtime' >&2; exit 1; }
mkdir -p artifacts/facilities-probe
bash scripts/system-native.sh
# The framework native directory with only System.Native replaced.
native_dir=$(dirname "$(readlink -f "$overlay/libSystem.Globalization.Native.a")")
framework="$root/artifacts/facilities-probe/native"
rm -rf "$framework"; mkdir -p "$framework"
cp -as "$native_dir/." "$framework/"
rm "$framework/libSystem.Native.a"
cp artifacts/system-native/libSystem.Native.a "$framework/libSystem.Native.a"
cargo rustc --lib --crate-type staticlib --release --features linux
rm -rf samples/FacilitiesProbe/obj samples/FacilitiesProbe/bin
dotnet publish samples/FacilitiesProbe/FacilitiesProbe.csproj -c Release -r "$rid" "-p:ProbeExpect=${expect//;/%3B}" \
  "-p:IlcSdkPath=$overlay/" "-p:IlcFrameworkNativePath=$framework/" \
  "-p:PalLinkMap=$root/artifacts/facilities-probe/link.map" -o artifacts/facilities-probe
# A custom ICMP payload needs a raw socket; an unprivileged Ping may fall back
# to the system ping utility, which cannot preserve this test's payload. CI opts
# in to a single file capability rather than running the build/probe as root.
case "${PROBE_GRANT_NET_RAW:-0}" in
  0) ;;
  1)
    command -v setcap >/dev/null || { echo 'install libcap2-bin for PROBE_GRANT_NET_RAW=1' >&2; exit 1; }
    sudo -n setcap cap_net_raw=ep artifacts/facilities-probe/FacilitiesProbe
    trap 'sudo -n setcap -r artifacts/facilities-probe/FacilitiesProbe' EXIT
    ;;
  *) echo 'PROBE_GRANT_NET_RAW must be 0 or 1' >&2; exit 2 ;;
esac
timeout 300s artifacts/facilities-probe/FacilitiesProbe | tee artifacts/facilities-probe/run.log
grep -q '^FACILITIES PROBE PASS$' artifacts/facilities-probe/run.log
PROBE_GROUPS="$expect" python3 - <<'PY'
import re
from pathlib import Path
text = Path('artifacts/facilities-probe/link.map').read_text(errors='replace')
# GNU ld map: an input section line names the symbol, the next line names the object that supplied it.
providers = {name: origin for name, origin in re.findall(r'\.text\.(SystemNative_\w+)\s*\n\s*0x[0-9a-f]+\s+0x[0-9a-f]+\s+(\S+)', text)}
if not providers: raise SystemExit('no SystemNative sections in the link map: the map format changed')
foreign = sorted(name for name, origin in providers.items() if not re.search(r'libSystem\.Native\.a\(system_native_(pal|io|net|sys|proc)\.o\)', origin))
print(f"SystemNative symbols linked: {len(providers)}; from the boundary objects: {len(providers) - len(foreign)}")
if foreign: raise SystemExit('SystemNative symbols not provided by native/system_native_*.c: ' + ', '.join(foreign))
import os
reached = {'watches': ('SystemNative_INotifyInit', 'SystemNative_INotifyAddWatch', 'SystemNative_INotifyRemoveWatch'),
           'mappings': ('SystemNative_MMap', 'SystemNative_MUnmap', 'SystemNative_MSync'),
           'volumes': ('SystemNative_GetSpaceInfoForMountPoint', 'SystemNative_GetFileSystemTypeNameForMountPoint'),
           'network': ('SystemNative_GetNetworkInterfaces', 'SystemNative_GetNameInfo', 'SystemNative_SetIPv4MulticastOption'),
           'local': ('SystemNative_GetPeerID', 'SystemNative_GetDomainSocketSizes'),
           'packets': ('SystemNative_TryGetIPPacketInformation', 'SystemNative_ReceiveMessage')}
for needed in [name for group in os.environ['PROBE_GROUPS'].split(';') if group for name in reached.get(group, ())]:
    if needed not in providers: raise SystemExit(needed + ' is not in the image: the probe no longer reaches it')
PY
python3 scripts/audit_dependencies.py --runtime "$overlay/libRuntime.WorkstationGC.a" --runtime "$overlay/libaotminipal.a" \
  --runtime artifacts/system-native/libSystem.Native.a --pal target/release/libdotnet_pal_rs.a \
  --binary artifacts/facilities-probe/FacilitiesProbe --output artifacts/facilities-probe/dependency-inventory.log --require-isolated
echo "FACILITIES PROBE PASS rid=$rid expect='$expect': change watching, mapped files, volumes and network information of the BCL reach the OS only through the boundary"
