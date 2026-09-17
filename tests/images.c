#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dlfcn.h>
#include <elf.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
static const dotnet_pal_api *api;
static void anchor(void){}
static int32_t contains(const dotnet_pal_image_view *v,void *arg){
    unsigned *count=arg;(*count)++;assert(v->format==DOTNET_PAL_IMAGE_ELF64_LE);
    assert(v->name && v->name[v->name_length]==0);
    const Elf64_Phdr *h=(const Elf64_Phdr*)v->headers;
    uintptr_t p=(uintptr_t)&anchor;
    for(size_t i=0;i<v->header_count;i++){
        if(h[i].p_type==PT_LOAD){
            assert(v->load_bias<=UINTPTR_MAX-h[i].p_vaddr);
            uintptr_t low=v->load_bias+h[i].p_vaddr;
            assert(low<=UINTPTR_MAX-h[i].p_memsz);
            if(low<=p && p<low+h[i].p_memsz)return 37;
        }
    }return 0;
}
static int32_t stop(const dotnet_pal_image_view *v,void *arg){(void)v;(*(unsigned*)arg)++;return -19;}
static void *worker(void *arg){
    (void)arg;
    for(unsigned i=0;i<100;i++){
        unsigned count=0;int32_t result=0;
        assert(api->images.iterate(contains,&count,&result)==0 && result==37 && count>0);
        dotnet_pal_symbol_info info={0};assert(api->images.address_info((void*)&anchor,&info)==0 && info.base && info.name);
    }return NULL;
}
int main(void){
    api=dotnet_pal_get_api(2);assert(api && api->header.struct_size>=DOTNET_PAL_IMAGES_API_SIZE);
    assert((api->header.capabilities&DOTNET_PAL_CAP_IMAGES)==DOTNET_PAL_CAP_IMAGES);
    unsigned count=0;int32_t result=0;
    assert(api->images.iterate(stop,&count,&result)==0 && result==-19 && count==1);
    void *module=dlopen("libm.so.6",RTLD_NOW);assert(module);void *sym=dlsym(module,"cos");assert(sym);
    dotnet_pal_symbol_info info={0};assert(api->images.address_info(sym,&info)==0);
    Dl_info reference={0};assert(dladdr(sym,&reference));
    assert(info.base==reference.dli_fbase && info.symbol_address==reference.dli_saddr);
    assert(!strcmp((const char*)info.name,reference.dli_fname));
    if(reference.dli_sname)assert(!strcmp((const char*)info.symbol_name,reference.dli_sname));
    assert(!dlclose(module));
    pthread_t workers[8];for(unsigned i=0;i<8;i++)assert(!pthread_create(&workers[i],NULL,worker,NULL));
    for(unsigned i=0;i<8;i++)assert(!pthread_join(workers[i],NULL));
    assert(api->images.address_info(NULL,&info)==DOTNET_PAL_INVALID_ARGUMENT && !info.base);
    dotnet_pal_image_stats stats={0};assert(api->images.read_stats(&stats,sizeof stats)==0);
    assert(stats.iterate_ok==801 && stats.images_seen>=801 && stats.address_ok==801);
    puts("LOADED ELF IMAGES PASS actual metadata, symbol identity, early exit, eight-thread iteration");return 0;
}
