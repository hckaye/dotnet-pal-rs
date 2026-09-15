# dotnet-pal-rs

A **single, versioned OS-service boundary for NativeAOT**, implemented in Rust
`no_std`. Target-specific implementations sit behind a C ABI; .NET-version
adaptation stays outside the Rust backend. No `std` or heap allocator is required.

**Implemented scope:** GC virtual memory and clock/scheduling integration on
Linux; bounded linear storage and a real WASIp1 clock for Wasm boundary tests.
This is not a complete NativeAOT port, a C# code generator, or a production support
promise. Rust compilation alone does not establish NativeAOT compatibility.
See [readiness gates](docs/readiness.md) and [services contract](docs/services.md).

## Validation examples: Wasm and freestanding Rust targets

Start here to evaluate a new platform. **Build, boundary execution and managed
runtime execution are separate evidence levels.** The CI conclusion and logs for
the exact commit determine whether a configured test passed.

| Target / profile | Validation | Not established |
| --- | --- | --- |
| `wasm32-unknown-unknown`, `linear` | C-to-Rust contract tests executed in Node/V8 | Browser integration, threads or a managed runtime |
| `wasm32v1-none`, `linear` | Minimal-target Wasm memory tests, exhaustion/reuse | WebAssembly GC objects or C# execution |
| `wasm32-wasip1`, `linear` | Same storage suite with no host imports | WASI OS services |
| `wasm32-wasip1`, `wasi-clock` | Real `clock_time_get` import, failure injection and capability checks | WASIp2 components, files, thread scheduling or NativeAOT Wasm |
| `aarch64-unknown-none`, `host` / `host-services` | Freestanding static-library cross-build | Final host linkage or device execution |
| `riscv64gc-unknown-none-elf`, `host` / `host-services` | Freestanding cross-build | A .NET code generator / ABI port |
| `thumbv7em-none-eabi`, `host` / `host-services` | 32-bit build without 64-bit atomics | Board support or interrupt safety |
| Linux x64 / ARM64 | C ABI tests, GC negative/positive controls and source-rebuilt native runtime | Elimination of every OS dependency |

```sh
rustup toolchain install 1.85.1 --profile minimal
rustup override set 1.85.1
rustup target add wasm32-unknown-unknown wasm32-wasip1 wasm32v1-none
bash scripts/wasm.sh
bash scripts/wasi-clock.sh

rustup target add thumbv7em-none-eabi
cargo build --release --no-default-features --features host-services \
  --target thumbv7em-none-eabi
```

Wasm execution tests need Node.js 22, Clang with Wasm support and `wasm-ld`.
The freestanding archives require host callbacks and compiler/CRT helpers at final
link. An archive build is not an executable, and does not prove those dependencies
are available on a device.

## One entry point, independent capabilities

```text
NativeAOT GC / runtime-version adapters
                     |
            dotnet_pal_get_api(2)
         version / size / capability checks
                     |
           Rust no_std common boundary
             /        |         \
       VM operations  |    clock / scheduling
        linux / host  |    linux / host-services / wasi-clock
                      |
              bounded linear storage
                 native / Wasm
```

`include/dotnet_pal.h` is the public ABI. New groups are appended after existing
fields; consumers check both `struct_size` and capability bits. The ABI is
per-target C, not a wire format or a WASI Component Model interface. Missing
operations have NULL callbacks. A malformed required host table fails negotiation.

**Linear storage is not virtual memory.** It uses an 8 MiB reusable arena, 4 KiB
allocation granularity, checked alignment, zero-on-allocation and whole-allocation
release. No sparse reservation, protection fault, physical decommit, `memory.grow`
or executable memory is provided. Exhaustion is reported. The NativeAOT VM adapter
rejects this capability; connecting a GC to linear storage requires another
explicit runtime adaptation and tests.

The VM contract separates reserve, commit, decommit, release and reset. Decommit
retains the reservation; recommit returns zero-filled pages. Linux uses libc with
default features disabled. The `host` backend has no crate dependencies and uses
an immutable host-supplied VM table. `host-services` adds a separate required table
for monotonic time, interrupted-sleep retry and scheduling hints. Legacy `host`
builds need no new symbols. Details: [architecture](docs/architecture.md),
[clock and scheduling contracts](docs/services.md).

## Native tests

Prerequisites: Rust 1.85.1, C/C++ compiler, Clang, binutils, Python 3. Managed probes
also require **.NET SDK 10.0.100** and NativeAOT native prerequisites. These are
reproducible research pins, not recommendations for security servicing.

```sh
bash scripts/check.sh
bash scripts/services.sh
bash scripts/nativeaot.sh
```

Tests cover ABI layout, invalid inputs, overflow, alignment, zero-on-recommit,
inaccessible VM pages in isolated child processes, neighboring data preservation,
concurrency, linear exhaustion/reuse, clock monotonicity, EINTR handling, missing
capabilities and malformed/failing host callbacks. Core dumps are disabled for
intentional fault tests. The APIs still require valid foreign pointer lifetimes.

The same C# allocation workload runs with no VM wrapping, Rust Linux VM wrapping,
and Rust host VM wrapping. The baseline must leave VM counters zero; wrapped
variants must show actual GC reserve/commit calls. **C# imports statistics observers
only, never memory or clock operations.** Service counters must remain zero in
these VM-only experiments. `--wrap` redirects undefined references, not inlined or
same-object calls; it is not evidence of a complete runtime port.

## Source-rebuilt NativeAOT integration

Given a clean checkout of `dotnet/runtime` commit
`60629d14374c56f1cb51819049ad1fa529307f8d` (v10.0.0):

```sh
bash scripts/source-runtime.sh /path/to/runtime
```

The guarded patch routes six GC VM definitions and five GC clock/scheduling
definitions into the same boundary, using a NativeAOT-only CMake option. Normal
CoreCLR builds retain their original implementation. Modified/mismatched source
is rejected. The script rebuilds the **native NativeAOT component** and links it
with pinned published compiler/BCL artifacts. It does not rebuild the compiler or
BCL, alter the NuGet cache, or use `--wrap` in the rebuilt configuration.

Both Linux and host-services paths must show increasing VM and clock counters
inside the managed workload. CI configures this on native x64 and ARM64. Archive
SHA-256, source revision, symbol inventories and run logs are retained as evidence.
Clock/scheduling capability absence is fatal in this adapter, because upstream
GC APIs cannot return these errors safely. Sleep/yield usage is workload-dependent;
their semantics are tested independently. See [services](docs/services.md).

## Remaining work

Runtime PAL beyond these groups, GC synchronization, threads/TLS, attachment,
stack bounds/register contexts, safepoints/suspension, EH/unwind, process-wide
barriers, BCL shims, startup, object format, ABI/code generation and RID integration
remain outside the implemented boundary. `Thread.Sleep` and `Stopwatch` are not
redirected by the GC adapter. Wasm tests are boundary tests, not a .NET Wasm port.

Product qualification still requires a selected runtime feature profile, a full
OS-call inventory, low-memory/finalizer/fault stress, overhead measurements and
servicing policy. Do not infer production readiness from one passing workflow.
The goal is one logical boundary, not one monolithic source file.

## Primary references

- [Pinned GC contracts](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/env/gcenv.os.h)
- [Pinned GC implementation](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/unix/gcenv.unix.cpp)
- [NativeAOT native linking](https://learn.microsoft.com/dotnet/core/deploying/native-aot/interop)
- [Rust target support](https://doc.rust-lang.org/rustc/platform-support.html)
- [WASI Preview 1](https://github.com/WebAssembly/WASI/tree/wasi-0.1/preview1)
- [Node WASI embedding](https://nodejs.org/api/wasi.html)
