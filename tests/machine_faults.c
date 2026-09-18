#include "dotnet_pal.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
static int mode, called;
static uint32_t query(uint32_t kind,uint64_t *out) {
    (void)kind; ++called; *out=mode==5 ? 0 : UINT64_MAX;
    return mode==6 ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_OK;
}
static uint32_t affinity(uint32_t *out,size_t capacity,size_t *count) {
    ++called;
    if(capacity>=2) {out[0]=2;out[1]=2;}
    *count=mode==11 ? DOTNET_PAL_MACHINE_MAX_CPUS+1u : 2;
    return mode==8 ? DOTNET_PAL_OS_ERROR : mode==11 ? DOTNET_PAL_BUFFER_TOO_SMALL : DOTNET_PAL_OK;
}
static uint32_t bind_cpu(uint32_t cpu) {(void)cpu; ++called;return DOTNET_PAL_TIMEOUT;}
static uint32_t current(uint32_t *out) {++called;*out=UINT32_MAX;return DOTNET_PAL_OK;}
const dotnet_pal_host_machine *dotnet_pal_host_machine_v2(void) {
    static dotnet_pal_host_machine h;
    h.header=(dotnet_pal_header){2,sizeof(h),DOTNET_PAL_CAP_MACHINE};
    h.ops=(dotnet_pal_machine_ops){query,affinity,bind_cpu,current,NULL};
    if(mode==1)h.header.abi_version=99;
    if(mode==2)h.header.struct_size=sizeof(dotnet_pal_header);
    if(mode==3)h.header.capabilities=0;
    if(mode==4)h.ops.current_cpu=NULL;
    return &h; /* called once at negotiation; no mutation after publication */
}
int main(int argc,char **argv) {
    assert(argc==2);mode=atoi(argv[1]);assert(mode>=1 && mode<=13);
    const dotnet_pal_api *a=dotnet_pal_get_api(2);
    if(mode<=4) {assert(!a && called==0);puts("MACHINE malformed table rejected");return 0;}
    assert(a);
    uint64_t value=55; uint32_t data[2]={99,99};size_t count=88;
    if(mode==5 || mode==6) {assert(a->machine.query(1,&value)==DOTNET_PAL_OS_ERROR);assert(value==0);}
    if(mode==7 || mode==8 || mode==11) {
        assert(a->machine.process_affinity(data,2,&count)==DOTNET_PAL_OS_ERROR);
        assert(count==0 && data[0]==0 && data[1]==0);
    }
    if(mode==9) {assert(a->machine.current_cpu(data)==DOTNET_PAL_OS_ERROR);assert(data[0]==UINT32_MAX);}
    if(mode==10)assert(a->machine.bind_current(0)==DOTNET_PAL_OS_ERROR);
    if(mode==12) {
        size_t overlap[2]={99,99};
        assert(a->machine.process_affinity((uint32_t*)overlap,2,overlap)==DOTNET_PAL_INVALID_ARGUMENT);
        assert(overlap[0]==99 && overlap[1]==99 && called==0);
    }
    if(mode==13) {assert(a->machine.query(99,&value)==DOTNET_PAL_UNSUPPORTED);assert(value==0 && called==0);}
    printf("MACHINE host fault case=%d PASS\n",mode);
}
