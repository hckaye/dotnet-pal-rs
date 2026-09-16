# Demand-allocated linear storage

`linear-heap` is an alternative implementation of CAP_LINEAR, additionally marked
CAP_DYNAMIC_LINEAR. It reserves no payload arena at startup. Only a fixed 4,096-entry
ownership ledger is static; storage is requested when allocations arrive. A hard
live-byte budget is preserved (8 MiB by default, 64 MiB with linear-gc-small, or
256 MiB with linear-gc). The allocator may fail earlier due to its own limits,
alignment overhead, fragmentation, or the ledger limit.

The embedder supplies the versioned storage allocate/release hooks in dotnet_pal.h.
They must use one allocator, provide exclusive aligned memory, fail before side
effects, and never reenter PAL or managed code. The reference POSIX backend uses
posix_memalign/free. In WASIp1 this SHARES wasi-libc's allocator with the native
runtime and BCL. Calling memory.grow independently would invalidate libc's program
break assumptions; this implementation does not do so. libc performs real memory
growth as necessary, and the entire module retains its 128 MiB engine limit.

Allocation zeroes storage even if the provider returned dirty bytes. The Rust
ledger rejects foreign, partial, overlapping-success, and double-release requests.
A failed release retains ownership and budget. No operation makes pages inaccessible;
freeing permits allocator reuse but does not shrink Wasm memory. Stale pointer/ABA
protection, asynchronous cancellation and interrupt-safe operation are not supplied.
Malformed provider success is a provider contract failure, not a sandbox boundary.

Run scripts/linear-heap.sh for native eight-thread, zeroing, budget and fault tests.
The same Rust plus C test is instrumented by BOTH sanitizer configurations.
scripts/llvm-dynamic.sh (after llvm-source setup) links the unchanged source-built
managed runtime to this backend with NO linker wrapping. The original managed
roots/OOM/recovery/finalizer/EH/BCL tests remain mandatory, as does the single-host
import audit. It additionally asserts a small startup footprint and actual linear
memory growth. Workflow logs for the exact commit, not this description, determine
whether the configuration passed.
