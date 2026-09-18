#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <limits.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <unistd.h>
static const dotnet_pal_api *api;
static uint32_t selected;
static void *worker(void *arg) {
    (void)arg; uint32_t current = UINT32_MAX;
    assert(api->machine.bind_current(selected) == DOTNET_PAL_OK);
    for (int i=0;i<100;++i) {
        assert(api->machine.current_cpu(&current) == DOTNET_PAL_OK);
        assert(current == selected);
    }
    return NULL;
}
int main(void) {
    api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    assert(api && api->header.struct_size >= DOTNET_PAL_MACHINE_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_MACHINE);
    const dotnet_pal_machine_ops *m=&api->machine;
    uint64_t n=99, possible=0, physical=0, page=0, limit=0;
    assert(m->query(DOTNET_PAL_MACHINE_ONLINE_CPUS,&n)==0 && n==(uint64_t)sysconf(_SC_NPROCESSORS_ONLN));
    assert(m->query(DOTNET_PAL_MACHINE_POSSIBLE_CPUS,&possible)==0 && possible>=n);
    assert(m->query(DOTNET_PAL_MACHINE_PHYSICAL_BYTES,&physical)==0);
    assert(m->query(DOTNET_PAL_MACHINE_PAGE_BYTES,&page)==0);
    assert(page==(uint64_t)sysconf(_SC_PAGESIZE) && physical==(uint64_t)sysconf(_SC_PHYS_PAGES)*page);
    assert(m->query(DOTNET_PAL_MACHINE_ADDRESS_LIMIT,&limit)==0);
    struct rlimit r; assert(getrlimit(RLIMIT_AS,&r)==0);
    assert(limit==(r.rlim_cur==RLIM_INFINITY ? UINT64_MAX : (uint64_t)r.rlim_cur));
    assert(m->query(DOTNET_PAL_MACHINE_SWAP_BYTES,&n)==0);
    assert(m->query(UINT32_MAX,&n)==DOTNET_PAL_UNSUPPORTED && n==0);
    assert(m->query(1,NULL)==DOTNET_PAL_INVALID_ARGUMENT);
    size_t count=0;
    assert(m->process_affinity(NULL,0,&count)==DOTNET_PAL_BUFFER_TOO_SMALL && count>0);
    uint32_t *cpus=calloc(DOTNET_PAL_MACHINE_MAX_CPUS,sizeof(uint32_t)); assert(cpus);
    assert(m->process_affinity(cpus,DOTNET_PAL_MACHINE_MAX_CPUS,&count)==0);
    unsigned long mask[DOTNET_PAL_MACHINE_MAX_CPUS/(sizeof(long)*CHAR_BIT)]={0};
    assert(sched_getaffinity(getpid(),sizeof(mask),(cpu_set_t*)mask)==0);
    size_t found=0;
    for(size_t i=0;i<DOTNET_PAL_MACHINE_MAX_CPUS;++i) {
        if(mask[i/(sizeof(long)*CHAR_BIT)] & (1ul<<(i%(sizeof(long)*CHAR_BIT)))) {
            assert(found<count && cpus[found]==i && i<possible); ++found;
        }
    }
    assert(found==count); selected=cpus[0];
    pthread_t t; assert(pthread_create(&t,NULL,worker,NULL)==0); assert(pthread_join(t,NULL)==0);
    assert(m->bind_current(UINT32_MAX)==DOTNET_PAL_INVALID_ARGUMENT);
    dotnet_pal_machine_stats stats; assert(m->read_stats(&stats,sizeof(stats))==0);
    assert(stats.query_ok>=6 && stats.affinity_ok>=1 && stats.bind_ok==1 && stats.current_ok==100);
    free(cpus);
    puts("MACHINE PASS real topology/memory, sparse affinity and per-thread binding");
}
