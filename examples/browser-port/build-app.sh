#!/usr/bin/env bash
# Builds the managed BrowserApp for the browser port. Needs a Linux (x64 or
# arm64) host: the experimental ILCompiler.LLVM host pack is not published for
# macOS. Prerequisites: .NET SDK 10.0.401, WASI_SDK_PATH, NUGET_PACKAGES, Rust
# with wasm32-wasip1, Python 3. integration/llvm-wasi/container/Dockerfile
# provides all of them; from the repository root:
#   docker run --rm -v "$PWD:/work" -w /work -v dotnet-pal-nuget:/nuget dotnet-pal-llvm \
#     bash examples/browser-port/build-app.sh
set -euo pipefail
cd "$(dirname "$0")"
root="$(cd ../.. && pwd)"
: "${WASI_SDK_PATH:?provide the verified WASI SDK}"
: "${NUGET_PACKAGES:?provide an isolated package cache}"
export MSBuildEnableWorkloadResolver=false
target_dir="${CARGO_TARGET_DIR:-$root/target}/browser-port-heap"
mkdir -p artifacts/app
# 1. The port for the NativeAOT link: wasi-libc-backed storage hooks, 64 MiB budget,
#    plus the adapter archive and MSBuild props from build.rs (dotnet-pal-build).
env -u CARGO_TARGET_DIR cargo build -p browser-port --release --no-default-features --features heap \
  --target wasm32-wasip1 --target-dir "$target_dir"
props="$(find "$target_dir/wasm32-wasip1/release/build" -name browser-port.props | head -1)"
[[ -n "$props" ]] || { echo 'browser-port.props was not generated (is WASI_SDK_PATH set?)' >&2; exit 1; }
# 2. Pinned compiler packages and the audited BCL overlay, produced and digest-checked
#    by scripts/llvm-investigate.sh (the WASI pipeline) in the same checkout.
dotnet restore app/BrowserApp.csproj -r wasi-wasm
[[ -d "$root/artifacts/llvm/bcl" && -d "$root/artifacts/llvm/published-sdk" ]] || { echo 'run scripts/llvm-investigate.sh first: it verifies the compiler packages and prepares the BCL overlay' >&2; exit 1; }
# 3. Compile and link the C# program against the port.
rm -rf app/obj/Release app/bin/Release
dotnet publish app/BrowserApp.csproj -r wasi-wasm -c Release -p:IlcLlvmTarget=wasm32-unknown-wasip1 \
  "-p:IlcFrameworkPath=$root/artifacts/llvm/bcl/" "-p:IlcSdkPath=$root/artifacts/llvm/published-sdk/" \
  "-p:PalProps=$props" -o artifacts/app 2>&1 | tee artifacts/app/publish.log
"$WASI_SDK_PATH/bin/llvm-nm" artifacts/app/BrowserApp.wasm > artifacts/app/symbols.txt || true
node tests/app.mjs artifacts/app/BrowserApp.wasm | tee artifacts/app/node-run.log
node page.mjs --app artifacts/app/BrowserApp.wasm artifacts/app/index.html
echo 'MANAGED BROWSER BUILD PASS: BrowserApp.wasm executed under Node with the page hosts; open artifacts/app/index.html or run run-app.sh'
