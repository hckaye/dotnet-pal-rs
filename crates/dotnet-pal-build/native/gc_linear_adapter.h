#ifndef DOTNET_PAL_GC_LINEAR_ADAPTER_H
#define DOTNET_PAL_GC_LINEAR_ADAPTER_H
// Explicit eager-storage GC profile, NOT CAP_VM or transparent VM emulation.
// No inaccessible reservation, physical decommit or memory.grow is promised.
#include "dotnet_pal.h"
#include <atomic>
#include <cstdlib>
#include <limits>

namespace dotnet_pal_gc_linear {
struct Stats {
    uint64_t reserve_ok, commit_ok, decommit_ok, release_ok, reset_ok, failures;
    uint64_t owned_bytes, peak_bytes;
};
struct Region { void *address; size_t size; };
inline Region regions[256]{};
inline Stats counters{};
inline std::atomic_flag gate = ATOMIC_FLAG_INIT;
struct Guard {
    Guard() { while (gate.test_and_set(std::memory_order_acquire)) {} }
    ~Guard() { gate.clear(std::memory_order_release); }
};
inline void increment(uint64_t &value) { if (value != UINT64_MAX) ++value; }
inline const dotnet_pal_api *api() {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!p || p->header.abi_version != DOTNET_PAL_ABI_VERSION ||
        p->header.struct_size < DOTNET_PAL_LINEAR_API_SIZE ||
        !(p->header.capabilities & DOTNET_PAL_CAP_LINEAR) ||
        (p->header.capabilities & DOTNET_PAL_CAP_VM)) return nullptr;
    const auto &s = p->linear;
    if (!s.granularity || !s.capacity || !s.allocate || !s.zero || !s.release || !s.read_stats) return nullptr;
    size_t grain = s.granularity();
    return grain && !(grain & (grain - 1)) ? p : nullptr;
}
inline const dotnet_pal_api *require() { const auto *p = api(); if (!p) std::abort(); return p; }
inline size_t rounded(size_t size) {
    size_t grain = require()->linear.granularity();
    if (!size || size > SIZE_MAX - (grain - 1)) return 0;
    return (size + grain - 1) & ~(grain - 1);
}
inline size_t page_range(void *address, size_t size) {
    size_t grain = require()->linear.granularity();
    if (!address || reinterpret_cast<uintptr_t>(address) % grain != 0) return 0;
    return rounded(size);
}
// Metadata is adapter-owned, bounded and protected by gate. Direct payload access
// must be synchronized with decommit/reset/release. No stale-handle guarantee.
inline Region *find(void *address, size_t size) {
    uintptr_t start = reinterpret_cast<uintptr_t>(address);
    if (!address || !size || start > UINTPTR_MAX - size) return nullptr;
    for (auto &r : regions) {
        uintptr_t base = reinterpret_cast<uintptr_t>(r.address);
        if (r.address && start >= base && start - base < r.size && size <= r.size - (start - base)) return &r;
    }
    return nullptr;
}
inline void *reserve(size_t size, size_t alignment, uint32_t flags, uint16_t node) {
    (void)node;
    Guard lock;
    size_t n = rounded(size);
    if (!n || flags) { increment(counters.failures); return nullptr; }
    Region *slot = nullptr;
    for (auto &r : regions) if (!r.address) { slot = &r; break; }
    if (!slot) { increment(counters.failures); return nullptr; }
    void *p = nullptr;
    if (require()->linear.allocate(n, alignment, 0, &p) != DOTNET_PAL_OK || !p) {
        increment(counters.failures); return nullptr;
    }
    *slot = {p, n}; increment(counters.reserve_ok);
    counters.owned_bytes += n;
    if (counters.owned_bytes > counters.peak_bytes) counters.peak_bytes = counters.owned_bytes;
    return p;
}
inline bool commit(void *address, size_t size, uint16_t node) {
    (void)node;
    Guard lock;
    size_t n = page_range(address, size);
    if (!find(address, n)) { increment(counters.failures); return false; }
    // Already allocated and accessible; repeated logical commit preserves data.
    increment(counters.commit_ok); return true;
}
inline bool zero(void *address, size_t size, bool reset) {
    Guard lock;
    size_t n = page_range(address, size);
    if (!find(address, n) || require()->linear.zero(address, n) != DOTNET_PAL_OK) {
        increment(counters.failures); return false;
    }
    increment(reset ? counters.reset_ok : counters.decommit_ok); return true;
}
inline bool decommit(void *address, size_t size) { return zero(address, size, false); }
inline bool reset(void *address, size_t size, bool unlock) { (void)unlock; return zero(address, size, true); }
inline bool release(void *address, size_t size) {
    Guard lock;
    size_t n = page_range(address, size);
    Region *r = find(address, n);
    if (!r || r->address != address || r->size != n ||
        require()->linear.release(address, n) != DOTNET_PAL_OK) {
        increment(counters.failures); return false;
    }
    counters.owned_bytes -= n; *r = {}; increment(counters.release_ok); return true;
}
inline void *large_pages(size_t, uint16_t) { return nullptr; }
inline Stats snapshot() { Guard lock; return counters; }
}
#endif
