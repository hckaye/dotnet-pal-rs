#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0 # ABI tests intentionally fault in isolated child processes.
mkdir -p artifacts
cargo test -p dotnet-pal-rs --lib --features linux
cargo rustc --lib --crate-type staticlib --release --features linux
# Discard unused prebuilt-core sections instead of providing a fake Rust unwinder.
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/abi.c target/release/libdotnet_pal_rs.a -Wl,--gc-sections -ldl -lpthread -lm -o artifacts/abi-linux
timeout 60s ./artifacts/abi-linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host --target-dir target/host
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/abi.c tests/host_backend.c target/host/release/libdotnet_pal_rs.a -Wl,--gc-sections -ldl -lpthread -lm -o artifacts/abi-host
timeout 60s ./artifacts/abi-host
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/invalid_host.c target/host/release/libdotnet_pal_rs.a -Wl,--gc-sections -ldl -lpthread -lm -o artifacts/invalid-host
timeout 60s ./artifacts/invalid-host
c++ -std=c++17 -Wall -Wextra -Werror -Iinclude -Inative tests/adapter.cpp -o artifacts/adapter
timeout 60s ./artifacts/adapter
python3 -m unittest discover -s tests -p 'test_*.py'
cargo test -p dotnet-pal-rs --lib --no-default-features --features linear
cargo rustc --lib --crate-type staticlib --release --no-default-features --features linear --target-dir target/linear
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/linear.c target/linear/release/libdotnet_pal_rs.a -Wl,--gc-sections -ldl -lpthread -lm -o artifacts/linear
timeout 60s ./artifacts/linear
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/linear_threads.c target/linear/release/libdotnet_pal_rs.a -Wl,--gc-sections -ldl -lpthread -lm -o artifacts/linear-threads
timeout 60s ./artifacts/linear-threads
