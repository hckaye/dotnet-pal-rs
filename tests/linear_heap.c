#define _POSIX_C_SOURCE 200809L
#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
/* Test provider uses real allocations and injects only fail-before-side-effect
 * errors. It deliberately dirties new storage so Rust's zeroing is observable.
 */
static _Atomic unsigned mode, calls, releases;
static _Atomic size_t live_bytes;
static void *overlap;
static const dotnet_pal_linear_ops *l;
uint32_t dotnet_pal_storage_allocate_v2(size_t size, size_t alignment, void **out) {
    atomic_fetch_add(&calls, 1);
    switch (atomic_load(&mode)) {
    case 1: *out = (void*)1; return DOTNET_PAL_OUT_OF_MEMORY;
    case 2: *out = (void*)1; return 999;
    case 3: *out = overlap; return DOTNET_PAL_OK;
    case 4: *out = (void*)1; return DOTNET_PAL_OK;
    }
    int error = posix_memalign(out, alignment, size);
    if (error) { *out = NULL; return DOTNET_PAL_OUT_OF_MEMORY; }
    memset(*out, 0xa5, size);
    atomic_fetch_add(&live_bytes, size);
    return DOTNET_PAL_OK;
}
uint32_t dotnet_pal_storage_release_v2(void *address, size_t size) {
    atomic_fetch_add(&releases, 1);
    if (atomic_load(&mode) == 5) return DOTNET_PAL_OS_ERROR;
    free(address);
    atomic_fetch_sub(&live_bytes, size);
    return DOTNET_PAL_OK;
}
static void *worker(void *arg) {
    size_t seed = (size_t)arg + 1, g = l->granularity();
    for (unsigned i = 0; i < 300; ++i) {
        size_t n = ((seed + i) % 7 + 1) * g;
        void *p = NULL;
        assert(l->allocate(n - 1, 16*g, 0, &p) == 0 && (uintptr_t)p % (16*g) == 0);
        for (size_t j = 0; j < n; ++j) assert(((unsigned char*)p)[j] == 0);
        memset(p, (int)seed, n);
        assert(l->zero((char*)p + 1, n-2) == 0);
        assert(((unsigned char*)p)[0] == seed && ((unsigned char*)p)[n-1] == seed);
        assert(l->release(p, n-1) == 0);
    }
    return NULL;
}
int main(void) {
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
    assert(api && api->header.capabilities == (DOTNET_PAL_CAP_LINEAR | DOTNET_PAL_CAP_DYNAMIC_LINEAR));
    assert(!api->vm.reserve && !api->vm.commit);
    l = &api->linear;
    assert(atomic_load(&calls) == 0 && atomic_load(&live_bytes) == 0);
    size_t g = l->granularity(); void *p = NULL, *q = NULL;
    assert(l->allocate(2*g-1, 16*g, 0, &p) == 0);
    assert(atomic_load(&live_bytes) == 2*g);
    memset(p, 0x73, 2*g); overlap = p;
    for (unsigned m = 1; m <= 4; ++m) {
        atomic_store(&mode, m); q = (void*)1;
        assert(l->allocate(g, g, 0, &q) == (m == 1 ? DOTNET_PAL_OUT_OF_MEMORY : DOTNET_PAL_OS_ERROR));
        assert(!q && atomic_load(&live_bytes) == 2*g);
        assert(((unsigned char*)p)[0] == 0x73);
    }
    atomic_store(&mode, 5);
    assert(l->release(p, 2*g) == DOTNET_PAL_OS_ERROR);
    assert(atomic_load(&live_bytes) == 2*g && l->zero(p, 2*g) == 0);
    atomic_store(&mode, 0);
    unsigned before = atomic_load(&releases);
    assert(l->release((char*)p+g, g) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(l->release(p, g) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(l->zero((char*)p+2*g-1, 2) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(atomic_load(&releases) == before);
    assert(l->release(p, 2*g) == 0);
    assert(l->release(p, 2*g) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(l->zero(p, 1) == DOTNET_PAL_INVALID_ARGUMENT);
    /* The budget rejects before calling the backend, and returns after release. */
    size_t capacity = l->capacity();
    for (unsigned wave = 0; wave < 3; ++wave) {
        assert(l->allocate(capacity, g, 0, &p) == 0);
        before = atomic_load(&calls);
        assert(l->allocate(g, g, 0, &q) == DOTNET_PAL_OUT_OF_MEMORY && !q);
        assert(atomic_load(&calls) == before);
        assert(l->release(p, capacity) == 0 && atomic_load(&live_bytes) == 0);
    }
    pthread_t threads[8];
    for (size_t i=0;i<8;++i) assert(pthread_create(&threads[i], NULL, worker, (void*)i) == 0);
    for (unsigned i=0;i<8;++i) assert(pthread_join(threads[i], NULL) == 0);
    assert(atomic_load(&live_bytes) == 0);
    dotnet_pal_linear_stats stats;
    assert(l->read_stats(&stats, sizeof stats) == 0);
    assert(stats.allocate_ok == stats.release_ok && stats.allocate_ok == 2404);
    puts("DYNAMIC LINEAR PASS demand allocation, budget, failure rollback, ownership, zeroing, eight-thread stress");
}
