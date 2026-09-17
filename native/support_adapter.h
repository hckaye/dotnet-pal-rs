#ifndef DOTNET_PAL_SUPPORT_ADAPTER_H
#define DOTNET_PAL_SUPPORT_ADAPTER_H
#include "dotnet_pal.h"
#include <atomic>
#include <cstring>
namespace dotnet_pal_support {
inline std::atomic<const dotnet_pal_support_ops*> installed{nullptr};
static_assert(std::atomic<void*>::is_always_lock_free, "signal diagnostics require lock-free pointer publication");
inline bool initialize() {
    const auto *a=dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if(!a || a->header.abi_version!=DOTNET_PAL_ABI_VERSION || a->header.struct_size<DOTNET_PAL_SUPPORT_API_SIZE
        || (a->header.capabilities&DOTNET_PAL_CAP_SUPPORT)!=DOTNET_PAL_CAP_SUPPORT)return false;
    const auto *s=&a->support;
    if(!s->allocate || !s->resize || !s->release || !s->rw_create || !s->rw_read || !s->rw_write
        || !s->rw_unlock || !s->rw_destroy || !s->write_stderr || !s->thread_name)return false;
    installed.store(s,std::memory_order_release);return true;
}
inline const dotnet_pal_support_ops *require() {
    auto *s=installed.load(std::memory_order_acquire);
    if(!s){if(!initialize())__builtin_trap();s=installed.load(std::memory_order_acquire);}
    return s;
}
inline void *allocate(size_t n) {
    void *p=nullptr;
    if(require()->allocate(n?n:1,0,&p)!=DOTNET_PAL_OK)return nullptr;
    return p;
}
inline void release(void *p) {if(p && require()->release(p)!=DOTNET_PAL_OK)__builtin_trap();}
inline bool name(const char *n) {
    if(!n)return false;
    // This adapter targets the Linux native profile, whose names are 15 bytes.
    size_t len=std::strlen(n);if(len>15)len=15;
    return require()->thread_name(reinterpret_cast<const uint8_t*>(n),len)==DOTNET_PAL_OK;
}
inline void fatal_message(const char *message) {
    // No lazy initialization, foreign table discovery, allocation or blocking
    // lock in the stack-overflow/signal path. PalInit publishes this table first.
    auto *s=installed.load(std::memory_order_acquire);
    if(!s || !message)__builtin_trap();
    size_t size=0;while(message[size])++size;
    size_t written=0;
    (void)s->write_stderr(reinterpret_cast<const uint8_t*>(message),size,&written);
}
inline uint64_t filetime() {
    uint64_t ns=0;
    auto *a=dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if(!a || a->header.struct_size<DOTNET_PAL_RUNTIME_API_SIZE || !(a->header.capabilities&DOTNET_PAL_CAP_REALTIME)
        || !a->runtime.realtime_ns || a->runtime.realtime_ns(&ns)!=DOTNET_PAL_OK)__builtin_trap();
    // The uint64 nanosecond input bounds this sum well below UINT64_MAX.
    return ns/100+UINT64_C(116444736000000000);
}
class ReadWriteLock {
    std::atomic<void*> handle{nullptr};
    void *get() {
        void *h=handle.load(std::memory_order_acquire);
        if(h)return h;
        void *candidate=nullptr;
        const auto *s=require();
        if(s->rw_create(&candidate)!=DOTNET_PAL_OK)return nullptr;
        if(handle.compare_exchange_strong(h,candidate,std::memory_order_release,std::memory_order_acquire))return candidate;
        if(s->rw_destroy(candidate)!=DOTNET_PAL_OK)__builtin_trap();
        return h;
    }
public:
    constexpr ReadWriteLock() noexcept = default;
    bool initialize() { return get()!=nullptr; }
    ReadWriteLock(const ReadWriteLock&)=delete;
    ReadWriteLock& operator=(const ReadWriteLock&)=delete;
    bool lock_shared(){void *h=get();return h && require()->rw_read(h)==DOTNET_PAL_OK;}
    bool lock(){void *h=get();return h && require()->rw_write(h)==DOTNET_PAL_OK;}
    bool unlock_shared(){void *h=handle.load(std::memory_order_acquire);return h && require()->rw_unlock(h)==DOTNET_PAL_OK;}
    bool unlock(){return unlock_shared();}
    // libunwind cache locks have process lifetime, like their original static
    // pthread initializers. Do not race a destructor against runtime shutdown.
};
}
#endif
