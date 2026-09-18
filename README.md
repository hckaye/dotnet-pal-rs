# dotnet-pal-rs

A **Rust `no_std` OS-service boundary for .NET NativeAOT ports**. The core crate
declares one trait per OS service the runtime needs, validates every call that
crosses the C ABI, and ships the pure pieces every port reuses. A platform port
is a crate that implements the traits it can honestly provide and exports the
single entry point `dotnet_pal_get_api(2)`. Absent services are reported as
clear capability bits and NULL callbacks, never as stubs that return success.

| Crate | Content |
| --- | --- |
| `dotnet-pal-rs` (this crate) | Traits (`port`), C ABI, front ends, pure storage providers, optional built-in Linux/host/WASI providers |
| `crates/dotnet-pal-std` | A complete desktop port on the Rust standard library (Linux, macOS; Windows compiles) |
| `crates/dotnet-pal-build` | `build.rs` helper that compiles the native NativeAOT adapters and writes MSBuild link inputs |
| `examples/browser-port` | A port whose services are JavaScript imports, and a C# program running in a browser through it |

See [porting](docs/porting.md) for the library workflow. The rest of this file
describes the qualification of the built-in configurations.

**Implemented and exercised:** native GC memory, clocks, scheduling, events,
recursive locks, background/finalizer thread creation, termination TLS, stack
bounds and process-wide memory barriers; managed stress/OOM/fault qualification
on Linux x64 and ARM64; actual C# GC/exception execution on WASIp1, including
a source-rebuilt LLVM native runtime without linker wrapping; the C-to-Rust
boundary executed inside a real headless browser; and a C# program whose GC
storage and runtime clock reach a web page through the boundary.

**Not complete:** this is not yet a fully OS-independent NativeAOT runtime or a
production-supported port. Dependency reports still identify direct runtime OS
references outside these groups. BCL shims, signal/context handling, module
inspection and other target-specific services remain. See [readiness](docs/readiness.md)
for exact completed and uncompleted work. Rust target compilation by itself does
not establish that NativeAOT can generate or execute code for that target.

## Using the library

```toml
[dependencies]
dotnet-pal-rs = { path = "dotnet-pal-rs", default-features = false, features = ["storage-arena"] }
[build-dependencies]
dotnet-pal-build = { path = "dotnet-pal-rs/crates/dotnet-pal-build" }
```

```rust
#![no_std]
use dotnet_pal_rs::port::{self, Error, Result};
struct MyPlatform;
impl port::Clock for MyPlatform { fn monotonic_ns() -> Result<u64> { Err(Error::Unsupported) } }
dotnet_pal_rs::define_pal! { Linear = dotnet_pal_rs::storage::Arena, Clock = MyPlatform }
dotnet_pal_rs::define_panic_handler!(dotnet_pal_rs::port::Trap);
```

`cargo test -p dotnet-pal-std` runs the desktop port against the negotiated C
table on the current machine. The built-in standalone configurations below are
selected with Cargo features and built with an explicit crate type:

```sh
cargo rustc --lib --crate-type staticlib --release --features linux
```

## Start with Wasm and freestanding validation

| Target / profile | What is executed or built | Important boundary |
| --- | --- | --- |
| `wasm32-unknown-unknown`, `linear` | C-to-Rust storage contracts run in Node/V8 | Not a managed runtime |
| `wasm32v1-none`, `linear` | Minimal-target Wasm allocation, exhaustion and reuse tests | Not shared-memory Wasm or WasmGC reference objects |
| `wasm32-wasip1`, `linear` | Same storage tests with no OS imports | Does not exercise WASI services |
| `wasm32-wasip1`, `wasi-clock` | A real WASIp1 clock import and injected host errors | No scheduler or threads are advertised |
| `examples/browser-port`, `arena` and `grow` | Same C contract under Node and headless Chromium; five JS imports, injected host failures, real memory growth | Single-threaded, no scheduler |
| `examples/browser-port`, `heap` + `app/` | C# GC, finalizers, exceptions and BCL console/clock/entropy in headless Chromium; GC storage and clock cross the boundary | Published packs unchanged; no files, sockets or JS interop |
| Experimental NativeAOT LLVM, WASIp1 | C# allocations, roots, GC, exceptions and clock calls; baseline, wrapped and source-rebuilt variants | Separate audited compiler family; single-threaded, eager storage |
| `aarch64-unknown-none`, `host` / `host-services` / `host-kernel` | Freestanding Rust archive builds | Final host linkage and device execution are not tested |
| `riscv64gc-unknown-none-elf`, same host profiles | Freestanding archive builds | Does not supply a .NET code generator or target ABI |
| `thumbv7em-none-eabi`, same host profiles | 32-bit archive builds without 64-bit atomics | Board support and interrupt safety are not established |
| Linux x64 / ARM64 | Native C ABI tests and source-rebuilt Workstation/Server GC qualification | Does not eliminate every runtime/BCL OS dependency |

CI results and logs for the **exact commit** are the evidence. Workflow definitions
or a successful archive build are not substitutes for runtime execution.

```sh
rustup toolchain install 1.98.1 --profile minimal
rustup override set 1.98.1
rustup target add wasm32-unknown-unknown wasm32-wasip1 wasm32v1-none
bash scripts/wasm.sh
bash scripts/wasi-clock.sh
bash examples/browser-port/run.sh   # needs a Chromium-based browser; see docs/wasm.md

rustup target add thumbv7em-none-eabi
cargo rustc --lib --crate-type staticlib --release --no-default-features --features host-kernel \
  --target thumbv7em-none-eabi
```

The boundary Wasm tests require Node.js 22, Clang with Wasm support and `wasm-ld`.
The browser step additionally needs `google-chrome`, `chromium` or a Playwright
`chrome-headless-shell` (override with `BROWSER_BIN`).
Freestanding archives still need host callbacks and target compiler/CRT support at
final link. They are not complete executables.

## Architecture

```text
NativeAOT GC / runtime-version adapters
                     |
            dotnet_pal_get_api(2)
          version / size / capabilities
                     |
           Rust no_std common boundary
          /            |              \
  VM / clock / kernel  linear storage   linear storage
   Linux or host       WASIp1 imports   browser JS imports
                       Node WASI host   page / headless Chromium
```

`include/dotnet_pal.h` defines the ABI. Existing prefix offsets are preserved by
append-only capability groups. Consumers check the end offset of a group before
reading it. The ABI is local to the target C calling convention, not a wire format
or a WASI Component Model interface.

The crate uses no Rust `std` or `alloc` dependency. **This does not mean all backend
operations are allocation-free:** Linux event/mutex/thread/TLS resources use native
allocation. Raw pointers and opaque handles require valid caller-owned lifetimes,
serialized destruction, and no unwind or managed reentry from host callbacks.
They are not security-checked handles for untrusted native callers.

The `host`, `host-services`, and `host-kernel` features require progressively more
immutable host tables before runtime initialization. Older host configurations
need no new provider symbols. A required missing or malformed table rejects
negotiation. Linux process-barrier support is probed; a local atomic fence is not
passed off as a process-wide barrier.

**Linear storage is not virtual memory.** The ordinary arena is 8 MiB; the explicit
`linear-gc` experiment uses 256 MiB. Both use 4 KiB allocation granularity and
reusable bounded metadata, not sparse address reservation or hardware protection.
The managed linear adapter keeps an owned-region ledger, preserves contents on
repeated logical commit, and zeroes on logical decommit. Decommitted storage remains
accessible; no physical Wasm page reclamation or `memory.grow` is promised.
The VM-only adapter rejects a linear-only backend.

See [architecture](docs/architecture.md), [kernel contracts](docs/kernel.md),
[clock contracts](docs/services.md), [Wasm and browser profiles](docs/wasm.md)
and [qualification](docs/qualification.md).

## Native helper services

The append-only `support` group adds native helper allocation/resize, reader/writer
locks, thread names and diagnostic output. It has Linux and explicit `host-support`
providers, paired source integration in libunwind/dump formatting, and guarded
startup initialization. See [native helper contracts](docs/native-support.md).

```sh
bash scripts/support.sh
```

## Native contracts and managed qualification

Prerequisites: Rust 1.98.1, C/C++ compiler, Clang, binutils, Python 3 and Linux
NativeAOT build prerequisites. Managed tests use **.NET SDK 10.0.100**. These are
reproducibility pins, not recommendations to deploy historical versions.

```sh
bash scripts/check.sh
bash scripts/services.sh
bash scripts/kernel.sh
bash scripts/gc-linear.sh
bash scripts/nativeaot.sh
```

The C ABI suites test alignment, overflow, invalid inputs, protected pages in
isolated child processes, zero-on-recommit, ownership geometry, neighboring data,
concurrent events/mutexes/threads/TLS, interrupted sleeps, bounded arena reuse,
malformed providers and explicit unsupported results. NativeAOT probes import
only statistics observers, not memory/clock/kernel operations. Negative controls
must leave Rust counters zero; positive controls must show runtime-driven calls.

Given a clean checkout of `dotnet/runtime` commit
`60629d14374c56f1cb51819049ad1fa529307f8d`:

```sh
bash scripts/source-runtime.sh /path/to/runtime
```

This rebuilds both native GC runtime archives, checks their separate SHA-256
identities, and uses a private link overlay with matching published compiler/BCL
artifacts. It does not modify the NuGet cache or use `--wrap` in the source variant.
Normal CoreCLR builds retain their original implementations.

Both Workstation and Server GC are exercised with baseline, Rust Linux, host-kernel
and a test-only commit-fault provider. Qualification includes concurrent allocation,
recursive monitors, thread-local state, finalizer accounting, pinned/weak roots,
GC inside exception filters, throw/rethrow/finally, native-thread reverse P/Invoke,
three OOM/recovery waves and exactly one injected commit failure before OS side
effects. The effective 128 MiB heap limit and actual GC mode are asserted.

`PAL_STRESS_SECONDS` selects 1..3600 seconds per stress configuration (CI default
10). A finite successful stress run is not proof against every possible race.
Timing/RSS/binary-size reports and unresolved-symbol inventories are retained.

## Actual managed WebAssembly integration

This path uses the **experimental NativeAOT LLVM** toolchain, not an automatic
extension of the ordinary RyuJIT-based shipping NativeAOT compiler. Compiler,
host/target packs and WASI SDK digests are in `integration/llvm-wasi/toolchain.json`.
The pipeline verifies all three compiler-related packages, including the target
runtime package, before linking.

With the verified WASI SDK 29 and an isolated NuGet package directory:

```sh
export WASI_SDK_PATH=/path/to/wasi-sdk-29.0-x86_64-linux
export NUGET_PACKAGES=/path/to/isolated-package-cache
bash scripts/llvm-investigate.sh
# Clean checkout of runtimelab revision 9954350a58ede8b8eaaeb24112ca4f1e78cc527c:
bash scripts/llvm-source.sh /path/to/runtimelab
```

The first script compares no-interposition and GC-to-Rust interposition using the
same C# workload. The second rebuilds `libPortableRuntime.a` from audited source,
links an observer-only object, rejects `--wrap` helpers and executes the C# workload
again. Both positive configurations must show GC startup allocation, additional
logical commits and clocks crossing Rust. The baseline must show zero calls.
Compiler/BCL artifacts remain the matching published packages.

Execution uses Node's real WASIp1 host and rejects non-Preview-1 imports or shared
memory. The explicit P1 link profile includes a pure error-code-to-text formatter
needed by the published System.Native archive; it is not a DNS implementation.
Globalization/timezone are invariant. Networking, components, shared threads and a
general Wasm platform port are not established by this workload.

## Browser host connection and C# in a browser

`examples/browser-port` builds the boundary for `wasm32-unknown-unknown`,
`wasm32v1-none` or `wasm32-wasip1` with five typed imports from the
`dotnet_pal_browser_v1` JavaScript module: monotonic clock, wall clock, entropy,
environment lookup and diagnostic output. `host/host.mjs` is the reference host
and runs unchanged in browsers and Node.

```sh
bash examples/browser-port/run.sh
docker build -t dotnet-pal-llvm integration/llvm-wasi/container
docker run --rm -v "$PWD:/work" -w /work -v dotnet-pal-nuget:/nuget dotnet-pal-llvm bash scripts/llvm-investigate.sh
docker run --rm -v "$PWD:/work" -w /work -v dotnet-pal-nuget:/nuget dotnet-pal-llvm bash examples/browser-port/build-app.sh
bash examples/browser-port/run-app.sh
```

The first command executes `tests/browser.c` against the port under Node and
inside headless Chromium. The container commands compile the C# program in
`examples/browser-port/app` with the experimental NativeAOT LLVM toolchain
(whose host pack exists for Linux only), link it against the port and the
adapters produced by `build.rs`, and run it under Node with the page's WASI
host. The last command runs the same module in headless Chromium: the program
asserts that GC storage and the runtime clock crossed the Rust boundary, that
finalizers, exceptions, entropy and the wall clock work, and that the page never
had to open a file. See [docs/wasm.md](docs/wasm.md) and the example's README.

## Memory safety, evidence and limits

```sh
rustup toolchain install nightly-2026-09-01 --profile minimal --component rust-src
bash scripts/sanitize.sh address
```

This instruments Rust, rebuilt core/compiler-builtins and C/C++ boundary callers
together with AddressSanitizer and leak checking. It is **not** an instrumented
NativeAOT managed runtime or a complete proof against undefined behavior.

`audit_dependencies.py` reports remaining runtime references separately from Rust
backend dependencies and final ELF dynamic imports. Its `--require-isolated`
option deliberately fails when direct OS references or unreviewed symbols remain.
The current runtime is not OS-isolated: reports expose signal/context, loader,
process/topology, diagnostics and native allocation dependencies still bypassing
the implemented groups. Do not remove these checks to advertise completion.

See [readiness](docs/readiness.md) and [servicing policy](docs/servicing.md) before
using this work as a product dependency. One passing workflow does not establish
production readiness, support for all Rust targets or upstream servicing support.

## Primary references

- [Pinned native GC contracts](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/env/gcenv.os.h)
- [Pinned LLVM Wasm GC](https://github.com/dotnet/runtimelab/blob/9954350a58ede8b8eaaeb24112ca4f1e78cc527c/src/coreclr/gc/wasm/gcenv.wasm.cpp)
- [NativeAOT interop](https://learn.microsoft.com/dotnet/core/deploying/native-aot/interop)
- [Rust target support](https://doc.rust-lang.org/rustc/platform-support.html)
- [WASI Preview 1](https://github.com/WebAssembly/WASI/tree/wasi-0.1/preview1)
- [Rust sanitizers](https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/sanitizer.html)

## Machine resource boundary

The `Machine` port trait and append-only C ABI group cover CPU discovery,
process affinity, calling-thread placement, cache/page/physical-memory readings,
available swap and virtual address limits. See [machine contracts](docs/machine.md)
and `scripts/machine.sh`. The native source build preserves cgroup policy while
routing these underlying OS measurements; whole-runtime OS isolation remains
a separate strict gate. Windows `std` provider contract execution is covered by the `windows-std`
CI job; its result does not establish managed NativeAOT support on Windows.
