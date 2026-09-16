#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <string.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
int pal_runtime_fault;
static uint32_t fault(void) { return pal_runtime_fault == 8 ? UINT32_MAX : DOTNET_PAL_OS_ERROR; }
static void text(char *out, const uint8_t *name, size_t len) {
    if (len) memcpy(out, name, len);
    out[len] = 0;
}
static uint32_t env_get(const uint8_t *name, size_t n, uint8_t *out, size_t cap, size_t *needed) {
    if (pal_runtime_fault >= 7) {
        *needed = 1; if (cap) out[0] = 42;
        return pal_runtime_fault == 9 ? DOTNET_PAL_OK : fault();
    }
    char key[DOTNET_PAL_MAX_NAME + 1]; text(key, name, n);
    const char *value = getenv(key);
    if (!value) return DOTNET_PAL_NOT_FOUND;
    *needed = strlen(value) + 1;
    if (cap < *needed) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, value, *needed); return DOTNET_PAL_OK;
}
static uint32_t pid_get(uint64_t *out) {
    if (pal_runtime_fault >= 7) { *out = pal_runtime_fault == 9 ? 0 : 42; return pal_runtime_fault == 9 ? DOTNET_PAL_OK : fault(); }
    *out = (uint64_t)getpid(); return DOTNET_PAL_OK;
}
static uint32_t tid_get(uint64_t *out) {
    if (pal_runtime_fault >= 7) return pid_get(out);
    long id = syscall(SYS_gettid); if (id <= 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint64_t)id; return DOTNET_PAL_OK;
}
static uint32_t realtime(uint64_t *out) {
    if (pal_runtime_fault >= 7) { *out = 42; return fault(); }
    struct timespec ts;
    if (clock_gettime(CLOCK_REALTIME, &ts) != 0 || ts.tv_sec < 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
    return DOTNET_PAL_OK;
}
static uint32_t random_bytes(uint8_t *out, size_t size) {
    if (pal_runtime_fault >= 7) { memset(out, 42, size); return fault(); }
    size_t done = 0;
    while (done < size) {
        ssize_t n = getrandom(out + done, size - done, 0);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return DOTNET_PAL_OS_ERROR;
        done += (size_t)n;
    }
    return DOTNET_PAL_OK;
}
static int protect_bits(uint32_t p) {
    return ((p & DOTNET_PAL_READ) ? PROT_READ : 0) |
           ((p & DOTNET_PAL_WRITE) ? PROT_WRITE : 0) |
           ((p & DOTNET_PAL_EXECUTE) ? PROT_EXEC : 0);
}
static uint32_t map_new(size_t size, uint32_t p, void **out) {
    if (pal_runtime_fault >= 7) { *out = (void*)4097; return pal_runtime_fault == 9 ? DOTNET_PAL_OK : fault(); }
    void *value = mmap(NULL, size, protect_bits(p), MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (value == MAP_FAILED) return errno == ENOMEM ? DOTNET_PAL_OUT_OF_MEMORY : DOTNET_PAL_OS_ERROR;
    *out = value; return DOTNET_PAL_OK;
}
static uint32_t map_free(void *address, size_t size) {
    if (pal_runtime_fault >= 7) return fault();
    return munmap(address, size) == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static uint32_t map_protect(void *address, size_t size, uint32_t p) {
    if (pal_runtime_fault >= 7) return fault();
    return mprotect(address, size, protect_bits(p)) == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static uint32_t mod_open(const uint8_t *name, size_t n, void **out) {
    if (pal_runtime_fault >= 7) { *out = NULL; return pal_runtime_fault == 9 ? DOTNET_PAL_OK : fault(); }
    char buffer[DOTNET_PAL_MAX_NAME + 1]; text(buffer, name, n);
    void *value = dlopen(name ? buffer : NULL, RTLD_LAZY | RTLD_LOCAL);
    if (!value) return DOTNET_PAL_NOT_FOUND;
    *out = value; return DOTNET_PAL_OK;
}
static uint32_t mod_symbol(void *h, const uint8_t *name, size_t n, void **out) {
    if (pal_runtime_fault >= 7) { *out = (void*)4096; return fault(); }
    char buffer[DOTNET_PAL_MAX_NAME + 1]; text(buffer, name, n);
    (void)dlerror(); void *value = dlsym(h, buffer);
    if (dlerror()) return DOTNET_PAL_NOT_FOUND;
    *out = value; return DOTNET_PAL_OK;
}
static uint32_t mod_close(void *h) {
    if (pal_runtime_fault >= 7) return fault();
    return dlclose(h) == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static uint32_t mod_info(void *address, dotnet_pal_module_info *out) {
    if (pal_runtime_fault >= 7) { memset(out, 0, sizeof *out); return pal_runtime_fault == 9 ? DOTNET_PAL_OK : fault(); }
    Dl_info info;
    if (!dladdr(address, &info)) return DOTNET_PAL_NOT_FOUND;
    *out = (dotnet_pal_module_info){info.dli_fbase, (const uint8_t*)info.dli_fname, strlen(info.dli_fname)};
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_runtime table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_runtime), DOTNET_PAL_CAP_RUNTIME},
    {env_get, pid_get, tid_get, realtime, random_bytes, map_new, map_free, map_protect,
     mod_open, mod_symbol, mod_close, mod_info, NULL}
};
const dotnet_pal_host_runtime *dotnet_pal_host_runtime_v2(void) {
    if (pal_runtime_fault == 1) return NULL;
    if (pal_runtime_fault >= 2 && pal_runtime_fault <= 6) {
        static dotnet_pal_host_runtime bad;
        bad = table;
        if (pal_runtime_fault == 2) bad.header.abi_version++;
        if (pal_runtime_fault == 3) bad.header.struct_size = sizeof(dotnet_pal_header);
        if (pal_runtime_fault == 4) bad.header.capabilities &= ~DOTNET_PAL_CAP_ENTROPY;
        if (pal_runtime_fault == 5) bad.ops.random_bytes = NULL;
        if (pal_runtime_fault == 6) return (const dotnet_pal_host_runtime*)((const char*)&table + 1);
        return &bad;
    }
    return &table;
}
