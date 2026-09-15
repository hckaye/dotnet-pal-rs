# dotnet-pal-rs

Experimental **single, versioned OS boundary for NativeAOT**, implemented in Rust
`no_std`. The goal is to funnel runtime/GC platform dependencies through a small,
tested C ABI that an SDK-backed implementation can replace.

**Current scope: the GC virtual-memory slice, not a complete NativeAOT port.**
This is not an official .NET project, a Switch runtime, or a supported console
shipping toolchain. No proprietary SDK code is included.

```text
C# allocation -> NativeAOT GC -> .NET-version adapter
                                    |
                         dotnet_pal_get_api(2)
                         versioned C VM table
                                    |
                      Rust no_std, no allocator
                         /                  \
               Linux libc backend      host callback backend
                                             |
                                  private SDK implementation
                                  (C, C++, or Rust)
```

## Implemented

The common layer checks version, flags, alignment, overflow and page rounding.
The VM contract distinguishes reserve, commit, decommit, release and reset.
Decommit retains the address reservation; recommit must return zero-filled pages.
There is no writable/executable memory API and no dependency on Rust `std` or
`alloc`. The Linux backend uses `libc` with default features disabled; the host
backend has no crate dependencies.

The original 0.1 prototype's per-function ABI is intentionally replaced by ABI 2.
`include/dotnet_pal.h` is the public boundary; `native/gc_vm_adapter.h` owns the
version-specific .NET translation. Flags and results use fixed-width integers,
not foreign-constructible Rust enums.

## Run the tests on Linux

Prerequisites: Rust 1.85.1, C/C++ compiler, Clang, binutils, Python 3, and for the
managed probe **.NET SDK 10.0.100** plus its NativeAOT native prerequisites.
These are research pins, not recommendations for production/security servicing.

```sh
bash scripts/check.sh
bash scripts/nativeaot.sh
```

`check.sh` tests the Rust core, the C ABI against both backends, inaccessible
reserved/decommitted pages (in child processes), alignment, invalid input,
zero-on-recommit, preservation of neighboring pages, concurrent VM operations,
invalid host tables, and the C++ adapter. Core dumps are disabled for fault tests.

`nativeaot.sh` checks the installed runtime's six expected GC C++ symbols, then
publishes and executes the **same C# allocation workload** three ways:

| Mode | Expected evidence |
| --- | --- |
| Baseline, no GC wrapping | All Rust VM counters remain zero |
| GC wrapped -> Linux Rust backend | GC startup reserve and subsequent commit counters are nonzero/increase |
| GC wrapped -> Rust host backend -> Linux C mock SDK | The same assertion, with the replaceable host backend |

The C# program only imports a statistics observer. It never imports or calls the
VM operations. Thus the negative control separates ordinary P/Invoke from actual
GC-to-Rust calls. Tests exercising managed threads/GC do **not** imply that thread
suspension, exceptions, or the rest of the runtime have been ported.

The integration test uses ELF `--wrap` against the prebuilt .NET 10.0.0 NativeAOT
runtime. This redirects undefined symbol references, not inlined or same-object
calls. **It is instrumentation, not proof that all OS calls have been eliminated.**
Large pages and write-watch are unsupported; NUMA placement is not implemented.

CI is defined for native Linux x64 and ARM64. Its logs are the source of truth for
execution results; having a workflow file alone does not mean the tests passed.

## A genuinely no-OS-dependent build of the boundary

```sh
rustup target add aarch64-unknown-none --toolchain 1.85.1
cargo +1.85.1 build --release --no-default-features --features host \
  --target aarch64-unknown-none
```

This compiles the **boundary library**, not NativeAOT or a Switch executable. The
final host supplies `dotnet_pal_host_v2` and a non-returning
`dotnet_pal_host_abort`, plus the target toolchain's compiler/CRT helpers.
The immutable host table must exist before managed runtime initialization.
`tests/host_backend.c` is a working Linux mock; it is not a console implementation.

For a Switch-oriented port, keep SDK-specific callbacks and build/link integration
in a separate authorized repository. Do not assume a public freestanding Rust
target is compatible with the commercial SDK ABI or its packaging requirements.
See [architecture and contracts](docs/architecture.md).

## Source-level integration path

`integration/dotnet10/patch_runtime.py` prepares a guarded source adapter for
exact runtime commit `60629d14374c56f1cb51819049ad1fa529307f8d` (v10.0.0):

```sh
python3 integration/dotnet10/patch_runtime.py /path/to/runtime --check
python3 integration/dotnet10/patch_runtime.py /path/to/runtime
```

It changes six GC VM definitions and adds a NativeAOT-scoped CMake option.
Pass `-DDOTNET_PAL_ROOT=/absolute/path/to/dotnet-pal-rs` to CMake through the
runtime build's CMake-argument mechanism, rebuild NativeAOT, and link the Rust
archive into the final application. Do **not** also use `--wrap` for that build.
Normal CoreCLR builds retain their original implementations because the define
is scoped to NativeAOT. A dirty or mismatched runtime checkout is rejected.

CI verifies that the patch applies to the pinned source. **Full runtime source
rebuild and execution of this source-patched variant are not part of this PoC's
CI yet.** Do not mistake patch applicability for a completed source port.

## What remains outside the boundary

Runtime PAL, GC synchronization/process services, TLS and thread attachment,
stack bounds and register contexts, safepoints/suspension, EH/unwind, process-wide
memory barriers, BCL native shims, startup, object format/ABI, and toolchain/RID
integration remain to be audited and adapted. There is no silent SDK fallback.

The next milestone is a fully rebuilt source-adapter configuration and an OS-call
inventory, then capability groups for synchronization, clocks, threads and TLS.
One logical boundary does not mean putting every runtime subsystem into one file.

## Primary references

- [Pinned GC VM contracts](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/env/gcenv.os.h)
- [Pinned Unix GC implementation](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/unix/gcenv.unix.cpp)
- [NativeAOT native linking](https://learn.microsoft.com/dotnet/core/deploying/native-aot/interop)
- [GNU linker options, including --wrap limitations](https://sourceware.org/binutils/docs/ld/Options.html)
- [Rust's public Switch freestanding target](https://doc.rust-lang.org/rustc/platform-support/aarch64-nintendo-switch-freestanding.html)
