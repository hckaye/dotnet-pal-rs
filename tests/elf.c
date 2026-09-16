#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <elf.h>
int pal_elf_fault;
int dotnet_pal_elf_test_anchor=17;
static const dotnet_pal_elf_ops *ops;
struct Visit {unsigned calls;int found;int stop;};
static int32_t visit(const dotnet_pal_elf_image *image,void *data){
    struct Visit *v=data; ++v->calls;
    assert(image && image->name && !(image->flags & ~DOTNET_PAL_ELF_LOAD_COUNTERS));
    for(uint32_t i=0;i<image->header_count;++i){
        dotnet_pal_elf64_header entry;
        memcpy(&entry,(const unsigned char*)image->headers+i*sizeof entry,sizeof entry);
        const dotnet_pal_elf64_header *h=&entry;
        if(h->type==PT_LOAD){
            uintptr_t start=image->load_bias+(uintptr_t)h->virtual_address;
            uintptr_t address=(uintptr_t)&dotnet_pal_elf_test_anchor;
            if(address>=start && address-start<h->memory_size)v->found=1;
        }
    }
    return v->stop;
}
static void *worker(void *unused){
    (void)unused;
    for(unsigned i=0;i<50;++i){
        struct Visit v={0};int32_t stop=111;
        assert(ops->enumerate(visit,&v,&stop)==0 && stop==0 && v.calls>0 && v.found);
        dotnet_pal_elf_symbol symbol={0};
        assert(ops->lookup(&dotnet_pal_elf_test_anchor,&symbol)==0);
        assert(symbol.module_base && symbol.module_name && symbol.symbol_address==&dotnet_pal_elf_test_anchor);
        assert(!strcmp(symbol.symbol_name,"dotnet_pal_elf_test_anchor"));
    }
    return NULL;
}
int main(int argc,char **argv){
    pal_elf_fault=argc==2?atoi(argv[1]):0;
    const dotnet_pal_api *api=dotnet_pal_get_api(2);
    if(pal_elf_fault>=1 && pal_elf_fault<=4){assert(!api);puts("ELF MALFORMED TABLE REJECTED");return 0;}
    assert(api && api->header.struct_size>=DOTNET_PAL_ELF_API_SIZE && (api->header.capabilities & DOTNET_PAL_CAP_ELF64_METADATA));
    ops=&api->elf;
    if(pal_elf_fault){
        if(pal_elf_fault==5 || pal_elf_fault==10 || pal_elf_fault==11){
            dotnet_pal_elf_symbol value;memset(&value,0xa5,sizeof value);
            assert(ops->lookup(&dotnet_pal_elf_test_anchor,&value)==DOTNET_PAL_OS_ERROR);
            assert(!value.module_base && !value.module_name && !value.symbol_address && !value.symbol_name);
        }else{
            struct Visit v={0,0,-37};int32_t result=111;
            assert(ops->enumerate(visit,&v,&result)==DOTNET_PAL_OS_ERROR && result==0);
            assert(v.calls==((pal_elf_fault==7 || pal_elf_fault==8)?1u:0u));
        }
        puts("ELF PROVIDER FAILURE REJECTED AND OUTPUT CLEARED");return 0;
    }
    int32_t result=111; struct Visit v={0,0,-37};
    assert(ops->enumerate(NULL,NULL,&result)==DOTNET_PAL_INVALID_ARGUMENT && result==0);
    assert(ops->enumerate(visit,&v,NULL)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(ops->enumerate(visit,&v,&result)==0 && result==-37 && v.calls==1);
    dotnet_pal_elf_symbol symbol;
    assert(ops->lookup(NULL,&symbol)==DOTNET_PAL_INVALID_ARGUMENT && !symbol.module_base);
    assert(ops->lookup(&dotnet_pal_elf_test_anchor,NULL)==DOTNET_PAL_INVALID_ARGUMENT);
    pthread_t workers[4];
    for(int i=0;i<4;++i)assert(pthread_create(&workers[i],NULL,worker,NULL)==0);
    for(int i=0;i<4;++i)assert(pthread_join(workers[i],NULL)==0);
    dotnet_pal_elf_stats stats;
    assert(ops->read_stats(&stats,sizeof stats-1)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(ops->read_stats(&stats,sizeof stats)==0);
    assert(stats.enumerate_ok==201 && stats.lookup_ok==200);
    puts("ELF METADATA PASS headers, containing image, symbol, early-stop and concurrent discovery");
}
