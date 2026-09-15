/* Same C -> Rust contract suite is linked natively and into Wasm modules. */
#include "dotnet_pal.h"
#define CHECK(c) do { if (!(c)) return __LINE__; } while (0)
int pal_test(void) {
    const dotnet_pal_api *api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    CHECK(api && api->header.abi_version == 2);
    CHECK(api->header.struct_size >= DOTNET_PAL_LINEAR_API_SIZE);
    CHECK(api->header.capabilities == DOTNET_PAL_CAP_LINEAR);
    CHECK(!api->vm.reserve && !api->vm.commit && !api->vm.decommit && !api->vm.page_size);
    CHECK(dotnet_pal_get_api(99) == NULL);
    const dotnet_pal_linear_ops *l = &api->linear;
    CHECK(l->granularity && l->capacity && l->allocate && l->zero && l->release && l->read_stats);
    size_t g = l->granularity(), capacity = l->capacity();
    CHECK(g > 0 && (g & (g - 1)) == 0 && capacity > 8 * g);
    dotnet_pal_linear_stats before, after;
    CHECK(l->read_stats(&before, sizeof(before)) == DOTNET_PAL_OK);
    CHECK(l->read_stats(&after, sizeof(after) - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    void *p = (void *)1, *q = NULL;
    CHECK(l->allocate(0, 0, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    CHECK(l->allocate(SIZE_MAX, 0, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->allocate(g, 3, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->allocate(g, 0, 1, &p) == DOTNET_PAL_UNSUPPORTED);
    CHECK(l->allocate(g, 0, 0, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->allocate(g, 0, 0, (void **)(uintptr_t)1) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->zero(NULL, 1) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->release(NULL, g) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->allocate(2 * g - 1, 16 * g, 0, &p) == DOTNET_PAL_OK);
    CHECK((uintptr_t)p % (16 * g) == 0);
    unsigned char *bytes = (unsigned char *)p;
    for (size_t i = 0; i < 2 * g; ++i) { CHECK(bytes[i] == 0); bytes[i] = 0xa5; }
    CHECK(l->zero(bytes + 7, g) == DOTNET_PAL_OK);
    CHECK(bytes[6] == 0xa5 && bytes[g + 7] == 0xa5);
    for (size_t i = 7; i < g + 7; ++i) CHECK(bytes[i] == 0);
    CHECK(l->zero(bytes + 2 * g - 1, 2) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->release(bytes + g, g) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->release(p, g) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->release(p, 2 * g - 1) == DOTNET_PAL_OK);
    CHECK(l->release(p, 2 * g) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->zero(p, 1) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(l->allocate(2 * g, 16 * g, 0, &q) == DOTNET_PAL_OK && p == q);
    bytes = (unsigned char *)q;
    for (size_t i = 0; i < 2 * g; ++i) CHECK(bytes[i] == 0);
    CHECK(l->release(q, 2 * g) == DOTNET_PAL_OK);
    /* Exhaustion fails with NULL; releasing storage makes the whole arena reusable. */
    CHECK(l->allocate(capacity, 0, 0, &p) == DOTNET_PAL_OK);
    CHECK(l->allocate(g, 0, 0, &q) == DOTNET_PAL_OUT_OF_MEMORY && q == NULL);
    CHECK(l->release(p, capacity) == DOTNET_PAL_OK);
    /* Fragmentation: three allocations, free and reuse the middle, neighbors intact. */
    void *a, *b, *c;
    CHECK(l->allocate(g, 0, 0, &a) == DOTNET_PAL_OK);
    CHECK(l->allocate(g, 0, 0, &b) == DOTNET_PAL_OK);
    CHECK(l->allocate(g, 0, 0, &c) == DOTNET_PAL_OK);
    *(unsigned char *)a = 17; *(unsigned char *)c = 42;
    CHECK(l->zero(a, 2 * g) == DOTNET_PAL_INVALID_ARGUMENT); // cannot span allocations
    CHECK(l->release(b, g) == DOTNET_PAL_OK);
    CHECK(l->allocate(g, 0, 0, &q) == DOTNET_PAL_OK && q == b);
    CHECK(*(unsigned char *)a == 17 && *(unsigned char *)c == 42);
    CHECK(l->release(a, g) == DOTNET_PAL_OK);
    CHECK(l->release(q, g) == DOTNET_PAL_OK);
    CHECK(l->release(c, g) == DOTNET_PAL_OK);
    CHECK(l->read_stats(&after, sizeof(after)) == DOTNET_PAL_OK);
    CHECK(after.allocate_ok - before.allocate_ok == 7);
    CHECK(after.release_ok - before.release_ok == 7);
    CHECK(after.zero_ok - before.zero_ok == 1);
    CHECK(after.rejected_or_failed > before.rejected_or_failed);
    dotnet_pal_stats vm;
    CHECK(api->read_stats(&vm, sizeof(vm)) == DOTNET_PAL_OK);
    CHECK(vm.reserve_ok == 0 && vm.commit_ok == 0); // not disguised virtual memory
    return 0;
}
#ifndef __wasm__
#include <stdio.h>
int main(void) {
    for (int i = 0; i < 2; ++i) {
        int line = pal_test();
        if (line) { fprintf(stderr, "LINEAR FAIL line=%d\n", line); return 1; }
    }
    puts("LINEAR PASS native C ABI; zeroing, reuse, exhaustion, fragmentation, no VM");
    return 0;
}
#endif
