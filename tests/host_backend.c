// Linux mock of a PRIVATE SDK backend. This is not Switch code.
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>
static size_t page_size(void) { return (size_t)sysconf(_SC_PAGESIZE); }
static uint32_t result(int rc) { return rc == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR; }
static uint32_t reserve(size_t n, size_t a, uint32_t flags, void **out) {
    (void)flags;
    size_t total = n + a - page_size();
    void *p = mmap(NULL, total, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) return DOTNET_PAL_OS_ERROR;
    uintptr_t aligned = ((uintptr_t)p + a - 1) & ~(a - 1);
    size_t prefix = aligned - (uintptr_t)p, suffix = total - prefix - n;
    if (prefix && munmap(p, prefix) != 0) abort();
    if (suffix && munmap((void *)(aligned + n), suffix) != 0) abort();
    *out = (void *)aligned;
    return DOTNET_PAL_OK;
}
static uint32_t commit(void *p, size_t n) { return result(mprotect(p, n, PROT_READ | PROT_WRITE)); }
static uint32_t decommit(void *p, size_t n) {
    return mmap(p, n, PROT_NONE, MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == MAP_FAILED
        ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_OK;
}
static uint32_t release(void *p, size_t n) { return result(munmap(p, n)); }
static uint32_t reset(void *p, size_t n) { return result(madvise(p, n, MADV_DONTNEED)); }
static const dotnet_pal_host_api HOST = {
    { DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_api), DOTNET_PAL_CAP_VM },
    { page_size, reserve, commit, decommit, release, reset }
};
const dotnet_pal_host_api *dotnet_pal_host_v2(void) { return &HOST; }
_Noreturn void dotnet_pal_host_abort(void) { abort(); }
