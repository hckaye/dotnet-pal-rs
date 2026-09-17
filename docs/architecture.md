# Boundary architecture and contracts

## One logical entry, separate capability groups

`dotnet_pal_get_api(2)` returns an immutable, process/module-lifetime C table or
NULL. Consumers validate its version, declared size, capability bits and required
callbacks before reading a group. Appended groups preserve the older prefix.
An unsupported group is absent, not an unreported fallback to a different allocator.

The current root contains VM, diagnostic VM counters, linear storage, clocks/
scheduling, and kernel operations. Runtime-specific adapters translate audited
.NET methods to these groups. The public C ABI does not contain .NET classes,
pthread structures, Rust enums or Rust trait objects. It is not a serialized or
cross-Wasm-instance format. No extension-query mapping/loader group has been
implemented in this revision.

The crate uses `no_std` and no Rust heap allocator. Linux kernel objects still
need native allocation. Host callbacks can be implemented in Rust, C or C++ and
must exist before managed startup; they must not unwind, reenter managed allocation
or recursively call the same boundary operation. Required malformed tables reject
negotiation. Pointer validity, resource lifetime and destruction synchronization
remain unsafe caller obligations, not something null/alignment checks can prove.

## VM versus linear storage

VM reserves inaccessible address space, commits read/write pages, preserves data
on repeated commit, and decommits while retaining the reservation. Recommit must
be zero-filled and adjacent regions must be preserved. Linux implements this with
anonymous mappings/protection/replacement. Full release uses the original or
rounded size. Write-watch and large pages are not silently emulated. NUMA placement
is an advisory hint not implemented by the current VM adapter.

Linear storage is a different capability. Its bounded arena is eagerly accessible,
uses 4 KiB allocation units and tracks whole allocations. It rejects invalid
geometry/partial release and can reuse freed blocks, but cannot distinguish stale
pointers from subsequently reused addresses. Metadata is locked; the caller must
synchronize direct payload access with zero/release. The lock is not interrupt-safe.

The LLVM GC linear adapter explicitly accepts weaker eager-storage semantics:
its bounded region ledger tracks ownership; reserve allocates zeroed storage;
logical commit preserves it; logical decommit zeros it without changing access
permissions or releasing physical Wasm pages. This is never advertised as CAP_VM.
The regular arena is 8 MiB, the opt-in managed experiment 256 MiB. Neither grows
memory. The initial Wasm module footprint includes the arena even for the baseline.

## Kernel and clocks

Clock epoch is unspecified, monotonic nanoseconds, not UTC. Native relative sleep
retries interruptions; yield is a hint. The WASIp1 profile imports the real clock
function and advertises no scheduling implementation.

Linux events implement manual/auto reset and monotonic timed waits; unexpected
wakeups recheck state. Mutexes can be recursive. Thread callbacks use a typed
trampoline; native handles are joined or detached explicitly. TLS destructor and
thread-attachment integration is tested. Stack bounds refer to the current native
thread. Process barriers use the actual Linux membarrier facility, not a local
compiler/atomic fence. Handle APIs are not async-signal/interrupt safe and do not
support concurrent destruction or asynchronous thread cancellation.

## Native integration

A guarded patch at the pinned .NET source routes GC memory, clocks, events,
critical sections and process barriers, plus selected runtime event/Crst,
background-thread, termination-TLS, stack and scheduling operations. The native
component is rebuilt for both GC variants. Published compiler/BCL artifacts remain
pinned and are not rebuilt. Each rebuilt archive has an independent digest checked
against the archive actually chosen by MSBuild. No source variant uses --wrap.

The ordinary link-interposition experiment remains a useful negative control but
cannot rewrite inlined or same-object calls. Its claims are correspondingly narrow.
The managed probes call only statistics observers; they do not manufacture runtime
coverage by directly invoking VM/kernel functions from C#.

## Managed Wasm integration

Experimental NativeAOT LLVM is a separate compiler family with an independently
audited revision and host/target packages. The selected link profile is WASIp1,
although that compiler package defaults to WASIp2. Native runtime cross-building
explicitly selects the P1 SDK toolchain, replaces six Wasm GC memory definitions
and three clock definitions, and links the rebuilt runtime with an observer-only
object. The final module's imports and absence of wrapper helpers are checked.

C# allocation/GC root preservation/zeroing/exception tests execute in a real Node
WASI host. The positive variant must increment Rust storage/adapter/clock counters;
the otherwise identical baseline must leave them zero. This is actual managed
execution, unlike the separate C-to-Rust boundary module tests. It still does not
establish shared Wasm threads or all BCL functionality.

## Browser host boundary

The browser port (`examples/browser-port`) replaces WASI imports with five
typed imports from the page's JavaScript: monotonic clock, wall clock, entropy, environment lookup and
diagnostic output. The Rust front ends stay the same, so the validation that
protects native callers (cleared outputs on failure, unknown statuses mapped to
`OS_ERROR`, zero-size requests never entering the host) also applies to a page.
The reference host bounds-checks every offset against the live memory and
refuses shared memory. Services a browser cannot provide, above all a blocking
sleep, are absent rather than emulated.

Its `grow` variant owns its storage: `memory.grow` pages feed the same
ownership ledger as the C-hook variant, so growth is observable from the page
and bounded by the engine or a linker memory maximum. Its `heap` variant links
with the NativeAOT LLVM runtime; the C# program in the example executes in
headless Chromium with GC storage and the runtime clock crossing the boundary.
See [wasm](wasm.md) and [porting](porting.md).

## Residual dependencies and completion

Dependency inventories separate archive references from final dynamic imports and
retain unknown symbols for review. Direct signal/context, module/loader,
process/topology/environment, diagnostics and native allocation dependencies still
exist outside the implemented groups; BCL shims also retain OS dependencies.
The explicit isolation gate fails in that state. Passing functional qualification
means the selected configurations worked, not that all OS access is centralized.

Code generation, target calling conventions, relocation/metadata formats, startup
and EH/stack-map support are not magically supplied by Rust target support. They
must match the chosen native/Wasm toolchain. The tested configurations reuse
existing target implementations of those responsibilities.

See [readiness](readiness.md) for uncompleted work and [qualification](qualification.md)
for executable acceptance tests rather than inferred support claims.
