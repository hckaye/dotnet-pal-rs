# Boundary contract and migration plan

## Goal, not a completed claim

There should eventually be one logical OS-service surface for a selected NativeAOT
configuration. Its implementation can use Rust `no_std` with a private SDK backend.
The compiler's code generation, calling convention and object/unwind formats are
not OS-service calls and cannot be made portable just by replacing a function table.
This repository currently implements only the GC VM capability group.

Keep .NET internals in version-specific adapters. Keep vendor structures out of
`dotnet_pal.h`. Add capability groups only with an upstream call-site inventory and
contract tests, rather than guessing hundreds of portable APIs in advance.
A C/C++ SDK backend is permitted: Rust is not a prerequisite for every SDK toolchain.

## ABI 2

The runtime-facing entry is `dotnet_pal_get_api(2)`, returning an immutable table
or NULL. The table is process-lifetime and available before managed initialization.
An older table is not layout-compatible just because a subset of names matches.
Version, byte size and capability bits are checked; unsupported flags fail closed.

Pointers are trusted native pointers, not security-checked handles. A caller must
supply valid writable output storage, own the entire supplied reservation range,
and serialize conflicting range operations. Null/alignment/overflow checks cannot
make arbitrary pointers safe. A valid-looking foreign pointer is not validation of
its ownership. The host table and its callbacks must be immutable and alive for
all callers. Unknown integer flags are rejected before invoking a backend.

The Rust core performs no allocation, locking, lazy OS initialization or managed
callbacks. No callback may throw, unwind, reenter managed allocation, acquire a
managed lock, or call back into the same PAL operation recursively. Abort is fatal.
This avoids depending on managed initialization to implement managed initialization.
The current code is not promised async-signal-safe; VM functions are not to be used
from signal handlers. Native 64-bit atomics are currently required for diagnostics.

## VM semantics

| Operation | Required behavior |
| --- | --- |
| page_size | Stable, nonzero power of two for the process lifetime; failure is 0, never an invented 4096 |
| reserve | Nonzero size is rounded up to pages with overflow detection. Alignment is 0 or a power of two, raised to page size. Own the whole inaccessible range. Set output to NULL on failure. Flags must currently be 0 |
| commit | Page-aligned start; rounded-up size within an owned reservation. Read/write access. Repeated commit preserves existing data. Linux overcommit does not promise physical memory availability |
| decommit | Keep reservation; remove access and discard old data. Recommit must return zero-filled pages. Preserve adjacent committed pages |
| release | Release the full owned reservation, using its original size or rounded size. Subrange release is not a portable contract |
| reset | Data becomes disposable, but the range remains committed/readable/writable. No guarantee about immediate reclamation or zeroes |

Linux uses anonymous mmap, mprotect, a fixed anonymous replacement for decommit,
and madvise for reset. MAP_FIXED is restricted by the caller-ownership contract;
it is not an API for manipulating arbitrary process mappings. OS failures return
OS_ERROR, with no managed allocation to construct error strings. The initial ABI
does not guarantee rollback of all backend side effects after an OS failure.
A backend must document its failure-state semantics before production use.

The common wrapper rounds ranges before calling the SDK host table. The host
receives page-multiple sizes and normalized alignment. Host callbacks must obey
the same ownership, accessibility and zero-fill guarantees. A preallocated-memory
arena without memory protection is NOT a conforming implementation of this VM
capability; a weaker capability would require explicit runtime changes and tests.

Write-watch, explicit large pages, executable pages and NUMA placement are not
implemented. The adapter declines large-page requests; it never forwards those
requests to an unrelated upstream allocator. The current Linux adapter treats the
NUMA argument as a hint and the unlock argument as the pinned Unix code does.

Counters count successful operations and failed/rejected attempts, not bytes or
proof of complete call-site coverage. Snapshots are atomic per counter, not a
consistent transaction across counters. Counter wrap is possible after 2^64 events.

## Evidence levels

1. Rust/unit and C ABI tests establish local contracts.
2. A C++ adapter test establishes argument translation and no fallback.
3. The NativeAOT negative/positive controls establish that real GC calls reach Rust.
4. The host-backed NativeAOT control establishes that the same core can delegate to
   a replaceable SDK-shaped backend without std or a global allocator.
5. The freestanding ARM64 build establishes compilation of the boundary without
   Linux/std dependencies. It is neither runtime execution nor Switch validation.
6. The source-patch applicability test establishes only source compatibility at a
   pin. Full source runtime rebuild/stress coverage is a subsequent requirement.

The --wrap probe is intentionally not called a full port. It wraps six public
GCToOSInterface VM methods, checks their symbols and disables large pages. Undefined
references are interposed; local/inlined references and all unrelated OS calls can
remain. The proposed source adapter is the route toward complete call-site coverage.

## Switch-oriented backend

An authorized developer would provide host callbacks using their approved SDK,
then separately resolve NativeAOT code generation/ABI, TLS, unwinding, runtime
startup, GC coordination, BCL imports, native linking and packaging. No public SDK
function names or memory/exception behavior are assumed here. No console API is
mocked and presented as working. Keep confidential details and vendor-derived code
out of this public repository. The Linux mock tests the host protocol only.

## Dependency inventory to complete

| Area | Current status |
| --- | --- |
| GC reserve/commit/decommit/release/reset | ABI and adapters implemented; executable verification in CI |
| GC large pages/write-watch/NUMA policy | Unsupported or advisory-only; explicit scope restrictions |
| NativeAOT Runtime/Pal.h and Unix PAL | Not redirected |
| GC events, locks, memory barriers, affinity | Not redirected |
| Thread attachment, TLS, stack/context capture | Not redirected |
| Suspension, signal/hardware exceptions, unwinding | Not redirected |
| BCL native libraries, filesystem, networking | Not redirected |
| Compiler ABI, metadata, relocations, startup, SDK linker | Target-port work outside this OS-service ABI |

Before adding each group: inventory every call site, define ownership and startup
rules, add failure/concurrency tests, and run a source-rebuilt runtime configuration.
Do not mark a target supported merely because `cargo build --target` succeeds.
