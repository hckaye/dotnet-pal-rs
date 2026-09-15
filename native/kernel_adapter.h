#ifndef DOTNET_PAL_KERNEL_ADAPTER_H
#define DOTNET_PAL_KERNEL_ADAPTER_H
#include "dotnet_pal.h"
#include <cstdlib>
#include <new>

namespace dotnet_pal_kernel {
inline const dotnet_pal_kernel_ops *api() {
    const auto *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION ||
        a->header.struct_size < DOTNET_PAL_KERNEL_API_SIZE ||
        (a->header.capabilities & DOTNET_PAL_CAP_KERNEL) != DOTNET_PAL_CAP_KERNEL) return nullptr;
    const auto *k = &a->kernel;
    if (!k->event_create || !k->event_destroy || !k->event_set || !k->event_reset || !k->event_wait ||
        !k->mutex_create || !k->mutex_destroy || !k->mutex_lock || !k->mutex_unlock ||
        !k->thread_create || !k->thread_join || !k->thread_detach || !k->tls_create ||
        !k->tls_destroy || !k->tls_get || !k->tls_set || !k->stack_bounds || !k->process_barrier) return nullptr;
    return k;
}
inline const dotnet_pal_kernel_ops *require() { auto *k = api(); if (!k) std::abort(); return k; }
inline void must(uint32_t status) { if (status != DOTNET_PAL_OK) std::abort(); }
inline bool mutex_init(void **h) { *h = nullptr; auto *k = api(); return k && k->mutex_create(1, h) == DOTNET_PAL_OK; }
inline void mutex_init_or_abort(void **h) { if (!mutex_init(h)) std::abort(); }
inline void mutex_destroy(void *h) { must(require()->mutex_destroy(h)); }
inline void mutex_lock(void *h) { must(require()->mutex_lock(h)); }
inline void mutex_unlock(void *h) { must(require()->mutex_unlock(h)); }
inline void barrier() { must(require()->process_barrier()); }
inline uint32_t wait_ms(void *h, uint32_t ms) {
    const uint64_t timeout = ms == UINT32_MAX ? DOTNET_PAL_INFINITE_NS : uint64_t(ms) * UINT64_C(1000000);
    const uint32_t status = require()->event_wait(h, timeout);
    return status == DOTNET_PAL_OK ? 0u : status == DOTNET_PAL_TIMEOUT ? 258u : UINT32_MAX;
}
// The runtime's uint32-returning callback is not ABI-cast to a void*-returning
// pthread callback. A real typed trampoline owns the small native bridge object.
struct BackgroundStart { uint32_t (*callback)(void *); void *arg; };
extern "C" inline void *background_start(void *arg) {
    auto *start = static_cast<BackgroundStart *>(arg);
    const auto callback = start->callback; void *context = start->arg;
    delete start;
    (void)callback(context);
    return nullptr;
}
inline bool start_background(uint32_t (*callback)(void *), void *arg, size_t stack) {
    auto *k = require();
    auto *start = new (std::nothrow) BackgroundStart{callback, arg};
    if (!start) return false;
    void *handle = nullptr;
    if (k->thread_create(background_start, start, stack, &handle) != DOTNET_PAL_OK) { delete start; return false; }
    // start may already be freed by the new thread. A detach failure cannot safely
    // be reported as "thread not started", so fail fast instead of retrying creation.
    must(k->thread_detach(handle));
    return true;
}
}
#endif
