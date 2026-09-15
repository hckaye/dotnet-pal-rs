#ifndef DOTNET_PAL_H
#define DOTNET_PAL_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define DOTNET_PAL_ABI_VERSION 2u
/* Capabilities are independent; no VM emulation is implied by linear storage. */
#define DOTNET_PAL_CAP_VM UINT64_C(1)
#define DOTNET_PAL_CAP_LINEAR UINT64_C(2)
#define DOTNET_PAL_CAP_CLOCK UINT64_C(4)
#define DOTNET_PAL_CAP_SCHEDULER UINT64_C(8)
#define DOTNET_PAL_OK 0u
#define DOTNET_PAL_UNSUPPORTED 1u
#define DOTNET_PAL_INVALID_ARGUMENT 2u
#define DOTNET_PAL_OS_ERROR 3u
#define DOTNET_PAL_OUT_OF_MEMORY 4u

typedef struct {
    uint32_t abi_version;
    uint32_t struct_size;
    uint64_t capabilities;
} dotnet_pal_header;

typedef struct {
    size_t (*page_size)(void);
    uint32_t (*reserve)(size_t size, size_t alignment, uint32_t flags, void **out);
    uint32_t (*commit)(void *address, size_t size);
    uint32_t (*decommit)(void *address, size_t size);
    uint32_t (*release)(void *address, size_t size);
    uint32_t (*reset)(void *address, size_t size);
} dotnet_pal_vm_ops;

typedef struct {
    uint64_t reserve_ok, commit_ok, decommit_ok, release_ok, reset_ok, rejected_or_failed;
} dotnet_pal_stats;

typedef struct {
    uint64_t allocate_ok, zero_ok, release_ok, rejected_or_failed;
} dotnet_pal_linear_stats;

/* NOT a substitute for vm.commit/decommit. allocate zeroes the rounded allocation;
 * release accepts only the full allocation. zero accepts a byte subrange within
 * ONE live allocation. No protection, sparse reservation, memory.grow or physical
 * reclamation guarantee. See docs/architecture.md for ownership requirements.
 */
typedef struct {
    size_t (*granularity)(void);
    size_t (*capacity)(void);
    uint32_t (*allocate)(size_t size, size_t alignment, uint32_t flags, void **out);
    uint32_t (*zero)(void *address, size_t size);
    uint32_t (*release)(void *address, size_t size);
    uint32_t (*read_stats)(dotnet_pal_linear_stats *out, size_t out_size);
} dotnet_pal_linear_ops;

typedef struct {
    uint64_t clock_ok, sleep_ok, yield_ok, rejected_or_failed;
} dotnet_pal_services_stats;

/* CLOCK: monotonic nanoseconds from an unspecified, host-defined epoch; not UTC.
 * SCHEDULER: relative sleep (retry interruption), and a scheduling hint, not a
 * thread switch guarantee. Callbacks are NULL unless their capability is present.
 * A valid clock output is cleared on failure; invalid output storage is not used.
 * None of these functions is promised to be signal/interrupt-safe.
 */
typedef struct {
    uint32_t (*monotonic_ns)(uint64_t *out);
    uint32_t (*sleep_ns)(uint64_t nanoseconds);
    uint32_t (*yield_thread)(void);
    uint32_t (*read_stats)(dotnet_pal_services_stats *out, size_t out_size);
} dotnet_pal_services_ops;

typedef struct {
    dotnet_pal_header header;
    dotnet_pal_vm_ops vm;
    uint32_t (*read_stats)(dotnet_pal_stats *out, size_t out_size);
    dotnet_pal_linear_ops linear;
    /* ABI 2 append-only extension. All preceding offsets stay unchanged. */
    dotnet_pal_services_ops services;
} dotnet_pal_api;

/* Use size checks BEFORE reading a capability group from a foreign table.
 * Each size marks the END of that group, not sizeof a future extended API. */
#define DOTNET_PAL_VM_API_SIZE offsetof(dotnet_pal_api, linear)
#define DOTNET_PAL_LINEAR_API_SIZE offsetof(dotnet_pal_api, services)
#define DOTNET_PAL_SERVICES_API_SIZE (offsetof(dotnet_pal_api, services) + sizeof(dotnet_pal_services_ops))

typedef struct {
    dotnet_pal_header header;
    dotnet_pal_vm_ops vm;
} dotnet_pal_host_api;

typedef struct {
    dotnet_pal_header header;
    uint32_t (*monotonic_ns)(uint64_t *out);
    uint32_t (*sleep_ns)(uint64_t nanoseconds);
    uint32_t (*yield_thread)(void);
} dotnet_pal_host_services;

/* Immutable process-lifetime table or NULL. Inspect version, size AND capabilities.
 * Missing groups contain NULL callbacks. ABI is per-target C, not a wire format. */
const dotnet_pal_api *dotnet_pal_get_api(uint32_t version);

/* host: VM table. host-services: additionally requires the separate services
 * table. Both must be immutable, readable, and valid before runtime startup.
 * Callbacks must be thread-safe, must not throw/unwind or reenter managed code.
 * Providers must supply at least a readable header even for a rejected table.
 */
const dotnet_pal_host_api *dotnet_pal_host_v2(void);
const dotnet_pal_host_services *dotnet_pal_host_services_v2(void);
#if defined(__cplusplus)
[[noreturn]] void dotnet_pal_host_abort(void);
#else
_Noreturn void dotnet_pal_host_abort(void);
#endif
#ifdef __cplusplus
}
#endif
#endif
