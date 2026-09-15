# Kernel-service boundary

The appended ABI 2 kernel group implements events, recursive/nonrecursive mutexes,
joinable/detachable threads, OS TLS keys, current-thread stack bounds and a
process-wide memory barrier. VM, services and linear prefixes are unchanged.
Every consumer checks version, group-end size, capability bits and callbacks.
This document specifies implemented contracts, not universal target support.

## Handles and lifetime

Handles are opaque non-null values, not transferable OS structures or checked
security tokens. Create operations clear a valid output on error; successful
create must return a non-null handle. A successful close/join/detach consumes that
handle. Foreign output storage must be valid, writable and properly aligned.
Operations on arbitrary or stale pointers remain undefined caller behavior.

The caller serializes lifecycle changes, waits for users to finish, and prohibits
asynchronous cancellation and unwinding through C callbacks. Destroying an active
event reports BUSY when active waiters are observable; this is not a guarantee
that concurrent destruction is safe. A locked mutex reports BUSY on destruction.
Retry is safe only when the backend documents that no destructive side effects
occurred. OS errors are not universally transactional.

All host tables are immutable before first publication, usable before managed
startup, and live until process termination. Callbacks must not reenter managed
code or recursively call the same boundary operation. Legacy `host` and
`host-services` builds need no kernel provider. `host-kernel` additionally requires
`dotnet_pal_host_kernel_v2`, all kernel capabilities and every operation callback.
The host table's statistics slot is not used: the common layer owns observations.

## Event contract

Create flags are 0 or 1. Manual-reset events remain signaled until reset;
auto-reset events release one waiter or remember one signal. Repeated sets
coalesce; they are not semaphore increments. Wait uses relative nanoseconds:
zero is a poll, UINT64_MAX is infinite. Finite waits use a monotonic absolute
deadline internally, so spurious wakeups do not extend the timeout. TIMEOUT is a
normal wait outcome, distinct from OS_ERROR and UNSUPPORTED. Reset clears either
kind of event, matching the pinned Unix runtime implementation.

The Linux backend holds a native mutex around state and condition waits. It uses
CLOCK_MONOTONIC condition variables and rechecks state after timeout reacquires
the mutex. No clock/scheduler fallback spins on a missing service. There is no
APC/alertable wait support; the pinned Unix GC/runtime likewise ignores that flag.

## Threads, TLS and stacks

Thread creation returns a joinable handle. A stack size of zero selects the native
default; unsupported explicit sizes fail. Join waits for native termination,
including TLS destructors. Detach releases the handle but does not wait or cancel
the worker. Argument storage stays live until the worker and its destructors are
done. TLS values are per native thread; deleting a key does not run destructors.
Clearing a value suppresses its destructor. Destructor reinstallation follows the
host's native TLS rules; the Linux provider uses pthread keys.

The current native stack bounds are a half-open [low, high) address interval.
This does not enumerate managed roots, capture registers, suspend another thread,
or define a Wasm shadow-stack layout. Compiler TLS fast paths remain separate
from the OS TLS key used for NativeAOT thread-termination notification.

## Process-wide barriers

A local Rust atomic fence is NOT a process-wide barrier. Linux probes membarrier,
registers private-expedited use once, or selects the kernel global barrier if
available. If neither is available, the capability and callback are absent.
The source-runtime profile rejects that configuration instead of substituting a
weaker operation. Later syscall failures propagate; the GC adapter fails fast.
The host reference provider uses the actual membarrier syscall, not a mock fence.

## Runtime integration and evidence

The guarded source patch additionally redirects:

- GCEvent and runtime UnixEvent operations;
- GC CLRCriticalSection and runtime Crst locking;
- PalStartBackgroundWork (GC/finalizer/helper thread creation);
- PalAttachThread termination TLS registration;
- PalGetMaximumStackBounds, PalSleep and PalSwitchToThread;
- GCToOSInterface::FlushProcessWriteBuffers, including calls via runtime PAL.

A typed native trampoline bridges the runtime's uint32-returning background
callback rather than casting it to an incompatible pthread function pointer.
C++ bridge allocations use the native CRT; the Rust common layer requires no
Rust `std`/`alloc`. Linux kernel resources use native CRT allocations too, which
are independent of the managed heap. There is no allocator-free guarantee for
creating OS synchronization objects or threads.

PalInit checks all required VM, clock/scheduler and kernel groups. Unexpected
errors in void-returning synchronization methods are fatal because the existing
runtime cannot recover safely. Creation and waits retain the upstream failure
channels. No global destruction is introduced for process-lifetime GC events.

Tests run the same C suite against Rust Linux and independent POSIX C providers,
exercise invalid host contracts, and verify actual event/lock/TLS/thread/stack/
barrier activity through observer-only C# imports after a source rebuild. The
baseline and VM-only probes must leave kernel activity at zero. Source-runtime
CI, not transformer unit tests or archive compilation, is execution evidence.

Not redirected by this group: managed Thread creation in BCL native shims,
hijacking/signal/context handling, compiler-generated TLS, native code unwinding,
all native heap allocation, or arbitrary BCL OS access. These are distinct audit
items; none is implied by the presence of a kernel capability.
