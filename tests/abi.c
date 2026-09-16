#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static const dotnet_pal_api *api;
#include "fault_probe.h"
static void *worker(void *arg) {
    (void)arg;
    const size_t page = api->vm.page_size();
    for (int i = 0; i < 64; ++i) {
        void *p = NULL;
        assert(api->vm.reserve(page, 0, 0, &p) == DOTNET_PAL_OK);
        assert(api->vm.commit(p, page) == DOTNET_PAL_OK);
        memset(p, 0x5a, page);
        assert(api->vm.decommit(p, page) == DOTNET_PAL_OK);
        assert(api->vm.commit(p, page) == DOTNET_PAL_OK);
        for (size_t j = 0; j < page; ++j) assert(((unsigned char *)p)[j] == 0);
        assert(api->vm.release(p, page) == DOTNET_PAL_OK);
    }
    return NULL;
}
int main(void) {
    _Static_assert(sizeof(dotnet_pal_header) == 16, "header layout");
    _Static_assert(sizeof(dotnet_pal_stats) == 48, "stats layout");
    _Static_assert(offsetof(dotnet_pal_api, vm) == 16, "VM layout");
    assert(dotnet_pal_get_api(1) == NULL); // replaces the original experimental ABI
    assert(dotnet_pal_get_api(UINT32_MAX) == NULL);
    api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    assert(api && api->header.abi_version == DOTNET_PAL_ABI_VERSION);
    assert(api->header.struct_size == sizeof(*api));
    assert((api->header.capabilities & (DOTNET_PAL_CAP_VM | DOTNET_PAL_CAP_LINEAR)) == DOTNET_PAL_CAP_VM);
    assert(!api->linear.allocate && !api->linear.release);
    size_t page = api->vm.page_size();
    assert(page > 0 && (page & (page - 1)) == 0);
    void *p = (void *)1;
    assert(api->vm.reserve(0, 0, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT && p == NULL);
    assert(api->vm.reserve(page, 3, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->vm.reserve(SIZE_MAX, 0, 0, &p) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->vm.reserve(page, 0, UINT32_MAX, &p) == DOTNET_PAL_UNSUPPORTED);
    assert(api->vm.reserve(page, 0, 0, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->vm.commit(NULL, page) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->vm.release((void *)1, page) == DOTNET_PAL_INVALID_ARGUMENT);

    assert(api->vm.reserve(2 * page - 1, 16 * page, 0, &p) == DOTNET_PAL_OK);
    assert((uintptr_t)p % (16 * page) == 0);
    pal_test_must_fault(p, 0); // reserve means inaccessible, not merely malloc
    assert(api->vm.commit(p, 2 * page - 1) == DOTNET_PAL_OK);
    memset(p, 0xa5, 2 * page);
    assert(api->vm.commit(p, page) == DOTNET_PAL_OK);
    assert(((unsigned char *)p)[0] == 0xa5); // idempotent commit preserves contents
    assert(api->vm.commit((char *)p + 1, page) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->vm.decommit(p, page) == DOTNET_PAL_OK);
    pal_test_must_fault(p, 0);
    assert(((unsigned char *)p)[page] == 0xa5); // adjacent page must remain intact
    assert(api->vm.commit(p, page) == DOTNET_PAL_OK);
    for (size_t i = 0; i < page; ++i) assert(((unsigned char *)p)[i] == 0);
    assert(api->vm.reset((char *)p + page, page) == DOTNET_PAL_OK);
    ((volatile unsigned char *)p)[page] = 0x42; // reset does not remove accessibility
    pal_test_release_must_fault(p, 2 * page - 1, api->vm.release);
    assert(api->vm.release(p, 2 * page - 1) == DOTNET_PAL_OK);

    pthread_t threads[4];
    for (int i = 0; i < 4; ++i) assert(pthread_create(&threads[i], NULL, worker, NULL) == 0);
    for (int i = 0; i < 4; ++i) assert(pthread_join(threads[i], NULL) == 0);
    dotnet_pal_stats stats;
    assert(api->read_stats(&stats, sizeof(stats) - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->read_stats(&stats, sizeof(stats)) == DOTNET_PAL_OK);
    assert(stats.reserve_ok == 257 && stats.decommit_ok == 257 && stats.release_ok == 257);
    printf("ABI PASS reserve=%llu commit=%llu decommit=%llu release=%llu\n",
        (unsigned long long)stats.reserve_ok, (unsigned long long)stats.commit_ok,
        (unsigned long long)stats.decommit_ok, (unsigned long long)stats.release_ok);
    return 0;
}
