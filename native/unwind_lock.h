#ifndef DOTNET_PAL_UNWIND_LOCK_H
#define DOTNET_PAL_UNWIND_LOCK_H
#include "dotnet_pal.h"
#include <atomic>

/* A source-level libunwind lock adapter. Shared acquisitions deliberately use
 * the same recursive mutex: this preserves exclusion and recursive read locking,
 * but serializes readers. No pthread structure or static initializer crosses ABI.
 * Not async-signal-safe (neither was libunwind's original pthread rwlock).
 */
class DotnetPalUnwindLock {
    std::atomic<void*> handle{nullptr};
    static const dotnet_pal_kernel_ops *ops() {
        const auto *api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
        if (!api || api->header.abi_version != DOTNET_PAL_ABI_VERSION ||
            api->header.struct_size < DOTNET_PAL_KERNEL_API_SIZE ||
            !(api->header.capabilities & DOTNET_PAL_CAP_MUTEX)) return nullptr;
        const auto &k = api->kernel;
        return k.mutex_create && k.mutex_destroy && k.mutex_lock && k.mutex_unlock ? &k : nullptr;
    }
    void *get(const dotnet_pal_kernel_ops *k) {
        void *value = handle.load(std::memory_order_acquire);
        if (value) return value;
        void *candidate = nullptr;
        if (k->mutex_create(1, &candidate) != DOTNET_PAL_OK || !candidate) return nullptr;
        if (handle.compare_exchange_strong(value, candidate, std::memory_order_acq_rel, std::memory_order_acquire))
            return candidate;
        // Another thread published its lock; our never-used candidate is exclusive.
        if (k->mutex_destroy(candidate) != DOTNET_PAL_OK) return nullptr;
        return value;
    }
public:
    constexpr DotnetPalUnwindLock() noexcept = default;
    DotnetPalUnwindLock(const DotnetPalUnwindLock&) = delete;
    DotnetPalUnwindLock& operator=(const DotnetPalUnwindLock&) = delete;
    // The upstream lock is static/process-lifetime. Do not introduce teardown
    // ordering with late native/managed finalization by adding a destructor.
    bool lock() { const auto *k = ops(); if (!k) return false;
        void *h = get(k); return h && k->mutex_lock(h) == DOTNET_PAL_OK; }
    bool unlock() { const auto *k = ops(); void *h = handle.load(std::memory_order_acquire);
        return k && h && k->mutex_unlock(h) == DOTNET_PAL_OK; }
    bool lock_shared() { return lock(); }
    bool unlock_shared() { return unlock(); }
    // Explicitly used by conformance tests after all threads join. Runtime static
    // lock teardown is intentionally not performed automatically.
    bool destroy() { const auto *k = ops(); void *h = handle.load(std::memory_order_acquire);
        if (!h) return true;
        if (!k || k->mutex_destroy(h) != DOTNET_PAL_OK) return false;
        handle.store(nullptr, std::memory_order_release); return true; }
};
#endif
