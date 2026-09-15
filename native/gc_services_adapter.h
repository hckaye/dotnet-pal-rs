#ifndef DOTNET_PAL_GC_SERVICES_ADAPTER_H
#define DOTNET_PAL_GC_SERVICES_ADAPTER_H
#include "dotnet_pal.h"
#include <stdlib.h>
#include <stdint.h>

namespace dotnet_pal_gc_services {
inline const dotnet_pal_api *api() {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    const uint64_t required = DOTNET_PAL_CAP_CLOCK | DOTNET_PAL_CAP_SCHEDULER;
    if (!p || p->header.abi_version != DOTNET_PAL_ABI_VERSION ||
        p->header.struct_size < DOTNET_PAL_SERVICES_API_SIZE ||
        (p->header.capabilities & required) != required) return nullptr;
    return p->services.monotonic_ns && p->services.sleep_ns &&
        p->services.yield_thread ? p : nullptr;
}
// The upstream APIs cannot report an OS-service failure. Fail closed rather than
// fabricating a timestamp, returning early from sleep or busy-looping silently.
inline const dotnet_pal_services_ops &required() {
    const auto *p = api();
    if (!p) abort();
    return p->services;
}
inline uint64_t monotonic_ns() {
    uint64_t ns = 0;
    if (required().monotonic_ns(&ns) != DOTNET_PAL_OK) abort();
    return ns;
}
inline int64_t counter() {
    uint64_t ns = monotonic_ns();
    if (ns > static_cast<uint64_t>(INT64_MAX)) abort();
    return static_cast<int64_t>(ns);
}
inline int64_t frequency() { (void)required(); return INT64_C(1000000000); }
inline uint64_t lowres_ms() { return monotonic_ns() / UINT64_C(1000000); }
inline void sleep_ms(uint32_t ms) {
    if (required().sleep_ns(static_cast<uint64_t>(ms) * UINT64_C(1000000)) != DOTNET_PAL_OK) abort();
}
inline void yield_thread(uint32_t /* switchCount */) {
    if (required().yield_thread() != DOTNET_PAL_OK) abort();
}
}
#endif
