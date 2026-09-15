# dotnet-pal-rs

A **single, versioned OS-service boundary for NativeAOT**, implemented in Rust
`no_std`. Platform-specific implementations sit behind a small C ABI; .NET
version-specific adaptation stays outside the Rust backend.

**Implemented scope: GC virtual-memory integration on Linux and a separate
bounded linear-storage capability for freestanding/Wasm boundary validation.**
This is not a complete NativeAOT port, a new C# code generator, or a production
support promise. Rust target compilation alone does not establish NativeAOT
compatibility. See [readiness criteria](docs/readiness.md).

## Validation examples: Wasm and freestanding Rust targets

Start here when evaluating a new target. These examples deliberately distinguish
compiling the boundary, executing it, and running an actual managed runtime.
The CI results for the exact revision are the execution evidence, not this table.

| Target / configuration | Validation provided | What it does NOT establish |
| --- | --- | --- |
| `wasm32-unknown-unknown`, `linear` | C-to-Rust contract suite linked into a core Wasm module and executed in Node/V8 | NativeAOT code generation, browser integration or threads |
| `wasm32-wasip1`, `linear` | Same suite, compiled for the WASIp1 target; this no_std module has no WASI imports | WASI syscalls, files, clocks, Preview 2 components or a .NET WASI runtime |
| `wasm32v1-none`, `linear` | Minimal-target module, repeated allocation/reuse tests | Full C# execution or WebAssembly GC integration |
| `aarch64-unknown-none`, `host` | Boundary static-library cross-build with no std/alloc/libc dependency | Final host linkage, device execution or NativeAOT startup |
| `riscv64gc-unknown-none-elf`, `host` | Boundary static-library cross-build | A .NET code generator/ABI port for this target |
| `thumbv7em-none-eabi`, `host` | 32-bit boundary build, without 64-bit atomics | An embedded .NET runtime, interrupt safety or board support |
| Linux x64 / ARM64, `linux` and `host` | C ABI/concurrency tests and real NativeAOT GC negative/positive controls | Redirection of every runtime OS call |
| Linux x64, source-rebuilt runtime | Rebuild the native NativeAOT component and run GC probes without `--wrap` | A rebuilt compiler/BCL or a complete OS-independent runtime |

```sh
rustup toolchain install 1.85.1 --profile minimal
rustup override set 1.85.1
rustup target add wasm32-unknown-unknown wasm32-wasip1 wasm32v1-none
bash scripts/wasm.sh

rustup target add thumbv7em-none-eabi
cargo build --release --no-default-features --features host \
  --target thumbv7em-none-eabi
```

Wasm tests require Node.js 22, Clang with WebAssembly support, and `wasm-ld`.
They validate the linked binary, reject unexpected imports/shared memory, run the
same C contract suite repeatedly, create independent module instances, and assert
that the allocator does not grow the module's memory. This is a narrow C ABI test
for the exact signatures in this repository, not blanket C/Rust ABI compatibility.

## One entry point, distinct capabilities

```text
NativeAOT GC -> version-specific VM adapter --+
                                             |
Other host/runtime adapters ----------------+--> dotnet_pal_get_api(2)
                                                   |
                                          version / size / capabilities
                                                   |
                               +-------------------+------------------+
                               |                                      |
                       CAP_VM (sparse VM)                 CAP_LINEAR (owned storage)
                         linux / host                         linear backend
                      mmap or host callbacks              native / Wasm modules
```

`include/dotnet_pal.h` is the public boundary. ABI 2's existing VM prefix and
`dotnet_pal_host_api` layout are preserved. The linear table is an **append-only
extension** guarded by `struct_size` and `DOTNET_PAL_CAP_LINEAR`. Missing capability
groups contain NULL callbacks. Always check version, size and capabilities before
reading a group. The C ABI is target-local, not a serialized format or a WASI
Component Model interface.

**Linear storage is not virtual memory.** The linear backend has an 8 MiB reusable
arena, 4 KiB allocation granularity, checked alignment, zero-on-allocation,
byte-range zeroing and whole-allocation release. It uses no heap allocator or
host imports. Exhaustion returns `OUT_OF_MEMORY`; released blocks can be reused.
There is no sparse reservation, protection fault, physical decommit, `memory.grow`
or executable memory. The pinned NativeAOT VM adapter explicitly rejects it.
Connecting a managed GC to this weaker capability requires a separate runtime
adaptation and its own tests; it is not silently enabled by selecting a Rust target.

The VM capability distinguishes reserve, commit, decommit, release and reset.
Decommit keeps the address reservation; recommit must return zero-filled pages.
The Linux backend uses libc with default features disabled. The `host` backend
has no crate dependencies and forwards to an immutable host-supplied table.
Both obey the same VM contract; the common layer validates geometry and flags.

## Build and test the native boundary

Prerequisites: Rust 1.85.1, C/C++ compiler, Clang, binutils and Python 3. The managed
probes additionally require **.NET SDK 10.0.100** and NativeAOT's native prerequisites.
These are reproducible research pins, not production/security-servicing advice.

```sh
bash scripts/check.sh
bash scripts/nativeaot.sh
```

`check.sh` covers ABI layout, invalid arguments/host tables, overflow, page rounding,
zero-on-recommit, inaccessible reserved/decommitted pages in child processes,
neighbor preservation, concurrency, adapter rejection of unsupported capabilities,
linear-arena exhaustion/reuse/fragmentation and saturating diagnostic counters.

`nativeaot.sh` executes the same C# workload with three configurations:

| Configuration | Required evidence |
| --- | --- |
| Baseline, no GC wrapping | All Rust VM counters remain zero |
| GC wrapped -> Rust Linux backend | GC startup reservations and subsequent commits reach Rust |
| GC wrapped -> Rust host backend -> Linux C callbacks | Same assertions with a replaced backend |

The C# probe imports only a statistics observer, never VM operations. The linker
experiment uses ELF `--wrap`: undefined references are redirected, not inlined or
same-object calls. This is not proof that all OS dependencies have been removed.

## Source-rebuilt NativeAOT integration

The source adapter is pinned to `dotnet/runtime` commit
`60629d14374c56f1cb51819049ad1fa529307f8d` (v10.0.0). It redirects six GC VM definitions
under a NativeAOT-only CMake option, leaving normal CoreCLR builds unchanged.
A mismatched or modified target file is rejected rather than patched heuristically.

Given a clean checkout of that exact runtime revision:

```sh
bash scripts/source-runtime.sh /path/to/runtime
```

This builds the **complete native NativeAOT component**, checks for a reference to
`dotnet_pal_get_api` in the resulting runtime archive, constructs a private SDK
link overlay without modifying the NuGet cache, and publishes/runs the GC probe
with both backends. The source configuration uses an observer-only object and
rejects accidental `__wrap` helper symbols. Compiler and BCL are the published,
matching 10.0.0 versions; they are not rebuilt. Build prerequisites are listed in
`.github/workflows/ci.yml`; the `source-runtime` job records the build evidence.

## Implementing a host

Use `--no-default-features --features host`. Provide `dotnet_pal_host_v2()` and a
non-returning `dotnet_pal_host_abort()`, plus your toolchain's compiler/CRT helpers.
The immutable table and callbacks must exist **before** managed initialization,
be thread-safe, never unwind, and never reenter managed allocation. The working
Linux example is `tests/host_backend.c`; it tests the protocol, not another OS.

Pointers are trusted native pointers, not security-checked handles. Users must own
the ranges they pass and serialize conflicting operations. The boundary cannot
validate arbitrary foreign pointer lifetimes. Linear-arena metadata has a lock;
raw payload access is still the caller's responsibility. Operations are not
interrupt-/signal-handler-safe. See [contracts](docs/architecture.md).

## Scope remaining before a complete port

Runtime PAL; GC events, locks and process services; TLS/thread attachment;
stack/register contexts; safepoints/suspension; EH/unwind; process-wide memory
barriers; BCL native shims; and runtime startup are not yet behind this boundary.
Code generation, object format, relocations, calling conventions and linker/RID
integration are separate target-port work. Large pages and write-watch are
unsupported; NUMA placement is only an ignored hint in the VM adapter.

A green boundary test is not a production qualification. The next groups must be
added with call-site inventories, failure semantics and source-runtime tests.
No proprietary SDK code is included; this is not an official .NET project.

## Primary references

- [Pinned GC contracts](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/env/gcenv.os.h)
- [NativeAOT developer workflow](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/docs/workflow/building/coreclr/nativeaot.md)
- [Rust wasm32-unknown-unknown](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html)
- [Rust wasm32-wasip1](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html)
- [Rust wasm32v1-none](https://doc.rust-lang.org/rustc/platform-support/wasm32v1-none.html)
- [GNU ld --wrap semantics](https://sourceware.org/binutils/docs/ld/Options.html)
