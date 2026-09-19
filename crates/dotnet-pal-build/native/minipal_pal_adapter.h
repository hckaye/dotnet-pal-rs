#ifndef DOTNET_PAL_MINIPAL_ADAPTER_H
#define DOTNET_PAL_MINIPAL_ADAPTER_H
/* C front end used by the patched minipal sources (clock, thread id, debugger,
 * entropy, log output, mutexes, CPU count and feature words). minipal is C, so
 * this header is C99; it reads the negotiated table lazily because minipal
 * functions run before PalInit (static constructors, GS cookie seeding). */
#include "dotnet_pal.h"
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
static inline const dotnet_pal_api *dotnet_pal_minipal_api(void) {
    const dotnet_pal_api *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION) __builtin_trap();
    return a;
}
static inline bool dotnet_pal_minipal_has(const dotnet_pal_api *a, size_t size, uint64_t capability) {
    return a->header.struct_size >= size && (a->header.capabilities & capability) == capability;
}
/* Monotonic nanoseconds; the runtime cannot run without a clock. */
static inline int64_t dotnet_pal_minipal_ticks_ns(void) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    uint64_t ns = 0;
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_SERVICES_API_SIZE, DOTNET_PAL_CAP_CLOCK) || !a->services.monotonic_ns
        || a->services.monotonic_ns(&ns) != DOTNET_PAL_OK || ns > (uint64_t)INT64_MAX) __builtin_trap();
    return (int64_t)ns;
}
/* Blocking delay; false when the port has no scheduler (the caller spins). */
static inline bool dotnet_pal_minipal_sleep_ns(uint64_t ns) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    return dotnet_pal_minipal_has(a, DOTNET_PAL_SERVICES_API_SIZE, DOTNET_PAL_CAP_SCHEDULER) && a->services.sleep_ns
        && a->services.sleep_ns(ns) == DOTNET_PAL_OK;
}
static inline size_t dotnet_pal_minipal_thread_id(void) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    uint64_t id = 0;
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_IDENTITY) || !a->runtime.thread_id
        || a->runtime.thread_id(&id) != DOTNET_PAL_OK || id == 0 || id > (uint64_t)SIZE_MAX) __builtin_trap();
    return (size_t)id;
}
static inline bool dotnet_pal_minipal_can_check_debugger(void) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    return dotnet_pal_minipal_has(a, DOTNET_PAL_PROCESS_API_SIZE, DOTNET_PAL_CAP_PROCESS) && a->process.debugger_present != NULL;
}
static inline bool dotnet_pal_minipal_debugger_present(void) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    uint32_t present = 0;
    return dotnet_pal_minipal_can_check_debugger() && a->process.debugger_present(&present) == DOTNET_PAL_OK && present != 0;
}
/* 0 on success, -1 when the port has no entropy source. */
static inline int32_t dotnet_pal_minipal_random(uint8_t *buffer, int32_t length) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    if (length < 0) return -1;
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_ENTROPY) || !a->runtime.random_bytes) return -1;
    return a->runtime.random_bytes(buffer, (size_t)length) == DOTNET_PAL_OK ? 0 : -1;
}
/* Diagnostic text; the port's single fatal-output channel carries every log level. */
static inline size_t dotnet_pal_minipal_write(const char *text, size_t length) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    size_t written = 0;
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_SUPPORT_API_SIZE, DOTNET_PAL_CAP_DIAGNOSTICS) || !a->support.write_stderr) return 0;
    (void)a->support.write_stderr((const uint8_t*)text, length, &written);
    return written;
}
static inline const dotnet_pal_kernel_ops *dotnet_pal_minipal_kernel(void) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_KERNEL_API_SIZE, DOTNET_PAL_CAP_MUTEX) || !a->kernel.mutex_create
        || !a->kernel.mutex_lock || !a->kernel.mutex_unlock || !a->kernel.mutex_destroy) __builtin_trap();
    return &a->kernel;
}
static inline uint32_t dotnet_pal_minipal_cpu_max(void) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    uint32_t count = 0;
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_TOPOLOGY_API_SIZE, DOTNET_PAL_CAP_TOPOLOGY) || !a->topology.cpu_max
        || a->topology.cpu_max(&count) != DOTNET_PAL_OK || count == 0) __builtin_trap();
    return count;
}
static inline void dotnet_pal_minipal_cpu_features(uint64_t *first, uint64_t *second) {
    const dotnet_pal_api *a = dotnet_pal_minipal_api();
    *first = 0; *second = 0;
    if (!dotnet_pal_minipal_has(a, DOTNET_PAL_TOPOLOGY_API_SIZE, DOTNET_PAL_CAP_TOPOLOGY) || !a->topology.cpu_features
        || a->topology.cpu_features(first, second) != DOTNET_PAL_OK) __builtin_trap();
}
#endif
