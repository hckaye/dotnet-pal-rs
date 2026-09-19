#ifndef DOTNET_PAL_IMAGE_ADAPTER_H
#define DOTNET_PAL_IMAGE_ADAPTER_H
// Image group front end for the unwinder and the runtime: unwind-table lookup,
// readability probes, build ids, and the diagnostic line output the unwinder's
// logging macros use instead of stdio.
#include "dotnet_pal.h"
#include <atomic>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
namespace dotnet_pal_image {
inline std::atomic<const dotnet_pal_api*> installed{nullptr};
inline bool initialize() {
    auto *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION || a->header.struct_size < DOTNET_PAL_IMAGE_API_SIZE
        || !(a->header.capabilities & DOTNET_PAL_CAP_IMAGE)) return false;
    const auto *i = &a->image;
    if (!i->unwind_info || !i->readable || !i->build_id) return false;
    installed.store(a, std::memory_order_release);
    return true;
}
inline const dotnet_pal_api *require() {
    auto *a = installed.load(std::memory_order_acquire);
    if (!a) std::abort();
    return a;
}
inline bool unwind(uintptr_t address, dotnet_pal_unwind_info &out) {
    out = dotnet_pal_unwind_info{};
    return require()->image.unwind_info(address, &out, sizeof out) == DOTNET_PAL_OK;
}
// False when the range is not readable or the port cannot tell.
inline bool readable(uintptr_t address, size_t size) { return require()->image.readable(address, size) == DOTNET_PAL_OK; }
inline bool build_id(uintptr_t base, const void *&data, uint32_t &length) {
    static uint8_t storage[64]; size_t needed = 0;
    data = nullptr; length = 0;
    if (require()->image.build_id(base, storage, sizeof storage, &needed) != DOTNET_PAL_OK || needed > sizeof storage) return false;
    data = storage; length = static_cast<uint32_t>(needed);
    return true;
}
// One diagnostic line through the port's fatal-output channel. Formatting uses
// the C runtime's vsnprintf (a CRT contract, not an OS service); output does not.
inline void log(const char *format, ...) {
    auto *a = installed.load(std::memory_order_acquire);
    if (!a || a->header.struct_size < DOTNET_PAL_SUPPORT_API_SIZE || !a->support.write_stderr) return;
    char line[512];
    va_list args; va_start(args, format);
    int n = std::vsnprintf(line, sizeof line - 1, format, args);
    va_end(args);
    if (n < 0) return;
    size_t length = static_cast<size_t>(n) < sizeof line - 1 ? static_cast<size_t>(n) : sizeof line - 2;
    line[length++] = '\n';
    size_t written = 0;
    (void)a->support.write_stderr(reinterpret_cast<const uint8_t*>(line), length, &written);
}
}
#endif
