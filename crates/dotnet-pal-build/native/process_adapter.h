#ifndef DOTNET_PAL_PROCESS_ADAPTER_H
#define DOTNET_PAL_PROCESS_ADAPTER_H
// Process group front end: orderly exit, debugger presence and the crash-dump
// utility launch. The group is optional for a port; the runtime treats an absent
// group as "no debugger can be detected, no crash dumps".
#include "dotnet_pal.h"
#include <atomic>
#include <cstdlib>
namespace dotnet_pal_process {
inline std::atomic<const dotnet_pal_process_ops*> installed{nullptr};
inline bool initialize() {
    auto *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION) return false;
    if (a->header.struct_size < DOTNET_PAL_PROCESS_API_SIZE || !(a->header.capabilities & DOTNET_PAL_CAP_PROCESS)) return true; // absent, not malformed
    const auto *p = &a->process;
    if (!p->exit || !p->debugger_present) return false;
    installed.store(p, std::memory_order_release);
    return true;
}
inline const dotnet_pal_process_ops *table() { return installed.load(std::memory_order_acquire); }
inline bool available() { return table() != nullptr; }
inline bool can_dump() { auto *p = table(); return p && p->crash_dump; }
[[noreturn]] inline void exit(int32_t code) {
    if (auto *p = table()) p->exit(code);
    // A port without an exit reaches its abort path instead of returning here.
    std::abort();
}
inline bool debugger_present() {
    auto *p = table(); uint32_t v = 0;
    return p && p->debugger_present(&v) == DOTNET_PAL_OK && v != 0;
}
// argv is NUL-terminated strings without a trailing NULL entry.
inline bool crash_dump(const char *const *argv, size_t argc, char *error, size_t error_capacity) {
    auto *p = table();
    if (!p || !p->crash_dump) return false;
    return p->crash_dump(reinterpret_cast<const uint8_t *const *>(argv), argc, reinterpret_cast<uint8_t*>(error), error_capacity) == DOTNET_PAL_OK;
}
}
#endif
