#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/images
case "$(uname -s)" in
  Linux) libs=(-lpthread -ldl -lm); gc=(-Wl,--gc-sections);;
  Darwin) libs=(-lpthread -lm); gc=(-Wl,-dead_strip);;
  *) exit 2;;
esac
run(){ python3 -c 'import subprocess,sys;subprocess.run(sys.argv[1:],check=True,timeout=60)' "$@"; }
common=(-std=c11 -O2 -Wall -Wextra -Werror -Iinclude)
cargo build --release --no-default-features --features host-images --target-dir target/host-images
lib=target/host-images/release/libdotnet_pal_rs.a
cc "${common[@]}" tests/images_host.c tests/host_backend.c "$lib" "${gc[@]}" "${libs[@]}" -o artifacts/images/faults
for mode in {0..12};do run artifacts/images/faults "$mode";done
if [[ $(uname -s) == Linux ]]; then
  cargo build --release
  cc "${common[@]}" tests/images.c target/release/libdotnet_pal_rs.a "${gc[@]}" "${libs[@]}" -o artifacts/images/linux
  run artifacts/images/linux
  cc "${common[@]}" tests/images.c native/images_linux.c tests/host_backend.c "$lib" "${gc[@]}" "${libs[@]}" -o artifacts/images/host
  run artifacts/images/host
fi
