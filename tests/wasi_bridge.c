#include "dotnet_pal.h"
#include "wasi_schema.h"
#define CHECK(x) do { if (!(x)) return __LINE__; } while(0)
extern uint32_t __imported_wasi_snapshot_preview1_clock_time_get(uint32_t,uint64_t,uint32_t);
extern uint32_t __imported_wasi_snapshot_preview1_random_get(uint32_t,uint32_t);
extern void __imported_wasi_snapshot_preview1_proc_exit(uint32_t);
int pal_bridge_test(int mode) {
    const dotnet_pal_api *a=dotnet_pal_get_api(2);
    CHECK(a && a->header.struct_size>=DOTNET_PAL_WASI_API_SIZE && (a->header.capabilities&DOTNET_PAL_CAP_WASI_DISPATCH));
    if(mode==2) { __imported_wasi_snapshot_preview1_proc_exit(0); return 999; }
    uint64_t time=0;uint8_t bytes[16]={0};
    if(mode==1) {
        CHECK(__imported_wasi_snapshot_preview1_random_get((uint32_t)(uintptr_t)bytes,16)==29);return 0;
    }
    CHECK(__imported_wasi_snapshot_preview1_clock_time_get(1,1,(uint32_t)(uintptr_t)&time)==0 && time>0);
    CHECK(__imported_wasi_snapshot_preview1_random_get((uint32_t)(uintptr_t)bytes,16)==0);
    CHECK(a->wasi.call_count(DOTNET_PAL_WASI_CLOCK_TIME_GET)==1 && a->wasi.call_count(DOTNET_PAL_WASI_RANDOM_GET)==1);
    CHECK(a->wasi.invoke(999,NULL,0)==28);
    const uint64_t bad[]={UINT64_C(1)<<32,16};
    CHECK(a->wasi.invoke(DOTNET_PAL_WASI_RANDOM_GET,bad,2)==28);
    dotnet_pal_wasi_stats stats={0};CHECK(a->wasi.read_stats(&stats,sizeof stats)==0);
    CHECK(stats.calls==2 && stats.rejected==2 && stats.host_errors==0);
    return 0;
}
