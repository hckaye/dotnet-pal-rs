#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_runtime_fault;
#endif
static const dotnet_pal_runtime_ops *r;
static void *worker(void *arg) {
    uint64_t tid = 0, pid = 0; uint8_t random[64], value[16]; size_t needed;
    for (int i = 0; i < 500; ++i) {
        assert(r->thread_id(&tid) == 0 && tid == (uint64_t)syscall(SYS_gettid));
        assert(r->process_id(&pid) == 0 && pid == (uint64_t)getpid());
        assert(r->random_bytes(random, sizeof random) == 0);
        assert(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, value, sizeof value, &needed) == 0);
        assert(strcmp((const char*)value, "abc") == 0);
    }
    *(uint64_t*)arg = tid; return NULL;
}
#include "fault_probe.h"
int main(int argc, char **argv) {
    (void)argc; (void)argv;
#ifdef PAL_HOST_TEST
    pal_runtime_fault = argc > 1 ? atoi(argv[1]) : 0;
#endif
    assert(setenv("PAL_RUNTIME_VALUE", "abc", 1) == 0);
    assert(setenv("PAL_RUNTIME_EMPTY", "", 1) == 0);
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_runtime_fault >= 1 && pal_runtime_fault <= 6) {
        assert(!api); puts("RUNTIME malformed host rejected"); return 0;
    }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_RUNTIME_API_SIZE);
    assert((api->header.capabilities & DOTNET_PAL_CAP_RUNTIME) == DOTNET_PAL_CAP_RUNTIME);
    r = &api->runtime;
    uint64_t value = UINT64_MAX; size_t needed = 99;
    uint8_t output[32]; memset(output, 99, sizeof output);
#ifdef PAL_HOST_TEST
    if (pal_runtime_fault >= 7) {
        assert(r->process_id(&value) == DOTNET_PAL_OS_ERROR && value == 0);
        assert(r->realtime_ns(&value) == DOTNET_PAL_OS_ERROR && value == 0);
        assert(r->random_bytes(output, sizeof output) == DOTNET_PAL_OS_ERROR);
        for (size_t i = 0; i < sizeof output; ++i) assert(output[i] == 0);
        assert(r->environment_get((const uint8_t*)"x", 1, output, sizeof output, &needed) == DOTNET_PAL_OS_ERROR);
        assert(output[0] == 0 && needed == 0);
        void *mapping = (void*)1;
        assert(r->mapping_allocate(4096, 3, &mapping) == DOTNET_PAL_OS_ERROR && !mapping);
        void *module = (void*)1;
        assert(r->module_open(NULL, 0, &module) == DOTNET_PAL_OS_ERROR && !module);
        dotnet_pal_module_info info = {(void*)1, (void*)1, 1};
        assert(r->module_info((void*)4096, &info) == DOTNET_PAL_OS_ERROR && !info.base && !info.name && info.name_length == 0);
        puts("RUNTIME host error and malformed success sanitized"); return 0;
    }
#endif
    assert(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, NULL, 0, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == 4);
    assert(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, output, 3, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && output[0] == 0 && needed == 4);
    assert(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, output, 4, &needed) == 0 && strcmp((char*)output, "abc") == 0 && needed == 4);
    assert(r->environment_get((const uint8_t*)"PAL_RUNTIME_EMPTY", 17, output, 1, &needed) == 0 && output[0] == 0 && needed == 1);
    assert(r->environment_get((const uint8_t*)"PAL_RUNTIME_MISSING", 19, output, sizeof output, &needed) == DOTNET_PAL_NOT_FOUND && needed == 0 && output[0] == 0);
    assert(r->environment_get((const uint8_t*)"a=b", 3, output, sizeof output, &needed) == DOTNET_PAL_INVALID_ARGUMENT);
    const uint8_t nul_name[] = {'a',0,'b'};
    assert(r->environment_get(nul_name, 3, output, sizeof output, &needed) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(r->environment_get((const uint8_t*)"x", DOTNET_PAL_MAX_NAME+1, output, sizeof output, &needed) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(r->process_id(&value) == 0 && value == (uint64_t)getpid());
    assert(r->thread_id(&value) == 0 && value == (uint64_t)syscall(SYS_gettid));
    assert(r->realtime_ns(&value) == 0 && llabs((long long)(value/1000000000)-(long long)time(NULL)) < 60);
    assert(r->process_id(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    _Alignas(uint64_t) uint8_t unaligned[sizeof(uint64_t)+1];
    assert(r->thread_id((uint64_t*)(unaligned+1)) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(r->random_bytes(NULL, 0) == 0);
    assert(r->random_bytes(NULL, 1) == DOTNET_PAL_INVALID_ARGUMENT);
    uint8_t first[64], second[64];
    assert(r->random_bytes(first, sizeof first) == 0 && r->random_bytes(second, sizeof second) == 0);
    assert(memcmp(first, second, sizeof first) != 0);
    size_t page = api->vm.page_size(); void *raw = NULL;
    assert(r->mapping_allocate(page*3, 3, &raw) == 0 && raw);
    uint8_t *memory = raw; memory[0]=23; memory[page]=45; memory[page*2]=67;
    assert(r->mapping_protect(memory+page, page, DOTNET_PAL_READ) == 0);
    assert(memory[page] == 45); pal_test_must_fault(memory+page, 1);
    assert(memory[0] == 23 && memory[page*2] == 67);
    assert(r->mapping_protect(memory+page, page, DOTNET_PAL_READ|DOTNET_PAL_WRITE) == 0);
    memory[page]=89; assert(r->mapping_release(raw, page*3) == 0);
    assert(r->mapping_allocate(0, 3, &raw) == DOTNET_PAL_INVALID_ARGUMENT && raw == NULL);
    assert(r->mapping_allocate(page, 128, &raw) == DOTNET_PAL_INVALID_ARGUMENT && raw == NULL);
    void *module = NULL, *symbol = NULL;
    assert(r->module_open(NULL, 0, &module) == 0 && module);
    assert(r->module_symbol(module, (const uint8_t*)"malloc", 6, &symbol) == 0 && symbol);
    dotnet_pal_module_info info = {0};
    assert(r->module_info(symbol, &info) == 0 && info.base && info.name && strlen((const char*)info.name) == info.name_length);
    const char *absent = "dotnet_pal_no_such_symbol";
    assert(r->module_symbol(module, (const uint8_t*)absent, strlen(absent), &symbol) == DOTNET_PAL_NOT_FOUND && !symbol);
    assert(r->module_close(module) == 0);
    uint64_t ids[4] = {0}; pthread_t workers[4];
    for (int i=0;i<4;++i) assert(pthread_create(&workers[i], NULL, worker, &ids[i]) == 0);
    for (int i=0;i<4;++i) assert(pthread_join(workers[i], NULL) == 0);
    for (int i=0;i<4;++i) for(int j=0;j<i;++j) assert(ids[i] != ids[j]);
    dotnet_pal_runtime_stats stats;
    assert(r->read_stats(&stats, sizeof stats-1) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(r->read_stats(&stats, sizeof stats) == 0 && stats.identity_ok >= 4002 && stats.entropy_ok >= 2003);
    puts("RUNTIME CONTRACT PASS environment/identity/realtime/entropy/mappings/modules/concurrency");
}
