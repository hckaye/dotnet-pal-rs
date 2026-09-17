# Browser port example

A port of the `dotnet-pal-rs` boundary to a web page, and a C# application
compiled by the experimental NativeAOT LLVM toolchain that runs in the browser
through it. The example is laid out the way a third-party port would be:

| Path | Content |
| --- | --- |
| `src/lib.rs` | The port: five JavaScript imports implement clock, wall time, entropy, environment and diagnostics; `define_pal!` exports the entry point |
| `build.rs` | Asks `dotnet-pal-build` for the NativeAOT adapter objects and the MSBuild link inputs when `WASI_SDK_PATH` is set |
| `host/host.mjs` | Reference JavaScript host for the `dotnet_pal_browser_v1` imports (browser and Node) |
| `host/wasi.mjs` | WASI Preview 1 subset for the managed runtime and BCL in a page: console, clocks, entropy, arguments, environment; files and sockets are absent |
| `host/probe.mjs`, `host/app.mjs` | Shared runners for the boundary test and for the managed application |
| `tests/browser.c` | C contract test linked freestanding into the port (both storage variants) |
| `app/` | The C# program (`BrowserApp.csproj`, `Program.cs`) |
| `run.sh` | Boundary test under Node and headless Chromium |
| `build-app.sh`, `run-app.sh` | Managed application: build on Linux (or in the container), run in headless Chromium |

## Storage variants

| Feature | Storage | Use |
| --- | --- | --- |
| `arena` (default) | Bounded static arena, part of the initial memory | Boundary test |
| `grow` | `memory.grow` pages owned by the port | Boundary test for modules without another allocator |
| `heap` | wasi-libc's allocator through the C hooks, 64 MiB budget | The NativeAOT link: the runtime and BCL already use wasi-libc |

## Boundary test

```sh
bash examples/browser-port/run.sh
```

Builds the `arena` and `grow` variants for `wasm32-unknown-unknown` and
`wasm32v1-none`, links `tests/browser.c`, and runs it under Node and inside a
real headless Chromium from a generated `file://` page. Needs Clang with a Wasm
backend, `wasm-ld`, Node and a Chromium-based browser (`BROWSER_BIN` overrides
the search).

## Managed application

The compiler host pack of the experimental toolchain is published for
`linux-x64`, `linux-arm64` and `win-x64`, so the C# build runs on Linux. From
the repository root, with the container from `integration/llvm-wasi/container`:

```sh
docker build -t dotnet-pal-llvm integration/llvm-wasi/container
docker run --rm -v "$PWD:/work" -w /work -v dotnet-pal-nuget:/nuget dotnet-pal-llvm bash scripts/llvm-investigate.sh
docker run --rm -v "$PWD:/work" -w /work -v dotnet-pal-nuget:/nuget dotnet-pal-llvm bash examples/browser-port/build-app.sh
bash examples/browser-port/run-app.sh
```

The first container run verifies the pinned compiler packages and prepares the
audited BCL overlay; it also executes the WASIp1 qualification workload. The
second builds the `heap` variant of the port for `wasm32-wasip1` (its `build.rs`
compiles the GC wrapper, entropy, errno-text and storage-hook adapters with the
WASI SDK and writes `browser-port.props`), publishes `app/BrowserApp.csproj`
against it, and runs the module under Node with the page hosts. `run-app.sh`
then loads the generated page in headless Chromium.

`Program.cs` imports only statistics observers. It allocates in waves and
collects, checks zeroing and root preservation, runs 64 finalizers, exercises
throw/filter/finally, reads entropy, the wall clock and an environment variable
supplied by the page, writes to the console, and finally asserts that GC storage
and the runtime clock crossed the Rust boundary (`rust_allocations`, `commits`,
`clock` counters above their startup values). The page's WASI host counts what
the BCL asked for; `path_open` must never be called because a page has no files.

## What is not covered

The application is single-threaded and uses invariant globalization. Files,
sockets, DOM access and JavaScript interop beyond the five imports are not
provided. The published compiler and runtime packs are used unchanged; the
source-rebuilt runtime of the WASI pipeline has not been linked against this
port.
