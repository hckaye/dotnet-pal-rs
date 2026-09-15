#define _POSIX_C_SOURCE 200809L
#include "dotnet_pal.h"
#include <assert.h>
#include <stdatomic.h>
#include <stdio.h>
#include <time.h>

static const dotnet_pal_kernel_ops *k;
static void *key, *lock;
static unsigned protected_count;
static _Atomic unsigned destroyed;
static _Atomic unsigned started, completed;
static uint64_t now(void) {
    struct timespec ts; assert(clock_gettime(CLOCK_MONOTONIC, &ts) == 0);
    return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
}
static void pause_ns(long ns) { struct timespec ts = {0, ns}; while (nanosleep(&ts, &ts) != 0) {} }
static void until(_Atomic unsigned *counter, unsigned n) {
    uint64_t deadline = now() + UINT64_C(10000000000);
    while (atomic_load(counter) < n) { assert(now() < deadline); pause_ns(100000); }
}
static void destructor(void *p) { assert(p != NULL); atomic_fetch_add(&destroyed, 1); }
static void check_stack(void) {
    void *lo = NULL, *hi = NULL; unsigned char here;
    assert(k->stack_bounds(&lo, &hi) == DOTNET_PAL_OK);
    assert((uintptr_t)lo <= (uintptr_t)&here && (uintptr_t)&here < (uintptr_t)hi);
}
static void *tls_worker(void *arg) {
    void *value = (void *)1;
    assert(k->tls_get(key, &value) == DOTNET_PAL_OK && value == NULL);
    assert(k->tls_set(key, arg) == DOTNET_PAL_OK);
    assert(k->tls_get(key, &value) == DOTNET_PAL_OK && value == arg);
    check_stack();
    for (int i = 0; i < 10000; ++i) {
        assert(k->mutex_lock(lock) == DOTNET_PAL_OK);
        ++protected_count;
        assert(k->mutex_unlock(lock) == DOTNET_PAL_OK);
    }
    return NULL; // pthread termination invokes the TLS destructor before join returns
}
static void *wait_worker(void *event) {
    atomic_fetch_add(&started, 1);
    assert(k->event_wait(event, UINT64_C(10000000000)) == DOTNET_PAL_OK);
    atomic_fetch_add(&completed, 1);
    return NULL;
}
static void *detached_worker(void *event) {
    assert(k->event_set(event) == DOTNET_PAL_OK);
    return NULL; // no further accesses to event, including during thread shutdown
}
int main(void) {
    const dotnet_pal_api *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    assert(a && a->header.struct_size >= DOTNET_PAL_KERNEL_API_SIZE);
    const uint64_t required = DOTNET_PAL_CAP_KERNEL & ~DOTNET_PAL_CAP_PROCESS_BARRIER;
    assert((a->header.capabilities & required) == required);
    k = &a->kernel;
    _Static_assert(sizeof(dotnet_pal_kernel_stats) == 96, "kernel statistics layout");
    check_stack();
    void *p = (void *)1;
    assert(k->event_create(2, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    assert(k->event_create(0, 2, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    assert(k->event_create(0, 0, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(k->mutex_create(2, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    assert(k->thread_create(NULL, NULL, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    assert(k->event_wait(NULL, 0) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(k->tls_get(NULL, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    p = (void *)1;
    assert(k->stack_bounds(&p, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == (void *)1);

    void *event = NULL;
    assert(k->event_create(1, 1, &event) == DOTNET_PAL_OK);
    assert(k->event_wait(event, 0) == DOTNET_PAL_OK);
    assert(k->event_wait(event, 0) == DOTNET_PAL_OK); // manual reset stays signaled
    assert(k->event_reset(event) == DOTNET_PAL_OK);
    uint64_t start = now();
    assert(k->event_wait(event, UINT64_C(20000000)) == DOTNET_PAL_TIMEOUT);
    assert(now() - start >= UINT64_C(19000000));
    void *threads[4];
    atomic_store(&started, 0); atomic_store(&completed, 0);
    for (int i = 0; i < 4; ++i) assert(k->thread_create(wait_worker, event, 0, &threads[i]) == DOTNET_PAL_OK);
    until(&started, 4);
    assert(k->event_set(event) == DOTNET_PAL_OK);
    for (int i = 0; i < 4; ++i) assert(k->thread_join(threads[i]) == DOTNET_PAL_OK);
    assert(atomic_load(&completed) == 4);
    assert(k->event_destroy(event) == DOTNET_PAL_OK);

    assert(k->event_create(0, 1, &event) == DOTNET_PAL_OK);
    assert(k->event_wait(event, 0) == DOTNET_PAL_OK);
    assert(k->event_wait(event, 0) == DOTNET_PAL_TIMEOUT);
    assert(k->event_set(event) == DOTNET_PAL_OK);
    assert(k->event_set(event) == DOTNET_PAL_OK); // not a counting semaphore
    assert(k->event_wait(event, 0) == DOTNET_PAL_OK);
    assert(k->event_wait(event, 0) == DOTNET_PAL_TIMEOUT);
    atomic_store(&started, 0); atomic_store(&completed, 0);
    for (int i = 0; i < 2; ++i) assert(k->thread_create(wait_worker, event, 0, &threads[i]) == DOTNET_PAL_OK);
    until(&started, 2);
    assert(k->event_set(event) == DOTNET_PAL_OK);
    until(&completed, 1);
    assert(atomic_load(&completed) == 1);
    assert(k->event_set(event) == DOTNET_PAL_OK);
    for (int i = 0; i < 2; ++i) assert(k->thread_join(threads[i]) == DOTNET_PAL_OK);
    assert(atomic_load(&completed) == 2);
    assert(k->event_destroy(event) == DOTNET_PAL_OK);

    assert(k->mutex_create(1, &lock) == DOTNET_PAL_OK);
    assert(k->mutex_lock(lock) == DOTNET_PAL_OK);
    assert(k->mutex_lock(lock) == DOTNET_PAL_OK);
    assert(k->mutex_destroy(lock) == DOTNET_PAL_BUSY);
    assert(k->mutex_unlock(lock) == DOTNET_PAL_OK);
    assert(k->mutex_unlock(lock) == DOTNET_PAL_OK);
    assert(k->tls_create(destructor, &key) == DOTNET_PAL_OK);
    unsigned arguments[4] = { 11, 22, 33, 44 };
    for (int i = 0; i < 4; ++i) assert(k->thread_create(tls_worker, &arguments[i], 256 * 1024, &threads[i]) == DOTNET_PAL_OK);
    for (int i = 0; i < 4; ++i) assert(k->thread_join(threads[i]) == DOTNET_PAL_OK);
    assert(protected_count == 40000 && atomic_load(&destroyed) == 4);
    assert(k->tls_get(key, &p) == DOTNET_PAL_OK && p == NULL);
    assert(k->tls_set(key, &arguments[0]) == DOTNET_PAL_OK);
    assert(k->tls_set(key, NULL) == DOTNET_PAL_OK);
    assert(k->tls_destroy(key) == DOTNET_PAL_OK);
    assert(atomic_load(&destroyed) == 4); // key deletion does not call destructors
    assert(k->mutex_destroy(lock) == DOTNET_PAL_OK);

    for (int i = 0; i < 100; ++i) {
        assert(k->event_create(0, 0, &event) == DOTNET_PAL_OK);
        assert(k->thread_create(detached_worker, event, 0, &p) == DOTNET_PAL_OK);
        assert(k->thread_detach(p) == DOTNET_PAL_OK);
        assert(k->event_wait(event, UINT64_C(10000000000)) == DOTNET_PAL_OK);
        assert(k->event_destroy(event) == DOTNET_PAL_OK);
    }
    if (a->header.capabilities & DOTNET_PAL_CAP_PROCESS_BARRIER) {
        assert(k->process_barrier != NULL);
        for (int i = 0; i < 100; ++i) assert(k->process_barrier() == DOTNET_PAL_OK);
    } else {
        assert(k->process_barrier == NULL); // no local-fence substitute
    }
    dotnet_pal_kernel_stats stats = {0};
    assert(k->read_stats(&stats, sizeof(stats)) == DOTNET_PAL_OK);
    assert(stats.event_create_ok >= 102 && stats.event_wait_ok >= 110);
    assert(stats.mutex_lock_ok >= 40002 && stats.thread_create_ok >= 110);
    assert(stats.tls_create_ok == 1 && stats.tls_set_ok >= 6 && stats.stack_bounds_ok >= 5);
    printf("KERNEL PASS events=%llu waits=%llu threads=%llu locks=%llu tls=%llu barriers=%llu\n",
        (unsigned long long)stats.event_create_ok, (unsigned long long)stats.event_wait_ok,
        (unsigned long long)stats.thread_create_ok, (unsigned long long)stats.mutex_lock_ok,
        (unsigned long long)stats.tls_set_ok, (unsigned long long)stats.barrier_ok);
    return 0;
}
