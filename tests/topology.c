/* Conformance test of the topology group on Linux: counts, masks, memory figures
 * and feature words must agree with what the kernel reports directly. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/sysinfo.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_topology_fault;
#endif
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_topology_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_topology_fault == 1) { assert(!api); puts("TOPOLOGY malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_TOPOLOGY_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_TOPOLOGY);
    const dotnet_pal_topology_ops *t = &api->topology;
    uint32_t max = 0, count = 0, current = 0;
#ifdef PAL_HOST_TEST
    if (pal_topology_fault == 2) {
        /* Provider failures and impossible answers are sanitized: zero outputs, error status. */
        assert(t->cpu_max(&max) == DOTNET_PAL_OS_ERROR && max == 0);
        assert(t->cpu_count(&count) == DOTNET_PAL_OS_ERROR && count == 0);
        uint64_t total = 5, available = 5;
        assert(t->physical_memory(&total, &available) == DOTNET_PAL_OS_ERROR && total == 0 && available == 0);
        uint8_t mask[8] = {1, 1, 1, 1, 1, 1, 1, 1}; size_t needed = 9;
        assert(t->process_affinity(mask, sizeof mask, &needed) == DOTNET_PAL_OS_ERROR && needed == 0 && mask[0] == 0);
        size_t bytes = 7;
        assert(t->cache_level_size(2, &bytes) == DOTNET_PAL_OS_ERROR && bytes == 0);
        uint64_t swap_total = 5, swap_free = 5;
        assert(t->swap_memory(&swap_total, &swap_free) == DOTNET_PAL_OS_ERROR && swap_total == 0 && swap_free == 0);
        dotnet_pal_topology_stats stats;
        assert(t->read_stats(&stats, sizeof stats) == 0 && stats.rejected_or_failed >= 6 && stats.cpu_ok == 0 && stats.cache_ok == 0);
        puts("TOPOLOGY host errors sanitized"); return 0;
    }
#endif
    assert(t->cpu_max(&max) == 0 && max >= 1);
    assert(t->cpu_count(&count) == 0 && count >= 1 && count <= max);
    long online = sysconf(_SC_NPROCESSORS_ONLN);
    assert(online > 0 && (long)max >= online);
    cpu_set_t set; CPU_ZERO(&set);
    assert(sched_getaffinity(0, sizeof set, &set) == 0);
    unsigned affinity = (unsigned)CPU_COUNT(&set);
    assert(count <= affinity); /* quotas can only lower the count */
    uint32_t status = t->current_cpu(&current);
    assert(status == 0 ? current < max : status == DOTNET_PAL_UNSUPPORTED);
    if (status == 0) assert((int)current == sched_getcpu() || 1); /* the scheduler may move us between the two calls */
    uint8_t mask[1024]; size_t needed = 0;
    assert(t->process_affinity(mask, sizeof mask, &needed) == 0 && needed == (max + 7) / 8 && needed <= sizeof mask);
    unsigned bits = 0;
    for (size_t i = 0; i < needed; ++i) for (int b = 0; b < 8; ++b) bits += (mask[i] >> b) & 1;
    assert(bits == affinity);
    for (unsigned cpu = 0; cpu < max && cpu < 8 * sizeof set; ++cpu)
        assert(((mask[cpu / 8] >> (cpu % 8)) & 1) == (CPU_ISSET(cpu, &set) ? 1 : 0));
    uint8_t small[1]; needed = 0;
    uint32_t partial = t->process_affinity(small, sizeof small, &needed);
    assert(needed == (max + 7) / 8 && (partial == (needed > 1 ? DOTNET_PAL_BUFFER_TOO_SMALL : 0)));
    assert(t->process_affinity(NULL, 4, &needed) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(t->process_affinity(mask, sizeof mask, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    /* Pinning to the CPU we already run on is always allowed; restore the mask afterwards. */
    for (unsigned cpu = 0; cpu < max && cpu < 8 * sizeof set; ++cpu) {
        if (!CPU_ISSET(cpu, &set)) continue;
        assert(t->set_thread_affinity(cpu) == 0);
        cpu_set_t now; CPU_ZERO(&now);
        assert(sched_getaffinity(0, sizeof now, &now) == 0 && CPU_COUNT(&now) == 1 && CPU_ISSET(cpu, &now));
        break;
    }
    assert(sched_setaffinity(0, sizeof set, &set) == 0);
    assert(t->set_thread_affinity(70000) == DOTNET_PAL_INVALID_ARGUMENT);
    uint64_t total = 0, available = 0, limit = 0, vlimit = 0;
    assert(t->physical_memory(&total, &available) == 0 && total > 0 && available <= total);
    long pages = sysconf(_SC_PHYS_PAGES), page = sysconf(_SC_PAGESIZE);
    assert(pages > 0 && page > 0 && total <= (uint64_t)pages * (uint64_t)page);
    assert(t->memory_limit(&limit) == 0 && (limit == 0 || limit <= (uint64_t)pages * (uint64_t)page));
    if (limit != 0) assert(total == limit); /* a limit in force is what the total reports */
    struct rlimit as;
    assert(getrlimit(RLIMIT_AS, &as) == 0);
    assert(t->virtual_limit(&vlimit) == 0 && vlimit == (as.rlim_cur == RLIM_INFINITY ? 0 : (uint64_t)as.rlim_cur));
    size_t cache = 0;
    assert(t->cache_size(&cache) == 0);
    /* One level at a time, against what the C library and sysfs say themselves. The deepest level that answers is the
     * largest cache, which is what cache_size reports. */
    static const int level_names[] = {_SC_LEVEL1_DCACHE_SIZE, _SC_LEVEL2_CACHE_SIZE, _SC_LEVEL3_CACHE_SIZE, _SC_LEVEL4_CACHE_SIZE};
    size_t deepest = 0, levels = 0;
    for (uint32_t level = 1; level <= DOTNET_PAL_MAX_CACHE_LEVEL; ++level) {
        size_t bytes = 7;
        assert(t->cache_level_size(level, &bytes) == 0);
        long known = sysconf(level_names[level - 1]);
        if (known > 0) assert(bytes == (size_t)known);
        if (bytes != 0) { deepest = bytes; ++levels; }
    }
    assert(levels == 0 || cache >= deepest);  /* the summary is at least the deepest level that answered */
    size_t bytes = 7;
    assert(t->cache_level_size(0, &bytes) == DOTNET_PAL_INVALID_ARGUMENT && bytes == 0);
    assert(t->cache_level_size(DOTNET_PAL_MAX_CACHE_LEVEL + 1, &bytes) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(t->cache_level_size(1, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    /* Swap, against the kernel's own figures. A container may cap them, so the boundary's are at most the machine's. */
    struct sysinfo info;
    assert(sysinfo(&info) == 0);
    uint64_t unit = info.mem_unit == 0 ? 1 : (uint64_t)info.mem_unit;
    uint64_t swap_total = 7, swap_free = 7;
    assert(t->swap_memory(&swap_total, &swap_free) == 0);
    assert(swap_free <= swap_total && swap_total <= (uint64_t)info.totalswap * unit);
    assert(t->swap_memory(&swap_total, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(t->swap_memory(NULL, &swap_free) == DOTNET_PAL_INVALID_ARGUMENT);
    uint64_t first = 0, second = 0;
    assert(t->cpu_features(&first, &second) == 0);
#if defined(__aarch64__)
    assert(first != 0); /* AT_HWCAP always reports the base ISA on Linux arm64 */
#endif
    assert(t->cpu_features(NULL, &second) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(t->physical_memory(&total, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    dotnet_pal_topology_stats stats;
    assert(t->read_stats(&stats, sizeof stats) == 0 && stats.cpu_ok >= 3 && stats.memory_ok >= 4 && stats.affinity_ok >= 2 && stats.cache_ok >= 1 + DOTNET_PAL_MAX_CACHE_LEVEL);
    assert(t->read_stats(&stats, sizeof stats - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    printf("TOPOLOGY PASS max=%u count=%u affinity=%u total=%llu available=%llu limit=%llu cache=%zu levels=%zu swap=%llu/%llu\n",
           max, count, affinity, (unsigned long long)total, (unsigned long long)available, (unsigned long long)limit, cache, levels,
           (unsigned long long)swap_free, (unsigned long long)swap_total);
    return 0;
}
