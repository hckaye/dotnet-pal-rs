/* Independent POSIX reference provider for the host-kernel conformance suite. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <pthread.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

typedef struct { pthread_mutex_t lock; pthread_cond_t cond; int manual, signaled; unsigned waiters; } Event;
static uint32_t result(int rc) {
    switch (rc) {
        case 0: return DOTNET_PAL_OK;
        case ENOMEM: case EAGAIN: return DOTNET_PAL_OUT_OF_MEMORY;
        case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
        case EBUSY: return DOTNET_PAL_BUSY;
        case ETIMEDOUT: return DOTNET_PAL_TIMEOUT;
        default: return DOTNET_PAL_OS_ERROR;
    }
}
static uint32_t event_create(uint32_t manual, uint32_t initial, void **out) {
    Event *e = calloc(1, sizeof *e); if (!e) return DOTNET_PAL_OUT_OF_MEMORY;
    pthread_condattr_t attr;
    int rc = pthread_condattr_init(&attr);
    if (rc) { free(e); return result(rc); }
    rc = pthread_condattr_setclock(&attr, CLOCK_MONOTONIC);
    if (!rc) rc = pthread_mutex_init(&e->lock, NULL);
    if (rc) { pthread_condattr_destroy(&attr); free(e); return result(rc); }
    rc = pthread_cond_init(&e->cond, &attr);
    pthread_condattr_destroy(&attr);
    if (rc) { pthread_mutex_destroy(&e->lock); free(e); return result(rc); }
    e->manual = (int)manual; e->signaled = (int)initial; *out = e; return DOTNET_PAL_OK;
}
static uint32_t event_destroy(void *h) {
    Event *e = h; int rc = pthread_mutex_lock(&e->lock); if (rc) return result(rc);
    if (e->waiters) { pthread_mutex_unlock(&e->lock); return DOTNET_PAL_BUSY; }
    rc = pthread_cond_destroy(&e->cond);
    int unlock = pthread_mutex_unlock(&e->lock); if (rc || unlock) return result(rc ? rc : unlock);
    rc = pthread_mutex_destroy(&e->lock); if (!rc) free(e); return result(rc);
}
static uint32_t event_set(void *h) {
    Event *e = h; int rc = pthread_mutex_lock(&e->lock); if (rc) return result(rc);
    e->signaled = 1;
    rc = e->manual ? pthread_cond_broadcast(&e->cond) : pthread_cond_signal(&e->cond);
    int unlock = pthread_mutex_unlock(&e->lock); return result(rc ? rc : unlock);
}
static uint32_t event_reset(void *h) {
    Event *e = h; int rc = pthread_mutex_lock(&e->lock); if (rc) return result(rc);
    e->signaled = 0; return result(pthread_mutex_unlock(&e->lock));
}
static uint32_t event_wait(void *h, uint64_t ns) {
    Event *e = h; struct timespec end = {0, 0};
    if (ns != 0 && ns != UINT64_MAX) {
        if (clock_gettime(CLOCK_MONOTONIC, &end)) return DOTNET_PAL_OS_ERROR;
        uint64_t nanos = (uint64_t)end.tv_nsec + ns % UINT64_C(1000000000);
        uint64_t seconds = (uint64_t)end.tv_sec + ns / UINT64_C(1000000000) + nanos / UINT64_C(1000000000);
        if (seconds > INT64_MAX) return DOTNET_PAL_INVALID_ARGUMENT;
        end.tv_sec = (time_t)seconds; end.tv_nsec = (long)(nanos % UINT64_C(1000000000));
    }
    int rc = pthread_mutex_lock(&e->lock); if (rc) return result(rc);
    ++e->waiters;
    while (!e->signaled) {
        if (!ns) { rc = ETIMEDOUT; break; }
        rc = ns == UINT64_MAX ? pthread_cond_wait(&e->cond, &e->lock) : pthread_cond_timedwait(&e->cond, &e->lock, &end);
        if (rc) break;
    }
    if ((!rc || rc == ETIMEDOUT) && e->signaled) { rc = 0; if (!e->manual) e->signaled = 0; }
    --e->waiters;
    int unlock = pthread_mutex_unlock(&e->lock); return result(unlock ? unlock : rc);
}
static uint32_t mutex_create(uint32_t recursive, void **out) {
    pthread_mutex_t *m = malloc(sizeof *m); if (!m) return DOTNET_PAL_OUT_OF_MEMORY;
    pthread_mutexattr_t attr; int rc = pthread_mutexattr_init(&attr);
    if (rc) { free(m); return result(rc); }
    rc = pthread_mutexattr_settype(&attr, recursive ? PTHREAD_MUTEX_RECURSIVE : PTHREAD_MUTEX_ERRORCHECK);
    if (!rc) rc = pthread_mutex_init(m, &attr);
    pthread_mutexattr_destroy(&attr);
    if (rc) { free(m); return result(rc); } *out = m; return DOTNET_PAL_OK;
}
static uint32_t mutex_destroy(void *h) { int rc = pthread_mutex_destroy(h); if (!rc) free(h); return result(rc); }
static uint32_t mutex_lock(void *h) { return result(pthread_mutex_lock(h)); }
static uint32_t mutex_unlock(void *h) { return result(pthread_mutex_unlock(h)); }
static uint32_t thread_create(dotnet_pal_thread_entry entry, void *arg, size_t size, void **out) {
    pthread_t *h = malloc(sizeof *h); if (!h) return DOTNET_PAL_OUT_OF_MEMORY;
    pthread_attr_t attr; int rc = pthread_attr_init(&attr);
    if (rc) { free(h); return result(rc); }
    if (size) rc = pthread_attr_setstacksize(&attr, size);
    if (!rc) rc = pthread_create(h, &attr, entry, arg);
    pthread_attr_destroy(&attr);
    if (rc) { free(h); return result(rc); } *out = h; return DOTNET_PAL_OK;
}
static uint32_t thread_join(void *h) { int rc = pthread_join(*(pthread_t *)h, NULL); if (!rc) free(h); return result(rc); }
static uint32_t thread_detach(void *h) { int rc = pthread_detach(*(pthread_t *)h); if (!rc) free(h); return result(rc); }
static uint32_t tls_create(dotnet_pal_tls_destructor dtor, void **out) {
    pthread_key_t *h = malloc(sizeof *h); if (!h) return DOTNET_PAL_OUT_OF_MEMORY;
    int rc = pthread_key_create(h, dtor); if (rc) { free(h); return result(rc); } *out = h; return DOTNET_PAL_OK;
}
static uint32_t tls_destroy(void *h) { int rc = pthread_key_delete(*(pthread_key_t *)h); if (!rc) free(h); return result(rc); }
static uint32_t tls_get(void *h, void **out) { *out = pthread_getspecific(*(pthread_key_t *)h); return DOTNET_PAL_OK; }
static uint32_t tls_set(void *h, void *value) { return result(pthread_setspecific(*(pthread_key_t *)h, value)); }
static uint32_t stack_bounds(void **low, void **high) {
    pthread_attr_t attr; int rc = pthread_getattr_np(pthread_self(), &attr); if (rc) return result(rc);
    size_t size = 0; void *base = NULL; rc = pthread_attr_getstack(&attr, &base, &size); pthread_attr_destroy(&attr);
    if (rc) return result(rc);
    *low = base; *high = (char *)base + size; return DOTNET_PAL_OK;
}
static uint32_t process_barrier(void) {
    if (syscall(SYS_membarrier, 16, 0) != 0) return DOTNET_PAL_UNSUPPORTED;
    return syscall(SYS_membarrier, 8, 0) == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static const dotnet_pal_host_kernel HOST = {
    { DOTNET_PAL_ABI_VERSION, sizeof HOST, DOTNET_PAL_CAP_KERNEL },
    { event_create, event_destroy, event_set, event_reset, event_wait,
      mutex_create, mutex_destroy, mutex_lock, mutex_unlock,
      thread_create, thread_join, thread_detach, tls_create, tls_destroy, tls_get, tls_set,
      stack_bounds, process_barrier, NULL }
};
const dotnet_pal_host_kernel *dotnet_pal_host_kernel_v2(void) { return &HOST; }
