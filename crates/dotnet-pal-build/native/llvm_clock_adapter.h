#ifndef DOTNET_PAL_LLVM_CLOCK_ADAPTER_H
#define DOTNET_PAL_LLVM_CLOCK_ADAPTER_H
#include "dotnet_pal.h"
#include <cstdint>
#include <cstdlib>
namespace dotnet_pal_llvm_clock {
inline uint64_t now() {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    uint64_t ns = 0;
    if (!p || p->header.abi_version != DOTNET_PAL_ABI_VERSION ||
        p->header.struct_size < DOTNET_PAL_SERVICES_API_SIZE ||
        !(p->header.capabilities & DOTNET_PAL_CAP_CLOCK) || !p->services.monotonic_ns ||
        p->services.monotonic_ns(&ns) != DOTNET_PAL_OK || ns > INT64_MAX) std::abort();
    return ns;
}
}
#endif
