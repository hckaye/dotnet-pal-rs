#ifndef DOTNET_PAL_CONTEXT_ADAPTER_H
#define DOTNET_PAL_CONTEXT_ADAPTER_H
#include "dotnet_pal.h"
#include <atomic>
#include <cstdlib>
#include <cerrno>
#include <signal.h>
namespace dotnet_pal_context {
inline std::atomic<const dotnet_pal_context_ops*> installed{nullptr};
inline bool initialize() {
    auto *a=dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.struct_size<DOTNET_PAL_CONTEXT_API_SIZE || !(a->header.capabilities&DOTNET_PAL_CAP_NATIVE_CONTEXT)) return false;
    const auto *c=&a->context;
    if (!c->abi_tag || !c->action_size || !c->action_alignment || !c->install || !c->restore ||
        !c->request_activation || !c->unblock_activation || !c->current_thread || !c->process_id_async ||
        !c->ignore_broken_pipe || !c->signal_number) return false;
#if defined(__linux__) && defined(__x86_64__)
    const uint64_t tag=DOTNET_PAL_CONTEXT_LINUX_X64;
#elif defined(__linux__) && defined(__aarch64__)
    const uint64_t tag=DOTNET_PAL_CONTEXT_LINUX_ARM64;
#else
    return false;
#endif
    if (c->abi_tag()!=tag || c->action_size()!=sizeof(struct sigaction) || c->action_alignment()!=alignof(struct sigaction)) return false;
    installed.store(c,std::memory_order_release);return true;
}
inline const dotnet_pal_context_ops* require() {
    auto *c=installed.load(std::memory_order_acquire);
    if (!c) std::abort();
    return c;
}
using Handler=void(*)(int,siginfo_t*,void*);
inline Handler handlers[5]{};
inline void dispatch(int32_t code,void *info,void *context,void *data) {
    // Never call through a cast-incompatible function pointer.
    auto handler=*static_cast<Handler*>(data);
    handler(code,static_cast<siginfo_t*>(info),context);
}
inline uint32_t kind(int code) {
    for(uint32_t i=0;i<5;++i) if(require()->signal_number(i)==code) return i;
    return UINT32_MAX;
}
inline bool add(int code,Handler handler,struct sigaction *previous) {
    const uint32_t k=kind(code);
    if(k==UINT32_MAX || !handler || handlers[k])return false;
    handlers[k]=handler;
    if(require()->install(k,dispatch,&handlers[k],previous,sizeof *previous)!=DOTNET_PAL_OK){handlers[k]=nullptr;return false;}
    return true;
}
inline void restore(int code,struct sigaction *previous) {
    const uint32_t k=kind(code);
    if(k==UINT32_MAX || require()->restore(k,previous,sizeof *previous)!=DOTNET_PAL_OK)std::abort();
}
inline uintptr_t thread_token() {
    uintptr_t token=0;
    if(require()->current_thread(&token)!=DOTNET_PAL_OK || token==0)std::abort();
    return token;
}
inline uint64_t process_id_async() {
    uint64_t id=0;
    if(require()->process_id_async(&id)!=DOTNET_PAL_OK || id==0)std::abort();
    return id;
}
inline int request(uintptr_t token) {
    switch(require()->request_activation(token)){
        case DOTNET_PAL_OK:return 0;
        case DOTNET_PAL_BUSY:return EAGAIN;
        case DOTNET_PAL_NOT_FOUND:return ESRCH;
        default:return EIO;
    }
}
inline void unblock(){if(require()->unblock_activation()!=DOTNET_PAL_OK)std::abort();}
inline void configure(){if(require()->ignore_broken_pipe()!=DOTNET_PAL_OK)std::abort();}
}
#endif
