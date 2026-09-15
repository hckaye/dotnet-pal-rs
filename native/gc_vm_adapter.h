#ifndef DOTNET_PAL_GC_VM_ADAPTER_H
#define DOTNET_PAL_GC_VM_ADAPTER_H
#include "dotnet_pal.h"

// Version-specific adaptation belongs here, NOT in the Rust/SDK boundary.
// Signatures audited against dotnet/runtime v10.0.0 (60629d14374c...).
namespace dotnet_pal_gc {
inline const dotnet_pal_api *api() {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    return p && p->header.struct_size >= sizeof(dotnet_pal_api) &&
        (p->header.capabilities & DOTNET_PAL_CAP_VM) ? p : nullptr;
}
inline void *reserve(size_t size, size_t alignment, uint32_t flags, uint16_t node) {
    (void)node; // NUMA is an advisory hint; placement is not implemented in this PoC.
    const auto *p = api();
    void *address = nullptr;
    return p && p->vm.reserve(size, alignment, flags, &address) == DOTNET_PAL_OK ? address : nullptr;
}
inline bool commit(void *address, size_t size, uint16_t node) {
    (void)node;
    const auto *p = api();
    return p && p->vm.commit(address, size) == DOTNET_PAL_OK;
}
inline bool decommit(void *address, size_t size) {
    const auto *p = api();
    return p && p->vm.decommit(address, size) == DOTNET_PAL_OK;
}
inline bool release(void *address, size_t size) {
    const auto *p = api();
    return p && p->vm.release(address, size) == DOTNET_PAL_OK;
}
inline bool reset(void *address, size_t size, bool unlock) {
    // The pinned Unix implementation also does not implement the unlock hint.
    (void)unlock;
    const auto *p = api();
    return p && p->vm.reset(address, size) == DOTNET_PAL_OK;
}
inline void *large_pages(size_t size, uint16_t node) {
    (void)size; (void)node;
    // Fail explicitly. Never mix upstream large-page mappings with host-owned memory.
    return nullptr;
}
}
#endif
