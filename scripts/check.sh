#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ulimit -c 0 # ABI tests intentionally fault in isolated child processes.
mkdir -p artifacts
cargo test --lib
cargo build --release
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/abi.c target/release/libdotnet_pal_rs.a -ldl -lpthread -lm -o artifacts/abi-linux
./artifacts/abi-linux
cargo build --release --no-default-features --features host --target-dir target/host
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/abi.c tests/host_backend.c target/host/release/libdotnet_pal_rs.a -ldl -lpthread -lm -o artifacts/abi-host
./artifacts/abi-host
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/invalid_host.c target/host/release/libdotnet_pal_rs.a -ldl -lpthread -lm -o artifacts/invalid-host
./artifacts/invalid-host
c++ -std=c++17 -Wall -Wextra -Werror -Iinclude -Inative tests/adapter.cpp -o artifacts/adapter
./artifacts/adapter
python3 -m unittest discover -s tests -p 'test_*.py'
