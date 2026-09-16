#include "gc_linear_adapter.h"
#include <cstring>
#include <cstdint>
#include <cstdlib>
// These symbols are checked against the exact, SHA-512-pinned target runtime pack.
// --wrap redirects external references; a later source rebuild must close same-object paths.
#ifndef DOTNET_PAL_OBSERVER_ONLY
extern "C" void *__wrap__ZN15GCToOSInterface14VirtualReserveEmmjt(size_t n, size_t a, uint32_t f, uint16_t node) {
    return dotnet_pal_gc_linear::reserve(n, a, f, node);
}
extern "C" bool __wrap__ZN15GCToOSInterface13VirtualCommitEPvmt(void *p, size_t n, uint16_t node) {
    return dotnet_pal_gc_linear::commit(p, n, node);
}
extern "C" bool __wrap__ZN15GCToOSInterface15VirtualDecommitEPvm(void *p, size_t n) {
    return dotnet_pal_gc_linear::decommit(p, n);
}
extern "C" bool __wrap__ZN15GCToOSInterface14VirtualReleaseEPvm(void *p, size_t n) {
    return dotnet_pal_gc_linear::release(p, n);
}
extern "C" bool __wrap__ZN15GCToOSInterface12VirtualResetEPvmb(void *p, size_t n, bool unlock) {
    return dotnet_pal_gc_linear::reset(p, n, unlock);
}
extern "C" void *__wrap__ZN15GCToOSInterface33VirtualReserveAndCommitLargePagesEmt(size_t n, uint16_t node) {
    return dotnet_pal_gc_linear::large_pages(n, node);
}
static uint64_t now() {
    const auto *p = dotnet_pal_gc_linear::require();
    uint64_t ns = 0;
    if (p->header.struct_size < DOTNET_PAL_SERVICES_API_SIZE ||
        !(p->header.capabilities & DOTNET_PAL_CAP_CLOCK) || !p->services.monotonic_ns ||
        p->services.monotonic_ns(&ns) != DOTNET_PAL_OK || ns > INT64_MAX) std::abort();
    return ns;
}
extern "C" int64_t __wrap__ZN15GCToOSInterface23QueryPerformanceCounterEv() { return static_cast<int64_t>(now()); }
extern "C" int64_t __wrap__ZN15GCToOSInterface25QueryPerformanceFrequencyEv() { return 1000000000; }
extern "C" uint64_t __wrap__ZN15GCToOSInterface24GetLowPrecisionTimeStampEv() { return now() / 1000000; }
#endif

extern "C" uint32_t dotnet_pal_linear_probe_stats(dotnet_pal_linear_stats *storage,
        dotnet_pal_gc_linear::Stats *adapter, dotnet_pal_services_stats *services) {
    if (!storage || !adapter || !services) return DOTNET_PAL_INVALID_ARGUMENT;
    const auto *p = dotnet_pal_gc_linear::require();
    uint32_t status = p->linear.read_stats(storage, sizeof(*storage));
    if (status) return status;
    *adapter = dotnet_pal_gc_linear::snapshot();
    if (p->header.struct_size < DOTNET_PAL_SERVICES_API_SIZE || !p->services.read_stats)
        return DOTNET_PAL_UNSUPPORTED;
    return p->services.read_stats(services, sizeof(*services));
}

// Read-only geometry observer; never allocates or increments operation counters.
extern "C" uint64_t dotnet_pal_linear_probe_capacity() {
    return dotnet_pal_gc_linear::require()->linear.capacity();
}
