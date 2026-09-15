#include "gc_vm_adapter.h"
#include <cassert>
#include <cstdio>

// A logical arena must never be silently accepted by the native GC adapter.
static const dotnet_pal_api linear = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_api), DOTNET_PAL_CAP_VM_LINEAR | DOTNET_PAL_CAP_ZERO_RECOMMIT},
    {}, nullptr
};
extern "C" const dotnet_pal_api *dotnet_pal_get_api(uint32_t version) {
    return version == DOTNET_PAL_ABI_VERSION ? &linear : nullptr;
}
int main() {
    assert(dotnet_pal_gc::api() == nullptr);
    assert(dotnet_pal_gc::reserve(65536, 65536, 0, 0) == nullptr);
    assert(!dotnet_pal_gc::commit(nullptr, 65536, 0));
    std::puts("PASS: native GC adapter rejects linear-memory capability profile");
}
