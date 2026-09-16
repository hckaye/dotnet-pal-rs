# Unwinder synchronization boundary

The native source adapter patches the pinned vendored libunwind RWMutex class
rather than renaming symbols or wrapping the linker. Lock storage becomes an
opaque PAL handle. Both shared and exclusive acquisitions use the existing
recursive mutex capability, preserving exclusion and recursive shared use while
serializing readers. This is a conservative fallback, not a claim of parallel
reader throughput or equivalence of every pthread rwlock scheduling policy.

First-use publication is atomic. Competing initializers destroy their unused
candidate. The runtime's global lock has process lifetime; tests explicitly destroy
it only after all workers join. No managed callback or managed allocation occurs.
The operations are not async-signal-safe, just as the upstream pthread rwlock was
not. Signal handling, register contexts and exception metadata retain their
independently validated adapters; this change does not replace the EH algorithm.

Both native collector archives are checked for residual pthread_rwlock lock/unlock
references. The normal source qualification still requires real C# exception,
GC/root and finalizer workloads on x64 and ARM64. Separate eight-thread initialization,
recursive read, write and busy-destruction tests also run under ASan and TSan.
A missing capability causes acquisition to fail, never an unlocked success.
