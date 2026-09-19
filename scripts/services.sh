#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0
mkdir -p artifacts
common=(-std=c11 -Wall -Wextra -Werror -Iinclude)
libs=(-Wl,--gc-sections -ldl -lpthread -lm)
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux
cc "${common[@]}" tests/services.c target/release/libdotnet_pal_standalone.a "${libs[@]}" -o artifacts/services-linux
timeout 60s artifacts/services-linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host-services --target-dir target/host-services
cc "${common[@]}" -DPAL_SERVICES_FAULT_HOST tests/services.c tests/host_backend.c tests/services_host.c \
  target/host-services/release/libdotnet_pal_standalone.a "${libs[@]}" -o artifacts/services-host
for mode in 0 1 2 3 4 5 6 7; do timeout 60s artifacts/services-host "$mode"; done
# Unextended host and bare linear builds must still link, with no new required host symbols.
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features host --target-dir target/host
cc "${common[@]}" tests/services.c tests/host_backend.c target/host/release/libdotnet_pal_standalone.a "${libs[@]}" -o artifacts/services-legacy-host
timeout 60s artifacts/services-legacy-host
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features linear --target-dir target/linear
cc "${common[@]}" tests/services.c target/linear/release/libdotnet_pal_standalone.a "${libs[@]}" -o artifacts/services-none
timeout 60s artifacts/services-none
c++ -std=c++17 -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native tests/services_adapter.cpp -o artifacts/services-adapter
timeout 60s artifacts/services-adapter
