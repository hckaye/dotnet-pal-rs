/* Reference Linux host provider for the neutral machine ABI. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <sched.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/sysinfo.h>
#include <unistd.h>
#define CPU_WORDS (DOTNET_PAL_MACHINE_MAX_CPUS / (sizeof(unsigned long) * CHAR_BIT))
static uint32_t number(int selector, uint64_t *out) {
    long v = sysconf(selector);
    if (v < 0) return DOTNET_PAL_UNSUPPORTED;
    *out = (uint64_t)v; return DOTNET_PAL_OK;
}
static uint32_t possible(uint64_t *out) {
    int fd = open("/sys/devices/system/cpu/possible", O_RDONLY | O_CLOEXEC);
    if (fd < 0) return DOTNET_PAL_UNSUPPORTED;
    char text[8193]; size_t used = 0; uint32_t result = DOTNET_PAL_OS_ERROR;
    for (;;) {
        if (used == sizeof(text) - 1) { result = DOTNET_PAL_UNSUPPORTED; break; }
        ssize_t n = read(fd, text + used, sizeof(text) - 1 - used);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) break;
        if (n == 0) { result = DOTNET_PAL_OK; break; }
        used += (size_t)n;
    }
    close(fd);
    if (result != DOTNET_PAL_OK || !used) return DOTNET_PAL_OS_ERROR;
    text[used] = 0;
    char *p = text; unsigned long previous = 0; int first = 1;
    for (;;) {
        if (*p < '0' || *p > '9') return DOTNET_PAL_OS_ERROR;
        errno = 0; char *end; unsigned long low = strtoul(p, &end, 10), high = low;
        if (errno || end == p) return DOTNET_PAL_OS_ERROR;
        if (*end == '-') {
            p = end + 1; if (*p < '0' || *p > '9') return DOTNET_PAL_OS_ERROR;
            high = strtoul(p, &end, 10);
            if (errno || end == p) return DOTNET_PAL_OS_ERROR;
        }
        if (high < low || high >= DOTNET_PAL_MACHINE_MAX_CPUS || (!first && low <= previous)) return DOTNET_PAL_OS_ERROR;
        previous = high; first = 0;
        if (*end == ',') { p = end + 1; continue; }
        while (*end == '\n' || *end == '\r' || *end == ' ' || *end == '\t') ++end;
        if (*end) return DOTNET_PAL_OS_ERROR;
        *out = previous + 1; return DOTNET_PAL_OK;
    }
}
static uint32_t query(uint32_t kind, uint64_t *out) {
    *out = 0;
    switch (kind) {
    case DOTNET_PAL_MACHINE_ONLINE_CPUS: return number(_SC_NPROCESSORS_ONLN, out);
    case DOTNET_PAL_MACHINE_POSSIBLE_CPUS: return possible(out);
    case DOTNET_PAL_MACHINE_PAGE_BYTES: return number(_SC_PAGESIZE, out);
    case DOTNET_PAL_MACHINE_CACHE_L1: return number(_SC_LEVEL1_DCACHE_SIZE, out);
    case DOTNET_PAL_MACHINE_CACHE_L2: return number(_SC_LEVEL2_CACHE_SIZE, out);
    case DOTNET_PAL_MACHINE_CACHE_L3: return number(_SC_LEVEL3_CACHE_SIZE, out);
    case DOTNET_PAL_MACHINE_CACHE_L4: return number(_SC_LEVEL4_CACHE_SIZE, out);
    case DOTNET_PAL_MACHINE_PHYSICAL_BYTES:
    case DOTNET_PAL_MACHINE_AVAILABLE_BYTES: {
        uint64_t pages, page;
        uint32_t s = number(kind == DOTNET_PAL_MACHINE_PHYSICAL_BYTES ? _SC_PHYS_PAGES : _SC_AVPHYS_PAGES, &pages);
        if (s) return s;
        s = number(_SC_PAGESIZE, &page); if (s) return s;
        if (!page || pages > UINT64_MAX / page) return DOTNET_PAL_OS_ERROR;
        *out = pages * page; return DOTNET_PAL_OK;
    }
    case DOTNET_PAL_MACHINE_ADDRESS_LIMIT: {
        struct rlimit r; if (getrlimit(RLIMIT_AS, &r)) return DOTNET_PAL_OS_ERROR;
        *out = r.rlim_cur == RLIM_INFINITY ? UINT64_MAX : (uint64_t)r.rlim_cur;
        return DOTNET_PAL_OK;
    }
    case DOTNET_PAL_MACHINE_SWAP_BYTES: {
        struct sysinfo s; if (sysinfo(&s) || !s.mem_unit) return DOTNET_PAL_OS_ERROR;
        if ((uint64_t)s.freeswap > UINT64_MAX / s.mem_unit) return DOTNET_PAL_OS_ERROR;
        *out = (uint64_t)s.freeswap * s.mem_unit; return DOTNET_PAL_OK;
    }
    default: return DOTNET_PAL_UNSUPPORTED;
    }
}
static uint32_t affinity(uint32_t *out, size_t capacity, size_t *count) {
    unsigned long bits[CPU_WORDS] = {0}; *count = 0;
    if (sched_getaffinity(getpid(), sizeof(bits), (cpu_set_t *)bits)) return DOTNET_PAL_OS_ERROR;
    for (size_t i = 0; i < DOTNET_PAL_MACHINE_MAX_CPUS; ++i)
        if (bits[i / (sizeof(long)*CHAR_BIT)] & (1ul << (i % (sizeof(long)*CHAR_BIT)))) ++*count;
    if (!*count) return DOTNET_PAL_OS_ERROR;
    if (*count > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    size_t n = 0;
    for (size_t i = 0; i < DOTNET_PAL_MACHINE_MAX_CPUS; ++i)
        if (bits[i / (sizeof(long)*CHAR_BIT)] & (1ul << (i % (sizeof(long)*CHAR_BIT)))) out[n++] = (uint32_t)i;
    return DOTNET_PAL_OK;
}
static uint32_t bind_cpu(uint32_t cpu) {
    if (cpu >= DOTNET_PAL_MACHINE_MAX_CPUS) return DOTNET_PAL_INVALID_ARGUMENT;
    unsigned long bits[CPU_WORDS] = {0};
    bits[cpu / (sizeof(long)*CHAR_BIT)] = 1ul << (cpu % (sizeof(long)*CHAR_BIT));
    return sched_setaffinity(0, sizeof(bits), (cpu_set_t *)bits) ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_OK;
}
static uint32_t current(uint32_t *out) {
    int cpu = sched_getcpu(); *out = UINT32_MAX;
    if (cpu < 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint32_t)cpu; return DOTNET_PAL_OK;
}
const dotnet_pal_host_machine *dotnet_pal_host_machine_v2(void) {
    static const dotnet_pal_host_machine api = {
        {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_machine), DOTNET_PAL_CAP_MACHINE},
        {query, affinity, bind_cpu, current, NULL}
    };
    return &api;
}
