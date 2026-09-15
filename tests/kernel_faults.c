#include "dotnet_pal.h"
#include <assert.h>
#include <stdlib.h>
static unsigned mode;
static uint32_t create_event(uint32_t a, uint32_t b, void **out) {
    (void)a; (void)b; *out = mode == 6 ? NULL : (void *)1;
    return mode == 6 ? DOTNET_PAL_OK : mode == 7 ? 999u : DOTNET_PAL_OS_ERROR;
}
static uint32_t create_mutex(uint32_t a, void **out) { return create_event(a, 0, out); }
static uint32_t create_thread(dotnet_pal_thread_entry e, void *a, size_t s, void **out) {
    (void)e; (void)a; (void)s; return create_event(0, 0, out);
}
static uint32_t create_tls(dotnet_pal_tls_destructor d, void **out) { (void)d; return create_event(0, 0, out); }
static uint32_t handle_op(void *h) { (void)h; return DOTNET_PAL_OS_ERROR; }
static uint32_t event_wait(void *h, uint64_t ns) { (void)h; (void)ns; return 999u; }
static uint32_t tls_get(void *h, void **out) { (void)h; *out = (void *)1; return DOTNET_PAL_OS_ERROR; }
static uint32_t tls_set(void *h, void *p) { (void)h; (void)p; return 999u; }
static uint32_t bounds(void **lo, void **hi) { *lo = (void *)20; *hi = (void *)10; return DOTNET_PAL_OK; }
static uint32_t barrier(void) { return DOTNET_PAL_OS_ERROR; }
static dotnet_pal_host_kernel table = {
    {2, sizeof table, DOTNET_PAL_CAP_KERNEL},
    {create_event, handle_op, handle_op, handle_op, event_wait,
     create_mutex, handle_op, handle_op, handle_op, create_thread, handle_op, handle_op,
     create_tls, handle_op, tls_get, tls_set, bounds, barrier, NULL}
};
const dotnet_pal_host_kernel *dotnet_pal_host_kernel_v2(void) { return mode == 1 ? NULL : &table; }
static void *entry(void *arg) { return arg; }
int main(int argc, char **argv) {
    assert(argc == 2); mode = (unsigned)strtoul(argv[1], NULL, 10); assert(mode >= 1 && mode <= 8);
    // Configuration occurs before first publication and is immutable afterwards.
    if (mode == 2) table.header.abi_version = 99;
    if (mode == 3) table.header.struct_size = sizeof(dotnet_pal_header);
    if (mode == 4) table.header.capabilities &= ~DOTNET_PAL_CAP_TLS;
    if (mode == 5) table.ops.event_wait = NULL;
    const dotnet_pal_api *a = dotnet_pal_get_api(2);
    if (mode <= 5) { assert(a == NULL); return 0; }
    assert(a); const dotnet_pal_kernel_ops *k = &a->kernel;
    void *h = (void *)1;
    assert(k->event_create(0, 0, &h) == DOTNET_PAL_OS_ERROR && h == NULL);
    h = (void *)1; assert(k->mutex_create(0, &h) == DOTNET_PAL_OS_ERROR && h == NULL);
    h = (void *)1; assert(k->thread_create(entry, NULL, 0, &h) == DOTNET_PAL_OS_ERROR && h == NULL);
    h = (void *)1; assert(k->tls_create(NULL, &h) == DOTNET_PAL_OS_ERROR && h == NULL);
    // Opaque values are never dereferenced by the dispatch layer/test callbacks.
    assert(k->tls_get((void *)1, &h) == DOTNET_PAL_OS_ERROR && h == NULL);
    assert(k->event_wait((void *)1, 0) == DOTNET_PAL_OS_ERROR);
    assert(k->tls_set((void *)1, NULL) == DOTNET_PAL_OS_ERROR);
    void *lo = (void *)1, *hi = (void *)1;
    assert(k->stack_bounds(&lo, &hi) == DOTNET_PAL_OS_ERROR && lo == NULL && hi == NULL);
    return 0;
}
