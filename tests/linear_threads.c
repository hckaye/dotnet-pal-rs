#include "dotnet_pal.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
static const dotnet_pal_linear_ops *ops;
static void *worker(void *argument) {
    unsigned char value = (unsigned char)(uintptr_t)argument;
    for (int i = 0; i < 500; ++i) {
        void *p = NULL;
        assert(ops->allocate(4096, 0, 0, &p) == DOTNET_PAL_OK);
        for (int j = 0; j < 4096; ++j) assert(((unsigned char *)p)[j] == 0);
        memset(p, value, 4096);
        for (int j = 0; j < 4096; ++j) assert(((unsigned char *)p)[j] == value);
        assert(ops->release(p, 4096) == DOTNET_PAL_OK);
    }
    return NULL;
}
int main(void) {
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
    assert(api && api->header.capabilities == DOTNET_PAL_CAP_LINEAR);
    ops = &api->linear;
    pthread_t threads[4];
    for (uintptr_t i = 0; i < 4; ++i) assert(pthread_create(&threads[i], NULL, worker, (void *)(i + 1)) == 0);
    for (int i = 0; i < 4; ++i) assert(pthread_join(threads[i], NULL) == 0);
    dotnet_pal_linear_stats stats;
    assert(ops->read_stats(&stats, sizeof(stats)) == 0);
    assert(stats.allocate_ok == 2000 && stats.release_ok == 2000 && stats.rejected_or_failed == 0);
    puts("LINEAR THREAD PASS 2000 allocations; native test, not Wasm thread support");
}
