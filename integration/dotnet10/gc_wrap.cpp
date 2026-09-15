#include "gc_vm_adapter.h"

// Test-only GNU/ELF link interposition, not the source integration method.
// These Itanium symbols are checked against the installed runtime archive before
// linking. --wrap redirects undefined references; it cannot rewrite inlined or
// same-object references. The source patch has no such link-time restriction.
#ifndef DOTNET_PAL_OBSERVER_ONLY
extern "C" void *__wrap__ZN15GCToOSInterface14VirtualReserveEmmjt(size_t n, size_t a, uint32_t f, uint16_t node) {
    return dotnet_pal_gc::reserve(n, a, f, node);
}
extern "C" bool __wrap__ZN15GCToOSInterface13VirtualCommitEPvmt(void *p, size_t n, uint16_t node) {
    return dotnet_pal_gc::commit(p, n, node);
}
extern "C" bool __wrap__ZN15GCToOSInterface15VirtualDecommitEPvm(void *p, size_t n) {
    return dotnet_pal_gc::decommit(p, n);
}
extern "C" bool __wrap__ZN15GCToOSInterface14VirtualReleaseEPvm(void *p, size_t n) {
    return dotnet_pal_gc::release(p, n);
}
extern "C" bool __wrap__ZN15GCToOSInterface12VirtualResetEPvmb(void *p, size_t n, bool unlock) {
    return dotnet_pal_gc::reset(p, n, unlock);
}
extern "C" void *__wrap__ZN15GCToOSInterface33VirtualReserveAndCommitLargePagesEmt(size_t n, uint16_t node) {
    return dotnet_pal_gc::large_pages(n, node);
}

#endif

// The C# probe calls ONLY this observation function, not VM operations.
extern "C" uint32_t dotnet_pal_probe_stats(dotnet_pal_stats *out, size_t size) {
    const auto *p = dotnet_pal_gc::api();
    return p && p->read_stats ? p->read_stats(out, size) : DOTNET_PAL_UNSUPPORTED;
}

// Observation only: this never calls the clock or scheduler operations.
extern "C" uint32_t dotnet_pal_probe_services_stats(dotnet_pal_services_stats *out, size_t size) {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!p || p->header.struct_size < DOTNET_PAL_SERVICES_API_SIZE || !p->services.read_stats)
        return DOTNET_PAL_UNSUPPORTED;
    return p->services.read_stats(out, size);
}

// Observation only; the baseline/VM-only paths must leave kernel counters zero.
extern "C" uint32_t dotnet_pal_probe_kernel_stats(dotnet_pal_kernel_stats *out, size_t size) {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!p || p->header.struct_size < DOTNET_PAL_KERNEL_API_SIZE || !p->kernel.read_stats)
        return DOTNET_PAL_UNSUPPORTED;
    return p->kernel.read_stats(out, size);
}
