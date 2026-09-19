#ifndef DOTNET_PAL_RUNTIME_ADAPTER_H
#define DOTNET_PAL_RUNTIME_ADAPTER_H
#include "dotnet_pal.h"
#include <atomic>
#include <cstdlib>
#include <cstring>
#include <limits>
namespace dotnet_pal_runtime {
inline std::atomic<const dotnet_pal_api*> installed{nullptr};
inline bool initialize() {
    auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    const uint64_t required = DOTNET_PAL_CAP_IDENTITY | DOTNET_PAL_CAP_REALTIME | DOTNET_PAL_CAP_NATIVE_MEMORY;
    if (!p || p->header.abi_version != DOTNET_PAL_ABI_VERSION || p->header.struct_size < DOTNET_PAL_RUNTIME_API_SIZE ||
        (p->header.capabilities & required) != required) return false;
    const auto &r = p->runtime;
    // Identity, the wall clock and native mappings are required; an environment,
    // entropy and module loading are services a port may honestly lack.
    if (!r.process_id || !r.thread_id || !r.realtime_ns || !r.mapping_allocate || !r.mapping_release || !r.mapping_protect) return false;
    installed.store(p, std::memory_order_release);
    return true;
}
inline const dotnet_pal_api *require() {
    auto *p = installed.load(std::memory_order_acquire);
    if (!p) std::abort();
    return p;
}
inline uint32_t environment(const char *name, char *buffer, uint32_t size) {
    size_t required = 0;
    if (!require()->runtime.environment_get) return 0; // no environment: every variable is unset
    auto status = require()->runtime.environment_get(reinterpret_cast<const uint8_t*>(name), std::strlen(name),
        reinterpret_cast<uint8_t*>(buffer), size, &required);
    if (status == DOTNET_PAL_OK) return static_cast<uint32_t>(required - 1);
    if (status == DOTNET_PAL_BUFFER_TOO_SMALL && required <= UINT32_MAX) return static_cast<uint32_t>(required);
    return 0;
}
inline uint64_t identity(bool thread) {
    uint64_t result = 0;
    auto &r = require()->runtime;
    if ((thread ? r.thread_id : r.process_id)(&result) != DOTNET_PAL_OK || result == 0) std::abort();
    return result;
}
inline bool permissions(uint32_t windows, uint32_t &result) {
    switch (windows & 255) {
        case 1: result = 0; return true;
        case 2: result = DOTNET_PAL_READ; return true;
        case 4: result = DOTNET_PAL_READ | DOTNET_PAL_WRITE; return true;
        case 32: result = DOTNET_PAL_READ | DOTNET_PAL_EXECUTE; return true;
        case 64: result = DOTNET_PAL_READ | DOTNET_PAL_WRITE | DOTNET_PAL_EXECUTE; return true;
        default: return false;
    }
}
inline void *allocate(size_t size, uint32_t access) {
    uint32_t prot = 0; void *result = nullptr;
    if (!permissions(access, prot)) return nullptr;
    return require()->runtime.mapping_allocate(size, prot, &result) == DOTNET_PAL_OK ? result : nullptr;
}
inline void release(void *address, size_t size) {
    if (require()->runtime.mapping_release(address, size) != DOTNET_PAL_OK) std::abort();
}
inline bool protect(void *address, size_t size, uint32_t access) {
    uint32_t prot = 0;
    if (!address || !size || !permissions(access, prot)) return false;
    auto *p = require();
    const size_t page = p->vm.page_size();
    if (!page || (page & (page-1))) return false;
    const uintptr_t start = reinterpret_cast<uintptr_t>(address);
    if (size > UINTPTR_MAX - start || start + size > UINTPTR_MAX - (page-1)) return false;
    const uintptr_t base = start & ~(page-1);
    const uintptr_t end = (start + size + page-1) & ~(page-1);
    return p->runtime.mapping_protect(reinterpret_cast<void*>(base), end-base, prot) == DOTNET_PAL_OK;
}
inline void *open(const char *name) {
    void *result = nullptr;
    if (!require()->runtime.module_open) return nullptr;
    const auto status = require()->runtime.module_open(reinterpret_cast<const uint8_t*>(name), name ? std::strlen(name) : 0, &result);
    return status == DOTNET_PAL_OK ? result : nullptr;
}
inline void *symbol(void *module, const char *name) {
    void *result = nullptr;
    if (!require()->runtime.module_symbol) return nullptr;
    const auto status = require()->runtime.module_symbol(module, reinterpret_cast<const uint8_t*>(name), std::strlen(name), &result);
    return status == DOTNET_PAL_OK ? result : nullptr;
}
inline void *module_base(void *address) {
    dotnet_pal_module_info info{};
    if (!require()->runtime.module_info) return nullptr;
    return require()->runtime.module_info(address, &info) == DOTNET_PAL_OK ? info.base : nullptr;
}
inline int32_t module_name(void *address, const char **name) {
    *name = nullptr;
    dotnet_pal_module_info info{};
    if (!require()->runtime.module_info || require()->runtime.module_info(address, &info) != DOTNET_PAL_OK || info.name_length > INT32_MAX) return 0;
    *name = reinterpret_cast<const char*>(info.name);
    return static_cast<int32_t>(info.name_length);
}
}
#endif
