/* Test-only transient commit fault provider. Immutable table, explicit control.
 * Failure happens BEFORE mprotect. Replaces, not supplements, host_backend.c.
 */
#define dotnet_pal_host_v2 original_host_vm
#include "host_backend.c"
#undef dotnet_pal_host_v2
#include <stdatomic.h>
static _Atomic unsigned failures_left;
static _Atomic unsigned failures_hit;
uint32_t pal_qualification_fault_control(uint32_t count) {
    if (count > 1024) return DOTNET_PAL_INVALID_ARGUMENT;
    atomic_store_explicit(&failures_left, count, memory_order_release);
    return DOTNET_PAL_OK;
}
uint64_t pal_qualification_fault_hits(void) { return atomic_load(&failures_hit); }
static uint32_t failing_commit(void *p, size_t n) {
    unsigned left = atomic_load_explicit(&failures_left, memory_order_acquire);
    while (left) {
        if (atomic_compare_exchange_weak(&failures_left, &left, left - 1)) {
            atomic_fetch_add(&failures_hit, 1);
            return DOTNET_PAL_OUT_OF_MEMORY;
        }
    }
    return commit(p, n);
}
static const dotnet_pal_host_api FAULT_HOST = {
    { DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_api), DOTNET_PAL_CAP_VM },
    { page_size, reserve, failing_commit, decommit, release, reset }
};
const dotnet_pal_host_api *dotnet_pal_host_v2(void) { return &FAULT_HOST; }
