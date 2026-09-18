#ifndef DOTNET_PAL_MACHINE_ADAPTER_H
#define DOTNET_PAL_MACHINE_ADAPTER_H
#include "dotnet_pal.h"
#include "support_adapter.h"
#include <climits>
#include <cstdlib>
#include <limits>
namespace dotnet_pal_machine {
inline const dotnet_pal_machine_ops *api() {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!p || p->header.abi_version != DOTNET_PAL_ABI_VERSION ||
        p->header.struct_size < DOTNET_PAL_MACHINE_API_SIZE ||
        !(p->header.capabilities & DOTNET_PAL_CAP_MACHINE)) return nullptr;
    const auto &m = p->machine;
    return m.query && m.process_affinity && m.bind_current && m.current_cpu ? &m : nullptr;
}
inline bool query(uint32_t kind, uint64_t *out) {
    const auto *m = api();
    return m && m->query(kind, out) == DOTNET_PAL_OK;
}
inline long long_query(uint32_t kind) {
    uint64_t v = 0;
    return query(kind, &v) && v <= LONG_MAX ? static_cast<long>(v) : -1;
}
inline long physical_pages() {
    uint64_t bytes = 0, page = 0;
    if (!query(DOTNET_PAL_MACHINE_PHYSICAL_BYTES, &bytes) ||
        !query(DOTNET_PAL_MACHINE_PAGE_BYTES, &page) || !page || bytes / page > LONG_MAX) return -1;
    return static_cast<long>(bytes / page);
}
inline uint64_t available_bytes() {
    uint64_t bytes = 0;
    (void)query(DOTNET_PAL_MACHINE_AVAILABLE_BYTES, &bytes);
    return bytes;
}
inline uint64_t swap_bytes() {
    uint64_t bytes = 0;
    (void)query(DOTNET_PAL_MACHINE_SWAP_BYTES, &bytes);
    return bytes;
}
template<class Limit> inline int read_limit(Limit *out) {
    uint64_t v = 0;
    if (!query(DOTNET_PAL_MACHINE_ADDRESS_LIMIT, &v)) return -1;
    if (v == UINT64_MAX) *out = std::numeric_limits<Limit>::max();
    else if (v <= std::numeric_limits<Limit>::max()) *out = static_cast<Limit>(v);
    else return -1;
    return 0;
}
inline uint32_t affinity_count() {
    const auto *m = api(); size_t n = 0;
    if (!m || m->process_affinity(nullptr, 0, &n) != DOTNET_PAL_BUFFER_TOO_SMALL ||
        !n || n > DOTNET_PAL_MACHINE_MAX_CPUS) return 0;
    return static_cast<uint32_t>(n);
}
template<class Set> bool initialize_affinity(Set &set, uint32_t possible) {
    const auto *m = api();
    if (!m || !possible || possible > DOTNET_PAL_MACHINE_MAX_CPUS) return false;
    size_t n = affinity_count();
    // Administrative affinity changes between sizing and copying are bounded retries.
    for (int retry = 0; retry < 4 && n != 0; ++retry) {
        if (n > DOTNET_PAL_MACHINE_MAX_CPUS) return false;
        auto *cpus = static_cast<uint32_t *>(dotnet_pal_support::allocate(n * sizeof(uint32_t)));
        if (!cpus) return false;
        size_t actual = 0;
        uint32_t result = m->process_affinity(cpus, n, &actual);
        if (result == DOTNET_PAL_OK) {
            bool valid = actual > 0 && actual <= n;
            for (size_t i = 0; valid && i < actual; ++i)
                valid = cpus[i] < possible && (i == 0 || cpus[i-1] < cpus[i]);
            if (valid) for (size_t i = 0; i < actual; ++i) set.Add(cpus[i]);
            dotnet_pal_support::release(cpus);
            return valid;
        }
        dotnet_pal_support::release(cpus);
        if (result != DOTNET_PAL_BUFFER_TOO_SMALL || actual <= n) return false;
        n = actual;
    }
    return false;
}
inline uint32_t current_cpu() {
    const auto *m = api(); uint32_t cpu = UINT32_MAX;
    if (!m || m->current_cpu(&cpu) != DOTNET_PAL_OK || cpu >= DOTNET_PAL_MACHINE_MAX_CPUS) std::abort();
    return cpu;
}
inline bool bind_current(uint32_t cpu) {
    const auto *m = api();
    return m && m->bind_current(cpu) == DOTNET_PAL_OK;
}
}
#endif
