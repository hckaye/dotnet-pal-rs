#include "gc_vm_adapter.h"

// Test-only GNU/ELF link interposition, not the final console integration method.
// These Itanium symbols are checked against the installed runtime archive before
// linking. --wrap redirects undefined references; it cannot rewrite inlined or
// same-object references. The source patch has no such link-time restriction.
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

// The C# probe calls ONLY this observation function, not VM operations.
extern "C" uint32_t dotnet_pal_probe_stats(dotnet_pal_stats *out, size_t size) {
    const auto *p = dotnet_pal_gc::api();
    return p ? p->read_stats(out, size) : DOTNET_PAL_UNSUPPORTED;
}
