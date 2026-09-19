#!/usr/bin/env bash
# Execute exact baseline/source configurations; skipped tests are not PASS.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# == 2 ]] || { echo 'usage: qualify.sh source-sdk source-manifest' >&2; exit 2; }
root="$PWD"; overlay=$(realpath "$1"); manifest=$(realpath "$2")
case "$(uname -m)" in x86_64) rid=linux-x64;; aarch64) rid=linux-arm64;; *) exit 2;; esac
mkdir -p artifacts/qualification
ulimit -c 0
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -c tests/qualification_native.c -o artifacts/qualification_native.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -DPAL_FAULT_PROVIDER -c tests/qualification_native.c -o artifacts/qualification_fault_native.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c tests/host_fault_backend.c -o artifacts/host_fault.o
clang -r artifacts/support_host.o artifacts/host_fault.o artifacts/services_host.o artifacts/kernel_host.o artifacts/runtime_host.o artifacts/context_host.o \
  artifacts/topology_host.o artifacts/process_host.o artifacts/image_host.o artifacts/streams_host.o -o artifacts/host_fault_all.o
export DOTNET_GCLargePages=0 COMPlus_GCLargePages=0
# RhConfig parses hex digits only; 0x-prefixed values are rejected silently upstream.
# Apply the cap to child probes only, not the compiler process.
export DOTNET_GCHeapCount=2 COMPlus_GCHeapCount=2
for profile in workstation server; do
  server=false; gc=0
  if [[ "$profile" == server ]]; then server=true; gc=1; fi
  export DOTNET_GCServer="$gc" DOTNET_gcServer="$gc" COMPlus_gcServer="$gc"
  for backend in baseline linux host fault; do
    output="$root/artifacts/qualification/$profile-$backend"
    args=("-p:PalLib=$root/target/release/libdotnet_pal_standalone.a"
          "-p:PalQualificationObject=$root/artifacts/qualification_native.o")
    if [[ "$backend" != baseline ]]; then
      args+=("-p:IlcSdkPath=$overlay/" "-p:PalSourceManifest=$manifest")
    fi
    if [[ "$backend" == host || "$backend" == fault ]]; then
      args+=("-p:PalLib=$root/target/host-kernel/release/libdotnet_pal_standalone.a")
      if [[ "$backend" == fault ]]; then
        args+=("-p:PalHostObject=$root/artifacts/host_fault_all.o"
               "-p:PalQualificationObject=$root/artifacts/qualification_fault_native.o")
      else
        args+=("-p:PalHostObject=$root/artifacts/host_services_backend.o")
      fi
    fi
    rm -rf samples/GcProbe/obj/Release samples/GcProbe/bin/Release
    dotnet publish samples/GcProbe/GcProbe.csproj -c Release -r "$rid" -p:PalWrap=false \
      "-p:ServerGarbageCollection=$server" "-p:IlcLinkServerGC=$server" \
      "-p:PalObserverObject=$root/artifacts/gc_observer.o" "${args[@]}" -o "$output"
    nm "$output/GcProbe" > "$output/symbols.txt"
    if grep -q '__wrap__ZN15GCToOSInterface' "$output/symbols.txt"; then
      echo 'Qualification must not use --wrap' >&2; exit 1
    fi
    if [[ "$backend" == fault ]]; then
      DOTNET_GCHeapHardLimit=08000000 timeout 180s "$output/GcProbe" qualify fault "$profile" | tee "$output/fault.log"
    else
      mode=source-kernel
      [[ "$backend" != baseline ]] || mode=baseline
      DOTNET_GCHeapHardLimit=20000000 timeout 180s "$output/GcProbe" "$mode" | tee "$output/observer.log"
      PAL_STRESS_SECONDS="${PAL_STRESS_SECONDS:-10}" DOTNET_GCHeapHardLimit=08000000 \
        timeout 240s "$output/GcProbe" qualify stress "$profile" | tee "$output/stress.log"
      DOTNET_GCHeapHardLimit=08000000 timeout 180s "$output/GcProbe" qualify oom "$profile" | tee "$output/oom.log"
    fi
  done
  for backend in linux host; do
    DOTNET_GCHeapHardLimit=08000000 python3 scripts/benchmark.py \
      --baseline "artifacts/qualification/$profile-baseline/GcProbe" \
      --candidate "artifacts/qualification/$profile-$backend/GcProbe" \
      --profile "$profile" --trials 3 \
      --output "artifacts/qualification/$profile-$backend-benchmark.json"
  done
done
