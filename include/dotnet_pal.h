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
#define DOTNET_PAL_CAP_DYNAMIC_LINEAR UINT64_C(262144)
#define DOTNET_PAL_CAP_CLOCK UINT64_C(4)
#define DOTNET_PAL_CAP_SCHEDULER UINT64_C(8)
#define DOTNET_PAL_OK 0u
#define DOTNET_PAL_UNSUPPORTED 1u
#define DOTNET_PAL_INVALID_ARGUMENT 2u
#define DOTNET_PAL_OS_ERROR 3u
#define DOTNET_PAL_OUT_OF_MEMORY 4u
#define DOTNET_PAL_TIMEOUT 5u
#define DOTNET_PAL_BUSY 6u
#define DOTNET_PAL_INFINITE_NS UINT64_MAX
#define DOTNET_PAL_CAP_EVENTS UINT64_C(16)
#define DOTNET_PAL_CAP_MUTEX UINT64_C(32)
#define DOTNET_PAL_CAP_THREADS UINT64_C(64)
#define DOTNET_PAL_CAP_TLS UINT64_C(128)
#define DOTNET_PAL_CAP_STACK UINT64_C(256)
#define DOTNET_PAL_CAP_PROCESS_BARRIER UINT64_C(512)
#define DOTNET_PAL_CAP_KERNEL UINT64_C(1008)

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

/* Kernel handles are opaque, native-owned objects. Closing a handle consumes it.
 * The caller owns lifetime synchronization: no use after close, no concurrent close,
 * no asynchronous cancellation, and no unwind across callbacks. See docs/kernel.md.
 */
typedef void *(*dotnet_pal_thread_entry)(void *arg);
typedef void (*dotnet_pal_tls_destructor)(void *value);
typedef struct {
    uint64_t event_create_ok, event_wait_ok, event_timeout, event_set_ok;
    uint64_t mutex_create_ok, mutex_lock_ok, thread_create_ok, tls_create_ok, tls_set_ok;
    uint64_t stack_bounds_ok, barrier_ok, rejected_or_failed;
} dotnet_pal_kernel_stats;
typedef struct {
    uint32_t (*event_create)(uint32_t manual_reset, uint32_t initial_state, void **out);
    uint32_t (*event_destroy)(void *handle);
    uint32_t (*event_set)(void *handle);
    uint32_t (*event_reset)(void *handle);
    uint32_t (*event_wait)(void *handle, uint64_t timeout_ns);
    uint32_t (*mutex_create)(uint32_t recursive, void **out);
    uint32_t (*mutex_destroy)(void *handle);
    uint32_t (*mutex_lock)(void *handle);
    uint32_t (*mutex_unlock)(void *handle);
    uint32_t (*thread_create)(dotnet_pal_thread_entry entry, void *arg, size_t stack_size, void **out);
    uint32_t (*thread_join)(void *handle);
    uint32_t (*thread_detach)(void *handle);
    uint32_t (*tls_create)(dotnet_pal_tls_destructor destructor, void **out);
    uint32_t (*tls_destroy)(void *handle);
    uint32_t (*tls_get)(void *handle, void **out);
    uint32_t (*tls_set)(void *handle, void *value);
    uint32_t (*stack_bounds)(void **low, void **high);
    uint32_t (*process_barrier)(void);
    uint32_t (*read_stats)(dotnet_pal_kernel_stats *out, size_t out_size);
} dotnet_pal_kernel_ops;
typedef struct {
    dotnet_pal_header header;
    dotnet_pal_kernel_ops ops;
} dotnet_pal_host_kernel;

/* Runtime extension. All string inputs are byte borrows, not NUL-terminated.
 * Input/output buffers must not overlap. No environment mutation may run in
 * parallel with environment_get. required includes NUL on OK/BUFFER_TOO_SMALL.
 * NOT_FOUND is distinct from an empty value. Failed output buffers are sanitized.
 * ModuleInfo.name is a borrowed NUL-terminated name, valid while its module stays
 * loaded; the caller prevents concurrent module unload. No function is promised
 * async-signal-safe. Mappings are explicit native storage, not GC reservations.
 */
#define DOTNET_PAL_BUFFER_TOO_SMALL 7u
#define DOTNET_PAL_NOT_FOUND 8u
#define DOTNET_PAL_CAP_ENVIRONMENT UINT64_C(1024)
#define DOTNET_PAL_CAP_IDENTITY UINT64_C(2048)
#define DOTNET_PAL_CAP_REALTIME UINT64_C(4096)
#define DOTNET_PAL_CAP_ENTROPY UINT64_C(8192)
#define DOTNET_PAL_CAP_NATIVE_MEMORY UINT64_C(16384)
#define DOTNET_PAL_CAP_MODULES UINT64_C(32768)
#define DOTNET_PAL_CAP_RUNTIME UINT64_C(64512)
#define DOTNET_PAL_READ 1u
#define DOTNET_PAL_WRITE 2u
#define DOTNET_PAL_EXECUTE 4u
#define DOTNET_PAL_MAX_NAME 4095u

typedef struct { void *base; const uint8_t *name; size_t name_length; } dotnet_pal_module_info;
typedef struct {
    uint64_t environment_ok, identity_ok, realtime_ok, entropy_ok;
    uint64_t mapping_allocate_ok, mapping_release_ok, mapping_protect_ok;
    uint64_t module_open_ok, module_symbol_ok, module_close_ok, module_info_ok, rejected_or_failed;
} dotnet_pal_runtime_stats;
typedef struct {
    uint32_t (*environment_get)(const uint8_t*, size_t, uint8_t*, size_t, size_t*);
    uint32_t (*process_id)(uint64_t*);
    uint32_t (*thread_id)(uint64_t*);
    uint32_t (*realtime_ns)(uint64_t*);
    uint32_t (*random_bytes)(uint8_t*, size_t);
    uint32_t (*mapping_allocate)(size_t, uint32_t, void**);
    uint32_t (*mapping_release)(void*, size_t);
    uint32_t (*mapping_protect)(void*, size_t, uint32_t);
    uint32_t (*module_open)(const uint8_t*, size_t, void**);
    uint32_t (*module_symbol)(void*, const uint8_t*, size_t, void**);
    uint32_t (*module_close)(void*);
    uint32_t (*module_info)(void*, dotnet_pal_module_info*);
    uint32_t (*read_stats)(dotnet_pal_runtime_stats*, size_t);
} dotnet_pal_runtime_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_runtime_ops ops; } dotnet_pal_host_runtime;

/* Optional complete raw WASIp1 transport. errno values are WASI 0..76,
 * NOT dotnet_pal status codes. Arguments are canonical 64-bit slots; wasm32
 * offsets and u32 values must fit in 32 bits. No implicit host fallback exists.
 */
#define DOTNET_PAL_CAP_WASI_DISPATCH UINT64_C(65536)
typedef struct { uint64_t calls, rejected, host_errors; } dotnet_pal_wasi_stats;
typedef struct {
    uint32_t (*invoke)(uint32_t opcode, const uint64_t *args, uint32_t argc);
    uint64_t (*call_count)(uint32_t opcode);
    uint32_t (*read_stats)(dotnet_pal_wasi_stats*, size_t);
} dotnet_pal_wasi_ops;
/* Architecture-bound native context extension. Signal info, CPU context and
 * previous actions are opaque SDK-native borrows, NOT portable wire structures.
 * The adapter must check abi_tag AND previous-action size/alignment before use.
 * Callback+data and previous-action storage live until process termination, even
 * after restore (in-flight callbacks can finish). Install is once per kind.
 * Callbacks may not unwind, allocate or enter managed code. Any suspension/wait
 * protocol must be specifically proven safe for the interrupted runtime state;
 * ordinary blocking PAL calls are not generally signal-safe. process_id_async
 * and restore may be called by a signal callback;
 * table lookup/registration must be completed before enabling signals.
 */
#define DOTNET_PAL_CAP_NATIVE_CONTEXT UINT64_C(131072)
#define DOTNET_PAL_CONTEXT_LINUX_X64 UINT64_C(0x4c4e580000000001)
#define DOTNET_PAL_CONTEXT_LINUX_ARM64 UINT64_C(0x4c4e580000000002)
#define DOTNET_PAL_SIGNAL_ACTIVATION 0u
#define DOTNET_PAL_SIGNAL_SEGMENTATION 1u
#define DOTNET_PAL_SIGNAL_BUS 2u
#define DOTNET_PAL_SIGNAL_FLOATING_POINT 3u
#define DOTNET_PAL_SIGNAL_ILLEGAL_INSTRUCTION 4u
typedef void (*dotnet_pal_signal_callback)(int32_t,void*,void*,void*);
typedef struct {uint64_t installs,restores,requests,unblocks,thread_queries,rejected;} dotnet_pal_context_stats;
typedef struct {
    uint64_t (*abi_tag)(void);
    size_t (*action_size)(void);
    size_t (*action_alignment)(void);
    uint32_t (*install)(uint32_t,dotnet_pal_signal_callback,void*,void*,size_t);
    uint32_t (*restore)(uint32_t,const void*,size_t);
    uint32_t (*unblock_activation)(void);
    uint32_t (*request_activation)(uintptr_t);
    uint32_t (*current_thread)(uintptr_t*);
    uint32_t (*process_id_async)(uint64_t*);
    uint32_t (*ignore_broken_pipe)(void);
    uint32_t (*read_stats)(dotnet_pal_context_stats*,size_t);
    int32_t (*signal_number)(uint32_t);
} dotnet_pal_context_ops;
typedef struct {dotnet_pal_header header;dotnet_pal_context_ops ops;} dotnet_pal_host_context;

/* Native helper heap, rwlocks and diagnostics are NOT GC virtual memory.
 * Heap results use the target C allocator alignment. Resize failure preserves
 * the old allocation; zero-sized requests are rejected. Closing/resize requires
 * exclusive lifetime ownership. No callback may unwind or reenter the PAL.
 * write_stderr completes the request or returns an error with validated progress.
 * Only write_stderr is required to be async-signal-safe after table negotiation.
 * Thread names are UTF-8/byte strings without NUL; Linux accepts at most 15 bytes.
 */
#define DOTNET_PAL_CAP_NATIVE_HEAP UINT64_C(524288)
#define DOTNET_PAL_CAP_RWLOCK UINT64_C(1048576)
#define DOTNET_PAL_CAP_THREAD_NAME UINT64_C(2097152)
#define DOTNET_PAL_CAP_DIAGNOSTICS UINT64_C(4194304)
#define DOTNET_PAL_CAP_SUPPORT UINT64_C(7864320)
typedef struct {
    uint64_t allocate_ok, resize_ok, release_ok, rw_create_ok, rw_read_ok;
    uint64_t rw_write_ok, rw_unlock_ok, rw_destroy_ok, write_ok, name_ok, rejected;
} dotnet_pal_support_stats;
typedef struct {
    uint32_t (*allocate)(size_t size, uint32_t zero, void **out);
    uint32_t (*resize)(void *address, size_t new_size, void **out);
    uint32_t (*release)(void *address);
    uint32_t (*rw_create)(void **out);
    uint32_t (*rw_read)(void *handle);
    uint32_t (*rw_write)(void *handle);
    uint32_t (*rw_unlock)(void *handle);
    uint32_t (*rw_destroy)(void *handle);
    uint32_t (*write_stderr)(const uint8_t *data, size_t size, size_t *written);
    uint32_t (*thread_name)(const uint8_t *name, size_t length);
    uint32_t (*read_stats)(dotnet_pal_support_stats *out, size_t size);
} dotnet_pal_support_ops;
typedef struct {dotnet_pal_header header;dotnet_pal_support_ops ops;} dotnet_pal_host_support;

typedef struct {
    dotnet_pal_header header;
    dotnet_pal_vm_ops vm;
    uint32_t (*read_stats)(dotnet_pal_stats *out, size_t out_size);
    dotnet_pal_linear_ops linear;
    /* ABI 2 append-only extension. All preceding offsets stay unchanged. */
    dotnet_pal_services_ops services;
    dotnet_pal_kernel_ops kernel;
    dotnet_pal_runtime_ops runtime;
    dotnet_pal_wasi_ops wasi;
    dotnet_pal_context_ops context;
    dotnet_pal_support_ops support;
} dotnet_pal_api;

/* Use size checks BEFORE reading a capability group from a foreign table.
 * Each size marks the END of that group, not sizeof a future extended API. */
#define DOTNET_PAL_SUPPORT_API_SIZE (offsetof(dotnet_pal_api, support) + sizeof(dotnet_pal_support_ops))
#define DOTNET_PAL_CONTEXT_API_SIZE (offsetof(dotnet_pal_api, context) + sizeof(dotnet_pal_context_ops))
#define DOTNET_PAL_WASI_API_SIZE (offsetof(dotnet_pal_api, wasi) + sizeof(dotnet_pal_wasi_ops))
#define DOTNET_PAL_RUNTIME_API_SIZE (offsetof(dotnet_pal_api, runtime) + sizeof(dotnet_pal_runtime_ops))
#define DOTNET_PAL_VM_API_SIZE offsetof(dotnet_pal_api, linear)
#define DOTNET_PAL_LINEAR_API_SIZE offsetof(dotnet_pal_api, services)
#define DOTNET_PAL_KERNEL_API_SIZE (offsetof(dotnet_pal_api, kernel) + sizeof(dotnet_pal_kernel_ops))
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

/* linear-heap only: backend hooks, not additional runtime-facing entry points.
 * Allocate uninitialized exclusive storage aligned to alignment; NULL on failure.
 * Release is all-or-nothing and must preserve storage on failure. No reentry into
 * PAL, unwinding, cancellation, or managed callbacks. They run under the ledger
 * lock. Use the SAME allocator for both hooks. On WASI, share wasi-libc's allocator
 * rather than calling memory.grow behind its program-break bookkeeping.
 */
uint32_t dotnet_pal_storage_allocate_v2(size_t size, size_t alignment, void **out);
uint32_t dotnet_pal_storage_release_v2(void *address, size_t size);


/* host: VM table. host-services: additionally requires the separate services
 * table. Both must be immutable, readable, and valid before runtime startup.
 * Callbacks must be thread-safe, must not throw/unwind or reenter managed code.
 * Providers must supply at least a readable header even for a rejected table.
 */
const dotnet_pal_host_api *dotnet_pal_host_v2(void);
const dotnet_pal_host_services *dotnet_pal_host_services_v2(void);
/* Required only for host-kernel; legacy providers need no new symbols. */
const dotnet_pal_host_kernel *dotnet_pal_host_kernel_v2(void);
/* Required only by host-runtime. */
const dotnet_pal_host_runtime *dotnet_pal_host_runtime_v2(void);
const dotnet_pal_host_context *dotnet_pal_host_context_v2(void);
/* Required only by host-support. */
const dotnet_pal_host_support *dotnet_pal_host_support_v2(void);
#if defined(__cplusplus)
[[noreturn]] void dotnet_pal_host_abort(void);
#else
_Noreturn void dotnet_pal_host_abort(void);
#endif
#ifdef __cplusplus
}
#endif
#endif
