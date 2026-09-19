/* Reference host implementation; no managed entry or runtime-specific types. */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include "dotnet_pal.h"
#include <errno.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static uint32_t st(int rc) {
    switch(rc) { case 0:return DOTNET_PAL_OK; case ENOMEM:case EAGAIN:return DOTNET_PAL_OUT_OF_MEMORY;
        case EINVAL:return DOTNET_PAL_INVALID_ARGUMENT; case EBUSY:return DOTNET_PAL_BUSY; default:return DOTNET_PAL_OS_ERROR; }
}
static uint32_t alloc_bytes(size_t n,uint32_t zero,void **out) {
    void *p=zero?calloc(1,n):malloc(n);
    if(!p)return DOTNET_PAL_OUT_OF_MEMORY;
    *out=p;return DOTNET_PAL_OK;
}
static uint32_t resize_bytes(void *p,size_t n,void **out) {
    void *q=realloc(p,n);if(!q)return DOTNET_PAL_OUT_OF_MEMORY;*out=q;return DOTNET_PAL_OK;
}
static uint32_t release_bytes(void *p){free(p);return DOTNET_PAL_OK;}
static uint32_t rw_create(void **out){
    pthread_rwlock_t *p=malloc(sizeof *p);if(!p)return DOTNET_PAL_OUT_OF_MEMORY;
    int rc=pthread_rwlock_init(p,NULL);if(rc){free(p);return st(rc);}*out=p;return DOTNET_PAL_OK;
}
static uint32_t rw_read(void *p){return st(pthread_rwlock_rdlock(p));}
static uint32_t rw_write(void *p){return st(pthread_rwlock_wrlock(p));}
static uint32_t rw_unlock(void *p){return st(pthread_rwlock_unlock(p));}
static uint32_t rw_destroy(void *p){int rc=pthread_rwlock_destroy(p);if(!rc)free(p);return st(rc);}
static uint32_t diagnostic_write(const uint8_t *data,size_t size,size_t *written){
    size_t done=0;while(done<size){ssize_t n=write(STDERR_FILENO,data+done,size-done);
        if(n>0)done+=(size_t)n;else if(n<0 && errno==EINTR)continue;
        else{*written=done;return DOTNET_PAL_OS_ERROR;}}
    *written=done;return DOTNET_PAL_OK;
}
static uint32_t thread_name(const uint8_t *data,size_t len){
    if(len>15)return DOTNET_PAL_INVALID_ARGUMENT;
    char name[16]={0};memcpy(name,data,len);
#ifdef __APPLE__
    return st(pthread_setname_np(name));
#else
    return st(pthread_setname_np(pthread_self(),name));
#endif
}
static const dotnet_pal_host_support SUPPORT={
    {DOTNET_PAL_ABI_VERSION,sizeof(dotnet_pal_host_support),DOTNET_PAL_CAP_SUPPORT},
    {alloc_bytes,resize_bytes,release_bytes,rw_create,rw_read,rw_write,rw_unlock,rw_destroy,diagnostic_write,thread_name,NULL}
};
const dotnet_pal_host_support *dotnet_pal_host_support_v2(void){return &SUPPORT;}
