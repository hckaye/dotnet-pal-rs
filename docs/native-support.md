# Native helper-service boundary

The append-only `support` group is accessed through `dotnet_pal_get_api(2)`.
It separates native helper allocation, reader/writer locks, thread naming and
fatal diagnostic output from GC virtual memory. Check `DOTNET_PAL_SUPPORT_API_SIZE`
and the requested capability before reading a callback. Older host configurations
need no new symbols. `host-support` explicitly requires an immutable host table;
malformed tables fail negotiation rather than supplying success stubs.

## Contracts

Allocation is ordinary C-allocator storage, not page reservation or managed GC
storage. Successful allocations have the target C allocator alignment. Size zero
and sizes greater than `PTRDIFF_MAX` are rejected. Resize preserves the old prefix
on success and the entire old allocation on failure; failure clears the result,
not the caller's owning pointer. Resize-to-zero is deliberately not a free API.
Release of NULL is permitted. Foreign pointers, concurrent resize/free and
cross-allocator frees remain caller errors, not security-checked operations.

RW locks permit concurrent readers and exclude writers. Destruction needs
exclusive lifetime ownership, no holders and no potential entrants. Linux uses
real pthread rwlocks; the common ABI contains only opaque handles.

Stderr writes retry EINTR and partial writes until the request completes or an
error occurs. On error, validated progress is returned; short successful host
reports are rejected. A successful table negotiation caches immutable callbacks,
so signal-time output does not rediscover a foreign provider. Only diagnostic
output, not allocation or locking, is promised signal-safe. The NativeAOT fatal
adapter uses a previously published table and never lazily initializes it.

Names contain no embedded NUL. The Linux provider permits at most 15 bytes; the
Linux-specific NativeAOT adapter trims long names and leaves the process's main
thread unchanged, preserving the upstream behavior.

## Actual source integration

At the pinned native runtime revision, the source adapter routes:

- paired native allocations in dump argument formatting and libunwind;
- libunwind cache reader/writer locking, preinitialized before signal handlers;
- runtime thread names, fatal messages and system FILETIME;
- GC logging thread/process identity through the existing runtime service group.

Only matched allocation/free pairs are rerouted. C++ new/delete, getline/asprintf
storage and unrelated BCL allocations retain their original allocator. There is
no global symbol interposition. CoreCLR and other builds without the feature
macro retain the original code. Source anchor and revision drift are rejected.

Cache growth now preserves the old cache and releases its lock on allocation
failure instead of copying through a NULL pointer. Overflow also declines cache
growth. This is an optional cache, not a change to exception metadata semantics.
The cache lock is allocated at NativeAOT initialization, not at first signal-time
stack walk. This does not make all libunwind operations asynchronous-signal-safe.

## Evidence producers

`scripts/support.sh` executes the Linux and host providers, shared-reader and
exclusive-writer tests, concurrent allocation/resize, signal-time diagnostics,
name roundtrips, malformed tables and failure-output contracts. It also executes
the C++ adapter's time conversion and concurrent initialization checks.

`scripts/unwind-cache.sh /patched/runtime` compiles the actual patched upstream
DwarfFDECache and tests OOM retention/retry and concurrent readers. Source-runtime
CI executes it before rebuilding both native collectors. The managed probe reads
statistics only; baseline and VM-only controls must not use these callbacks.
An extra dump-enabled startup (no crash, no dump generation) checks that actual
runtime initialization allocates its helper data through this boundary.

Both mixed-language sanitizers instrument the Rust and C support providers.
Results for an exact commit, not the existence of these tests, determine passage.
The whole-runtime dependency inventory remains strict: this group does not
complete loader inspection, topology, all diagnostics or every BCL service.
