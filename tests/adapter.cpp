#include "gc_vm_adapter.h"
#include <cassert>

static unsigned calls;
static bool enabled = true;
static unsigned char storage[4096];
static uint32_t reserve(size_t n, size_t a, uint32_t f, void **out) {
    ++calls;
    assert(n == 123 && a == 4096);
    *out = f ? nullptr : storage;
    return f ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OK;
}
static uint32_t range(void *p, size_t n) {
    ++calls;
    assert(p == storage && n == 123);
    return DOTNET_PAL_OK;
}
static const dotnet_pal_api API = {
    { DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_api), DOTNET_PAL_CAP_VM },
    { nullptr, reserve, range, range, range, range }, nullptr
};
extern "C" const dotnet_pal_api *dotnet_pal_get_api(uint32_t version) {
    assert(version == DOTNET_PAL_ABI_VERSION);
    return enabled ? &API : nullptr;
}
int main() {
    assert(dotnet_pal_gc::reserve(123, 4096, 0, UINT16_MAX) == storage);
    assert(dotnet_pal_gc::reserve(123, 4096, 1, UINT16_MAX) == nullptr);
    assert(dotnet_pal_gc::commit(storage, 123, UINT16_MAX));
    assert(dotnet_pal_gc::decommit(storage, 123));
    assert(dotnet_pal_gc::reset(storage, 123, false));
    assert(dotnet_pal_gc::release(storage, 123));
    assert(dotnet_pal_gc::large_pages(123, UINT16_MAX) == nullptr);
    assert(calls == 6);
    enabled = false;
    assert(dotnet_pal_gc::reserve(123, 4096, 0, 0) == nullptr);
    assert(!dotnet_pal_gc::commit(storage, 123, 0));
    assert(calls == 6); // no silent fallback to libc/upstream allocator
}
