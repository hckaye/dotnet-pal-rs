#ifndef DOTNET_PAL_TOPOLOGY_ADAPTER_H
#define DOTNET_PAL_TOPOLOGY_ADAPTER_H
// Topology group front end for the runtime and GC: CPU counts, affinity, memory
// figures, cache size and CPU feature words. The table is discovered once in
// PalInit; every accessor returns the port's answer or a documented default.
#include "dotnet_pal.h"
#include <atomic>
#include <cstdlib>
namespace dotnet_pal_topology {
inline std::atomic<const dotnet_pal_topology_ops*> installed{nullptr};
inline bool initialize() {
    auto *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION || a->header.struct_size < DOTNET_PAL_TOPOLOGY_API_SIZE
        || !(a->header.capabilities & DOTNET_PAL_CAP_TOPOLOGY)) return false;
    const auto *t = &a->topology;
    if (!t->cpu_max || !t->cpu_count || !t->current_cpu || !t->process_affinity || !t->set_thread_affinity
        || !t->physical_memory || !t->memory_limit || !t->virtual_limit || !t->cache_size || !t->cpu_features) return false;
    installed.store(t, std::memory_order_release);
    return true;
}
inline const dotnet_pal_topology_ops *require() {
    auto *t = installed.load(std::memory_order_acquire);
    if (!t) std::abort();
    return t;
}
// Highest possible logical CPU count; the GC sizes its affinity set from it.
inline uint32_t cpu_max() { uint32_t v = 0; if (require()->cpu_max(&v) != DOTNET_PAL_OK || v == 0) std::abort(); return v; }
// CPUs this process may use after affinity and quota; never zero.
inline uint32_t cpu_count() { uint32_t v = 0; if (require()->cpu_count(&v) != DOTNET_PAL_OK || v == 0) std::abort(); return v; }
inline bool current_cpu(uint32_t &out) { return require()->current_cpu(&out) == DOTNET_PAL_OK; }
// Calls add(cpu) for every CPU in the process affinity set; false when unknown.
template <typename Add> inline bool affinity(uint32_t max_cpus, Add add) {
    uint8_t mask[8192]; size_t needed = 0;
    if (require()->process_affinity(mask, sizeof mask, &needed) != DOTNET_PAL_OK) return false;
    for (uint32_t cpu = 0; cpu < max_cpus && cpu / 8 < needed; ++cpu)
        if (mask[cpu / 8] & (1u << (cpu % 8))) add(cpu);
    return true;
}
inline bool set_thread_affinity(uint32_t cpu) { return require()->set_thread_affinity(cpu) == DOTNET_PAL_OK; }
inline bool physical(uint64_t &total, uint64_t &available) { return require()->physical_memory(&total, &available) == DOTNET_PAL_OK; }
inline uint64_t memory_limit() { uint64_t v = 0; return require()->memory_limit(&v) == DOTNET_PAL_OK ? v : 0; }
inline uint64_t virtual_limit() { uint64_t v = 0; return require()->virtual_limit(&v) == DOTNET_PAL_OK ? v : 0; }
inline size_t cache_size() { size_t v = 0; return require()->cache_size(&v) == DOTNET_PAL_OK ? v : 0; }
inline bool features(uint64_t &first, uint64_t &second) { return require()->cpu_features(&first, &second) == DOTNET_PAL_OK; }
// Added after the group's first shape: a table built before them leaves them NULL, and a port that cannot
// answer reports UNSUPPORTED. Both read as "nothing known", which is what the runtime's own fallbacks expect.
inline size_t cache_level(uint32_t level) {
    const auto *t = require(); size_t v = 0;
    return t->cache_level_size && t->cache_level_size(level, &v) == DOTNET_PAL_OK ? v : 0;
}
inline uint64_t available_swap() {
    const auto *t = require(); uint64_t total = 0, available = 0;
    return t->swap_memory && t->swap_memory(&total, &available) == DOTNET_PAL_OK ? available : 0;
}
}
#endif
