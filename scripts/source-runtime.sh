#!/usr/bin/env bash
# Rebuild the native runtime at the audited pin; qualify it WITHOUT --wrap.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
[[ $# == 1 ]] || { echo 'usage: source-runtime.sh /clean/dotnet/runtime' >&2; exit 2; }
runtime=$(realpath "$1")
case "$(uname -m)" in x86_64) arch=x64;; aarch64) arch=arm64;; *) exit 2;; esac
[[ "$(uname -s)" == Linux ]]
mkdir -p artifacts
python3 integration/dotnet10/patch_runtime.py "$runtime" --check
python3 integration/dotnet10/patch_runtime.py "$runtime"
bash scripts/unwind-cache.sh "$runtime"
if ! "$runtime/src/coreclr/build-runtime.sh" -release -arch "$arch" -component nativeaot -ninja \
  -cmakeargs "-DDOTNET_PAL_ROOT=$root" > artifacts/source-build.log 2>&1; then
  tail -n 100 artifacts/source-build.log; exit 1
fi
tail -n 12 artifacts/source-build.log
bash scripts/nativeaot.sh
original=$(python3 -c 'from pathlib import Path; print(Path("artifacts/runtime-archive.txt").read_text(encoding="utf-8-sig").strip())')
overlay="$root/artifacts/source-sdk"
mkdir -p "$overlay"
[[ ! -e "$overlay/libRuntime.WorkstationGC.a" ]] || { echo 'Remove artifacts/source-sdk before rerunning' >&2; exit 1; }
cp -as "$(dirname "$original")/." "$overlay/"
for collector in WorkstationGC ServerGC; do
  mapfile -t archives < <(find "$runtime/artifacts/bin/coreclr" -name "libRuntime.$collector.a" -type f)
  [[ ${#archives[@]} == 1 ]] || { echo "Expected exactly one rebuilt $collector archive" >&2; exit 1; }
  nm -u "${archives[0]}" > "artifacts/source-$collector-undefined.txt"
  grep -q 'dotnet_pal_get_api' "artifacts/source-$collector-undefined.txt"
  python3 scripts/audit_machine_boundary.py "${archives[0]}" --output "artifacts/source-$collector-machine.json"
  rm "$overlay/libRuntime.$collector.a"
  cp "${archives[0]}" "$overlay/libRuntime.$collector.a"
done
python3 - "$overlay" "$runtime" <<'PYMANIFEST'
import hashlib, json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
revision = subprocess.check_output(["git", "-C", sys.argv[2], "rev-parse", "HEAD"], text=True).strip()
manifest = {"runtime_revision": revision, "adapter": "dotnet-pal-gc-vm-v2",
            "archives": {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                         for p in (root / "libRuntime.WorkstationGC.a", root / "libRuntime.ServerGC.a")}}
Path("artifacts/source-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
PYMANIFEST
clang++ -std=c++17 -O2 -fPIC -ffunction-sections -fdata-sections -Wall -Wextra -Werror \
  -DDOTNET_PAL_OBSERVER_ONLY -Iinclude -Inative -c integration/dotnet10/gc_wrap.cpp -o artifacts/gc_observer.o
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-context,host-support,host-machine --target-dir target/host-kernel
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c tests/services_host.c -o artifacts/services_host.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c tests/kernel_host.c -o artifacts/kernel_host.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c tests/runtime_host.c -o artifacts/runtime_host.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c tests/context_host.c -o artifacts/context_host.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c native/support_posix.c -o artifacts/support_host.o
clang -std=c11 -O2 -fPIC -Wall -Wextra -Werror -Iinclude -c native/machine_linux.c -o artifacts/machine_host.o
clang -r artifacts/machine_host.o artifacts/support_host.o artifacts/host_backend.o artifacts/services_host.o artifacts/kernel_host.o artifacts/runtime_host.o artifacts/context_host.o -o artifacts/host_services_backend.o
bash scripts/qualify.sh "$overlay" "$root/artifacts/source-manifest.json"
# Initialization alone prepares dump arguments; these runs do NOT create a dump.
for backend in linux host; do
  DOTNET_DbgEnableMiniDump=1 PAL_EXPECT_NATIVE_HEAP=1 \
    timeout 120s "artifacts/qualification/workstation-$backend/GcProbe" source-kernel \
    > "artifacts/qualification/workstation-$backend/support-startup.log" 2>&1
done
for profile in workstation server; do
  collector=WorkstationGC
  [[ "$profile" != server ]] || collector=ServerGC
  python3 scripts/audit_dependencies.py \
    --runtime "$overlay/libRuntime.$collector.a" --pal target/release/libdotnet_pal_rs.a \
    --binary "artifacts/qualification/$profile-linux/GcProbe" \
    --output "artifacts/qualification/$profile-linux/dependency-inventory.log"
done
echo "SOURCE RUNTIME QUALIFICATION PASS architecture=$arch (workstation and server, no --wrap)"
