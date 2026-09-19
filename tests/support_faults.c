#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define dotnet_pal_host_support_v2 reference_support_v2
#include "../crates/dotnet-pal-posix/native/support_posix.c"
#undef dotnet_pal_host_support_v2
static int mode;
static dotnet_pal_host_support table;
static uint32_t bad_allocate(size_t n,uint32_t z,void **out){(void)n;(void)z;*out=mode==7?NULL:(void*)1;return mode==7?0:DOTNET_PAL_OUT_OF_MEMORY;}
static uint32_t bad_resize(void *p,size_t n,void **out){(void)p;(void)n;*out=(void*)1;return DOTNET_PAL_OUT_OF_MEMORY;}
static uint32_t bad_create(void **out){*out=(void*)1;return 99;}
static uint32_t bad_write(const uint8_t *p,size_t n,size_t *out){(void)p;(void)n;*out=1;return mode==8?0:DOTNET_PAL_OS_ERROR;}
const dotnet_pal_host_support *dotnet_pal_host_support_v2(void){return mode==12?(const dotnet_pal_host_support*)(uintptr_t)3:&table;}
int main(int argc,char **argv){
    assert(argc==2);mode=atoi(argv[1]);table=*reference_support_v2();
    switch(mode){
        case 1:table.header.abi_version=99;break;
        case 2:table.header.struct_size=sizeof(dotnet_pal_header);break;
        case 3:table.header.capabilities=0;break;
        case 4:table.ops.allocate=NULL;break;
        case 5:case 7:table.ops.allocate=bad_allocate;break;
        case 6:table.ops.resize=bad_resize;break;
        case 8:case 9:table.ops.write_stderr=bad_write;break;
        case 10:table.ops.rw_create=bad_create;break;
        case 11:table.ops.thread_name=NULL;break;
        case 12:break;
        default:abort();
    }
    const dotnet_pal_api *a=dotnet_pal_get_api(2);
    if(mode<=4 || mode>=11){assert(!a);puts("SUPPORT HOST TABLE REJECTED");return 0;}
    assert(a);void *p=(void*)1;size_t done=99;
    if(mode==5 || mode==7){assert(a->support.allocate(10,0,&p)==(mode==7?DOTNET_PAL_OS_ERROR:DOTNET_PAL_OUT_OF_MEMORY));assert(!p);}
    if(mode==6){void *old=NULL;assert(a->support.allocate(19,0,&old)==0);memset(old,0x72,19);
        assert(a->support.resize(old,100,&p)==DOTNET_PAL_OUT_OF_MEMORY && !p);
        for(int i=0;i<19;i++){assert(((unsigned char*)old)[i]==0x72);}assert(a->support.release(old)==0);}
    if(mode==8 || mode==9){assert(a->support.write_stderr((const uint8_t*)"abcd",4,&done)==DOTNET_PAL_OS_ERROR);assert(done==(mode==8?0u:1u));}
    if(mode==10){assert(a->support.rw_create(&p)==DOTNET_PAL_OS_ERROR && !p);}
    puts("SUPPORT HOST FAILURE CONTRACT PASS");return 0;
}
