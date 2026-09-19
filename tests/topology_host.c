/* Independent POSIX reference provider for the host-topology conformance suite.
 * Fault 1 withholds the table; fault 2 makes every callback fail or answer
 * impossibly so the front end's sanitizing is observable. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
#if defined(__aarch64__)
#include <sys/auxv.h>
#endif
int pal_topology_fault;
static uint32_t cpu_max(uint32_t *out) {
    if (pal_topology_fault == 2) { *out = 0; return DOTNET_PAL_OK; } /* impossible answer */
    long v = sysconf(_SC_NPROCESSORS_CONF); if (v <= 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint32_t)v; return DOTNET_PAL_OK;
}
static uint32_t cpu_count(uint32_t *out) {
    if (pal_topology_fault == 2) return DOTNET_PAL_OS_ERROR;
    cpu_set_t set; CPU_ZERO(&set);
    if (sched_getaffinity(0, sizeof set, &set) != 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint32_t)CPU_COUNT(&set); return DOTNET_PAL_OK;
}
static uint32_t current_cpu(uint32_t *out) {
    int v = sched_getcpu(); if (v < 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint32_t)v; return DOTNET_PAL_OK;
}
static uint32_t process_affinity(uint8_t *mask, size_t capacity, size_t *needed) {
    if (pal_topology_fault == 2) { *needed = 3; if (capacity) mask[0] = 0xff; return DOTNET_PAL_OS_ERROR; }
    cpu_set_t set; CPU_ZERO(&set);
    if (sched_getaffinity(0, sizeof set, &set) != 0) return DOTNET_PAL_OS_ERROR;
    uint32_t max = 0; if (cpu_max(&max) != DOTNET_PAL_OK) return DOTNET_PAL_OS_ERROR;
    *needed = (max + 7) / 8;
    for (size_t i = 0; i < *needed && i < capacity; ++i) {
        uint8_t byte = 0;
        for (int b = 0; b < 8; ++b) if (CPU_ISSET(i * 8 + b, &set)) byte |= (uint8_t)(1u << b);
        mask[i] = byte;
    }
    return *needed > capacity ? DOTNET_PAL_BUFFER_TOO_SMALL : DOTNET_PAL_OK;
}
static uint32_t set_thread_affinity(uint32_t cpu) {
    cpu_set_t set; CPU_ZERO(&set);
    if (cpu >= CPU_SETSIZE) return DOTNET_PAL_INVALID_ARGUMENT;
    CPU_SET(cpu, &set);
    return sched_setaffinity(0, sizeof set, &set) == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static uint32_t physical_memory(uint64_t *total, uint64_t *available) {
    if (pal_topology_fault == 2) { *total = 1; *available = 2; return DOTNET_PAL_OK; } /* available > total */
    long pages = sysconf(_SC_PHYS_PAGES), free_pages = sysconf(_SC_AVPHYS_PAGES), page = sysconf(_SC_PAGESIZE);
    if (pages <= 0 || page <= 0) return DOTNET_PAL_OS_ERROR;
    *total = (uint64_t)pages * (uint64_t)page;
    *available = free_pages > 0 ? (uint64_t)free_pages * (uint64_t)page : 0;
    return DOTNET_PAL_OK;
}
static uint32_t memory_limit(uint64_t *limit) { *limit = 0; return DOTNET_PAL_OK; }
static uint32_t virtual_limit(uint64_t *limit) {
    struct rlimit as; if (getrlimit(RLIMIT_AS, &as) != 0) return DOTNET_PAL_OS_ERROR;
    *limit = as.rlim_cur == RLIM_INFINITY ? 0 : (uint64_t)as.rlim_cur; return DOTNET_PAL_OK;
}
static uint32_t cache_size(size_t *bytes) {
    long v = sysconf(_SC_LEVEL2_CACHE_SIZE); *bytes = v > 0 ? (size_t)v : 0; return DOTNET_PAL_OK;
}
static uint32_t cpu_features(uint64_t *first, uint64_t *second) {
#if defined(__aarch64__)
    *first = getauxval(AT_HWCAP); *second = getauxval(AT_HWCAP2);
#else
    *first = 0; *second = 0;
#endif
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_topology table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_topology), DOTNET_PAL_CAP_TOPOLOGY},
    {cpu_max, cpu_count, current_cpu, process_affinity, set_thread_affinity, physical_memory, memory_limit, virtual_limit, cache_size, cpu_features, NULL},
};
static const dotnet_pal_host_topology malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_topology), 0}, {0}};
const dotnet_pal_host_topology *dotnet_pal_host_topology_v2(void) { return pal_topology_fault == 1 ? &malformed : &table; }
