# One host boundary, native and linear-memory profiles

This change **extends the existing ABI 2**, preserving its layout, entry point,
Linux provider, host SDK callback backend, GC adapters, and source patch tooling.
It does not add a parallel PAL or replace the native VM contract with weaker
semantics. Only the memory group is implemented: one logical boundary for all
NativeAOT OS services remains a migration goal, not a completed port.

## Capability negotiation

Call `dotnet_pal_get_api(2)` and inspect both its header and capability bits.

| Bit | Contract |
| --- | --- |
| `DOTNET_PAL_CAP_VM` (1) | Original native VM semantics: inaccessible reserve/decommit, concurrent disjoint operations |
| `DOTNET_PAL_CAP_VM_LINEAR` (2) | Serialized logical suballocations of a host-owned writable region; no page protection |
| `DOTNET_PAL_CAP_ZERO_RECOMMIT` (4) | After decommit/recommit, the range is zero-filled |
| `DOTNET_PAL_CAP_STATS` (8) | The statistics callback supplies atomic diagnostic counters |

The native and linear profiles are **not interchangeable**. The existing native
GC adapter requires `CAP_VM` and rejects an arena. An explicit C++ regression test
checks that rejection. A future WASM runtime adapter must require `CAP_VM_LINEAR`,
select compatible GC/runtime behavior, and still implement its own root handling,
stack/safepoint, scheduling, exception, and compiler integration. Checking only the
ABI version is insufficient. These native C layouts are not a WASI Component
Model canonical ABI or a JavaScript-object interface.

Diagnostics no longer impose 64-bit atomics on every Rust target. On a target with
no such atomics the counters have no state, `CAP_STATS` is absent, and `read_stats`
returns `UNSUPPORTED` without writing pretend zero activity. Native builds keep
atomic counters and their existing negative/positive GC tests. This supersedes the
original architecture note that diagnostics required native 64-bit atomics.

## Reference WASM backend

Select exactly one Cargo backend: `linux`, `host`, or `arena`. The reference arena
is deliberately restricted to **single-threaded wasm32 with non-shared memory**.
Compilation with the atomics target feature is rejected, and the execution harness
also rejects a SharedArrayBuffer. Hosts must not override the linker to share this
instance memory. No concurrent or reentrant calls are permitted.

Before runtime startup call `dotnet_pal_arena_init(base, length, page_size)` once.
Until initialization, `dotnet_pal_get_api` returns NULL; a second initialization is
rejected. Backing storage must be aligned, writable, exclusively owned, nonmoving,
and valid for the lifetime of the instance. It must not overlap PAL state, stack,
other static data, or allocator-owned storage. This is an unsafe ownership contract,
not validation of an arbitrary foreign pointer. Each module instance has its own
state; there is no cross-instance singleton or dynamic backend replacement.

The arena maintains 64 reservation records and uses first-fit aligned placement.
Released ranges/records can be reused. Commit validates ownership and preserves
contents; decommit zeroes only the requested subrange but does not prevent access;
reset is a valid advisory no-op; release requires the original complete range.
Fresh reservations are zeroed. Page rounding, flags and overflow checks still run
through the same Rust front end as the native backend. Exhaustion or fragmentation
returns `OS_ERROR` under the existing ABI status set, with a NULL reserve output.

This backend does **not** implement virtual address reservation, page protection,
physical decommit, memory shrink, dynamic growth, threads, TLS, files, clocks,
networking, browser event scheduling, or a general-purpose allocator. In particular,
calling its decommit successfully does not lower a WASM instance's memory footprint.
The example uses a statically provisioned 256 KiB backing region, not an OS syscall.

## Tests and reproducible commands

Rust 1.85.1, Node.js with core WebAssembly support, and a C++ compiler are needed.

```sh
rustup toolchain install 1.85.1 --profile minimal
rustup target add --toolchain 1.85.1 wasm32-unknown-unknown wasm32v1-none
bash scripts/wasm.sh

c++ -std=c++17 -Wall -Wextra -Werror -Iinclude -Inative \
  tests/linear_rejected.cpp -o /tmp/linear-rejected
/tmp/linear-rejected
```

The script builds **actual WASM modules** for both targets. Node verifies that the
modules import no host functions and have non-shared memories. It instantiates each
twice, running the actual public ABI table three times per instance. The probe
checks initialization, negotiation, reserve/commit, preservation, partial decommit,
zero-recommit, neighboring bytes, invalid partial release, use-after-release
rejection, exhaustion, reset, reuse and capability-aware statistics.

`.github/workflows/wasm.yml` runs those tests separately from the existing native
CI. It also compile-checks the **host boundary library only** for `wasm32-wasip1`
and `aarch64-unknown-none`. Compile checks are not executions on those platforms;
the host profile still requires actual host callbacks when linked.

**These are Rust/WASM backend tests, not NativeAOT-generated WASM executables.**
The existing Linux GC experiment is preserved and is tested by its own workflow.
WASM C# code generation and complete runtime integration remain separate work.
The previous native-only documentation describes `CAP_VM`; it must not be read as
claiming those semantics for the new `CAP_VM_LINEAR` profile.

## Why this model also leaves room for consoles

A licensed SDK provider can implement the existing `host` callback interface in
Rust or C/C++. Rust compiler target availability does not itself establish the
vendor SDK ABI or shipping compatibility. Keep SDK-specific code out of this
public repository. Additional host models should specify their capabilities and
conformance tests instead of silently pretending to be POSIX or native VM.

Primary references:

- https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html
- https://doc.rust-lang.org/rustc/platform-support/wasm32v1-none.html
- https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html

These references describe upstream targets; CI results, not these links or the
existence of workflow files, establish this prototype's tested configurations.
