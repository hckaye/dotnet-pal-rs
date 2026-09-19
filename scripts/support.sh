#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/support
case "$(uname -s)" in
  Linux) libs=(-lpthread -ldl -lm); gc=(-Wl,--gc-sections);;
  Darwin) libs=(-lpthread -lm); gc=(-Wl,-dead_strip);;
  *) echo 'support tests require a POSIX host' >&2; exit 2;;
esac
run(){ python3 -c 'import subprocess,sys; subprocess.run(sys.argv[1:],check=True,timeout=60)' "$@"; }
cc_args=(-std=c11 -O2 -Wall -Wextra -Werror -Iinclude)
if [[ $(uname -s) == Linux ]]; then
  cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux
  cc "${cc_args[@]}" tests/support.c target/release/libdotnet_pal_standalone.a "${gc[@]}" "${libs[@]}" -o artifacts/support/linux
  run artifacts/support/linux
fi
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-support --target-dir target/host-support
lib=target/host-support/release/libdotnet_pal_standalone.a
cc "${cc_args[@]}" tests/support.c crates/dotnet-pal-posix/native/support_posix.c tests/host_backend.c "$lib" "${gc[@]}" "${libs[@]}" -o artifacts/support/host
run artifacts/support/host
cc "${cc_args[@]}" tests/support_faults.c tests/host_backend.c "$lib" "${gc[@]}" "${libs[@]}" -o artifacts/support/faults
for i in {1..12};do run artifacts/support/faults "$i";done
c++ -std=c++17 -O2 -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native tests/support_adapter.cpp "${libs[@]}" -o artifacts/support/adapter
run artifacts/support/adapter
