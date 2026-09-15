# Boundary contract and migration plan

## One logical service surface

.NET internals belong in version-specific adapters. Platform structures stay out
of `dotnet_pal.h`. Code generation, calling convention, object format and unwind
metadata are NOT OS-service calls and remain separate target-port obligations.
This implementation covers GC virtual memory and clock/scheduling services, plus
a distinct linear-storage capability and a WASIp1 clock for boundary testing.
It is not a complete runtime port.

Add capability groups with upstream call-site inventories and contract tests,
not by assuming every platform implements a desktop OS. The host backend can be
implemented in C, C++ or Rust. The core uses no std or global allocator.

## ABI 2 and additive extensions

`dotnet_pal_get_api(2)` returns an immutable process-lifetime table, available
before managed runtime initialization, or NULL. Consumers must check version,
struct_size and capability bits BEFORE accessing a group. Missing callbacks are
NULL. The existing VM/stats prefix and HostApi layout are unchanged; `linear` is
appended after that prefix and `services` after `linear`. Group-size checks refer
to the END of each group, not the size of an evolving complete API. Future
required semantic changes need a new version. Size checks alone cannot make an
unrelated foreign table ABI compatible.

Pointers are trusted native pointers, not security-checked handles. A caller must
supply valid writable output storage, own the supplied range, and serialize
conflicting operations. Geometry checks do not validate arbitrary pointer lifetime
or ownership. Neither the host nor this wrapper is an untrusted-code sandbox.

The host table must be immutable and valid before the first managed allocation.
Callbacks never throw/unwind, reenter managed allocation or call the same operation
recursively. They must preserve the memory contract, including failure semantics.
There is no dynamic registration requiring managed startup or a managed lock.
The optional `host-services` profile adds a separate required host table without
changing the existing VM host layout or introducing symbols to legacy `host` builds.

The VM dispatcher performs no allocation or locking. Diagnostic counters use
pointer-width atomics, saturate at usize::MAX, and widen to uint64_t on the ABI.
Snapshots are atomic per counter, not transactional. 32-bit targets do not require
64-bit atomics. Native VM operations are not signal/interrupt-handler-safe.

## VM contract (CAP_VM)

| Operation | Contract |
| --- | --- |
| page_size | Stable nonzero power of two; failure is 0, not an invented page size |
| reserve | Round nonzero size to pages with checked arithmetic; alignment 0 or power of two, at least page size; return inaccessible owned reservation; flags currently 0; NULL output on failure |
| commit | Page-aligned owned subrange; read/write access; repeated commit preserves contents; Linux overcommit does not guarantee physical availability |
| decommit | Keep reservation, remove access, discard data; recommit is zeroed; preserve neighbors |
| release | Release full owned reservation with original or rounded size; portable partial release is not offered |
| reset | Contents disposable but pages remain accessible; no immediate reclamation/zero guarantee |

Linux uses anonymous mmap, mprotect, fixed anonymous replacement for decommit,
and madvise for reset. MAP_FIXED must only be used on a caller-owned reservation.
Failures are returned without managed allocation. The wrapper does not promise
transactional rollback of OS side effects. A host must document its own failure
state. An invalid successful pointer from a host is refused; a violating host
could still leak its allocation and is not made safe by this check.

Large pages and write-watch are unsupported. NUMA placement is an advisory hint
ignored by the pinned adapter. Large-page requests fail instead of falling back to
another allocator and then mixing ownership domains.

## Linear contract (CAP_LINEAR)

The backend has an 8 MiB arena and 4 KiB allocation granularity, not WebAssembly's
memory page size. Allocate zeroes the rounded storage. Release must specify the
whole live allocation; blocks can then be reused. Zero accepts a byte subrange
inside exactly one live allocation. Metadata tracks allocation heads/tails and
rejects invalid/partial/double releases, except that reused addresses cannot be
distinguished from stale pointers (ABA is a caller-lifetime obligation).

An atomic lock protects bookkeeping; callers synchronize direct payload access
with zero/release. There is no std, heap allocator, memory.grow, storage-related
host import, physical decommit, sparse address reservation or hardware protection.
It must NOT advertise CAP_VM. The pinned NativeAOT VM adapter rejects a linear-only
table. A managed GC using linear memory needs its own explicit adaptation and tests.

The lock is not recursive or interrupt-safe. On Wasm, panic traps; on native
freestanding linear builds, panic halts in a loop. This bounded validation profile
is not a general-purpose allocator or a production fatal-error policy.

## Clock and scheduling (CAP_CLOCK, CAP_SCHEDULER)

These are independent of storage capabilities. Linux and `host-services` provide
both; `wasi-clock` provides only a monotonic clock. Bare linear and legacy host
profiles expose neither. Clock timestamps are unsigned nanoseconds from an
unspecified host epoch, not UTC. Sleep retries interruption; yield is only a
scheduling hint. Invalid outputs are rejected and valid failed outputs are cleared.
The source GC adapter converts time units explicitly and aborts on a missing
required service or callback failure instead of reporting false success.
See [services.md](services.md) for exact contracts, provider lifetimes and tests.

## Wasm linkage and evidence

The C caller and Rust library are linked into the SAME core Wasm module. Pointers
and callbacks refer to that module's linear memory/function table: not JS native
pointers, cross-instance pointers, a wire format or Component Model ABI.
The three `linear` target tests deliberately import no host syscalls. A separate
`wasi-clock` build imports only `wasi_snapshot_preview1.clock_time_get`, is tested
against Node's real WASI implementation, and has an isolated error-injection test.
None of these modules uses shared memory/threads or contains C# code.

Native baseline/positive probes demonstrate actual GC calls reach the Rust VM
path, not just application P/Invoke. The GNU --wrap probe only interposes undefined
references and only the VM group. Its service counters must remain zero.
Source-runtime jobs on x64 and ARM64 rebuild the native runtime and test VM plus
clock/scheduling adapters without wrapping, using an observer-only object and a
private link overlay. Clock counters must increase during the managed workload;
sleep/yield usage is logged but workload-dependent. Compiler/BCL are matching
published versions, not rebuilt. CI logs at a revision are evidence; workflow
definitions and patch applicability alone are not.

## Remaining dependencies

| Area | Status |
| --- | --- |
| GC VM methods | ABI, both backends, adapters and execution tests |
| GC clock/scheduling methods | Five source-adapted definitions; Linux/host-services; C ABI and managed counter tests |
| WASIp1 monotonic clock | Real import and failure tests; not a managed Wasm runtime |
| Linear storage | Separate capability, native/Wasm contract tests; not GC integration |
| GC locks/events/barriers/affinity | Not redirected |
| Runtime PAL, threads, TLS, stack/context | Not redirected |
| Suspension, hardware exceptions, EH/unwind | Not redirected |
| BCL clocks/shims, files, network | Not redirected |
| Codegen, metadata, relocations, startup, linker | Separate target-port work |

Before adding a group: inventory calls, specify ownership/startup/failure rules,
add failure/concurrency tests, and execute a source-rebuilt runtime configuration.
See readiness.md for product qualification gates.
