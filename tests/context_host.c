#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <string.h>
#include <unistd.h>
int pal_context_fault;
static _Atomic uintptr_t callbacks[5], data_slots[5];
static _Atomic int installed[5];
static int number(uint32_t kind) {
    switch(kind){case 0:return SIGRTMIN;case 1:return SIGSEGV;case 2:return SIGBUS;case 3:return SIGFPE;case 4:return SIGILL;default:return -1;}
}
static void dispatch(int code,siginfo_t *info,void *context){
    int saved=errno;
    for(uint32_t i=0;i<5;++i)if(number(i)==code){
        dotnet_pal_signal_callback callback=(dotnet_pal_signal_callback)atomic_load_explicit(&callbacks[i],memory_order_acquire);
        if(callback)callback(code,info,context,(void*)atomic_load(&data_slots[i]));
        break;
    }
    errno=saved;
}
static uint64_t tag(void){
#if defined(__x86_64__)
    return DOTNET_PAL_CONTEXT_LINUX_X64;
#elif defined(__aarch64__)
    return DOTNET_PAL_CONTEXT_LINUX_ARM64;
#else
    return 0;
#endif
}
static size_t action_size(void){return sizeof(struct sigaction);}
static size_t action_alignment(void){return _Alignof(struct sigaction);}
static uint32_t install(uint32_t kind,dotnet_pal_signal_callback callback,void *data,void *previous,size_t size){
    if(kind>4 || !callback || !previous || size!=sizeof(struct sigaction))return DOTNET_PAL_INVALID_ARGUMENT;
    int zero=0;if(!atomic_compare_exchange_strong(&installed[kind],&zero,1))return DOTNET_PAL_BUSY;
    if(sigaction(number(kind),NULL,previous)!=0){atomic_store(&installed[kind],0);return DOTNET_PAL_OS_ERROR;}
    struct sigaction *old=previous,new_action={0};new_action.sa_flags=SA_RESTART|SA_SIGINFO;new_action.sa_sigaction=dispatch;
    sigemptyset(&new_action.sa_mask);
    if(old->sa_flags&SA_ONSTACK){new_action.sa_flags|=SA_ONSTACK;new_action.sa_mask=old->sa_mask;}
    atomic_store(&data_slots[kind],(uintptr_t)data);
    atomic_store_explicit(&callbacks[kind],(uintptr_t)callback,memory_order_release);
    if(sigaction(number(kind),&new_action,NULL)!=0){atomic_store(&installed[kind],0);return DOTNET_PAL_OS_ERROR;}
    return DOTNET_PAL_OK;
}
static uint32_t restore(uint32_t kind,const void *previous,size_t size){
    if(kind>4 || size!=sizeof(struct sigaction))return DOTNET_PAL_INVALID_ARGUMENT;
    return sigaction(number(kind),previous,NULL)==0?DOTNET_PAL_OK:DOTNET_PAL_OS_ERROR;
}
static uint32_t unblock(void){sigset_t set;sigemptyset(&set);sigaddset(&set,SIGRTMIN);return pthread_sigmask(SIG_UNBLOCK,&set,NULL)==0?0:3;}
static uint32_t request(uintptr_t token){int rc=pthread_kill((pthread_t)token,SIGRTMIN);return rc==0?0:rc==EAGAIN?6:rc==ESRCH?8:3;}
static uint32_t current(uintptr_t *out){*out=(uintptr_t)pthread_self();return 0;}
static uint32_t pid_async(uint64_t *out){*out=(uint64_t)getpid();return 0;}
static uint32_t broken_pipe(void){struct sigaction action={0};sigemptyset(&action.sa_mask);action.sa_handler=SIG_IGN;return sigaction(SIGPIPE,&action,NULL)==0?0:3;}
static const dotnet_pal_host_context table={
    {DOTNET_PAL_ABI_VERSION,sizeof(dotnet_pal_host_context),DOTNET_PAL_CAP_NATIVE_CONTEXT},
    {tag,action_size,action_alignment,install,restore,unblock,request,current,pid_async,broken_pipe,NULL,number}
};
const dotnet_pal_host_context *dotnet_pal_host_context_v2(void){
    if(pal_context_fault==1)return NULL;
    if(pal_context_fault>=2 && pal_context_fault<=5){
        static dotnet_pal_host_context bad;bad=table;
        if(pal_context_fault==2)bad.header.abi_version++;
        if(pal_context_fault==3)bad.header.struct_size=sizeof(dotnet_pal_header);
        if(pal_context_fault==4)bad.header.capabilities=0;
        if(pal_context_fault==5)bad.ops.process_id_async=NULL;
        return &bad;
    }
    return &table;
}
