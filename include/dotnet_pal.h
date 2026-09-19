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

/* Machine topology and memory accounting (append-only group). Counts are logical
 * CPUs; masks are little-endian bitmaps (bit n = CPU n). physical_memory reports
 * the total and available bytes within the limit in force for this process
 * (container or job limit when one exists), memory_limit that limit or 0 when
 * none, virtual_limit the address-space limit or 0. cache_size is the largest
 * per-CPU data cache or 0 when unknown. cpu_features returns two target-defined
 * words (Linux arm64: AT_HWCAP and AT_HWCAP2; zero on targets without them).
 * NUMA placement is not part of the boundary: consumers see one node. */
#define DOTNET_PAL_CAP_TOPOLOGY UINT64_C(8388608)
typedef struct {
    uint64_t cpu_ok, affinity_ok, memory_ok, cache_ok, features_ok, rejected_or_failed;
} dotnet_pal_topology_stats;
typedef struct {
    uint32_t (*cpu_max)(uint32_t *out);
    uint32_t (*cpu_count)(uint32_t *out);
    uint32_t (*current_cpu)(uint32_t *out);
    uint32_t (*process_affinity)(uint8_t *mask, size_t capacity, size_t *needed);
    uint32_t (*set_thread_affinity)(uint32_t cpu);
    uint32_t (*physical_memory)(uint64_t *total, uint64_t *available);
    uint32_t (*memory_limit)(uint64_t *limit);
    uint32_t (*virtual_limit)(uint64_t *limit);
    uint32_t (*cache_size)(size_t *bytes);
    uint32_t (*cpu_features)(uint64_t *first, uint64_t *second);
    uint32_t (*read_stats)(dotnet_pal_topology_stats *out, size_t size);
} dotnet_pal_topology_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_topology_ops ops; } dotnet_pal_host_topology;

/* Process lifetime services (append-only group). exit never returns. crash_dump
 * runs the given argument vector as a crash-dump utility that may inspect this
 * process and waits for it; UNSUPPORTED when the target has no such facility.
 * A failure message of at most error_capacity-1 bytes plus NUL may be written. */
#define DOTNET_PAL_CAP_PROCESS UINT64_C(16777216)
typedef struct { uint64_t debugger_ok, dump_ok, rejected_or_failed; } dotnet_pal_process_stats;
typedef struct {
    void (*exit)(int32_t code);
    uint32_t (*debugger_present)(uint32_t *out);
    uint32_t (*crash_dump)(const uint8_t *const *argv, size_t argc, uint8_t *error, size_t error_capacity);
    uint32_t (*read_stats)(dotnet_pal_process_stats *out, size_t size);
} dotnet_pal_process_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_process_ops ops; } dotnet_pal_host_process;

/* Executable image inspection (append-only group). unwind_info locates the
 * image containing address and its DWARF unwind tables (an eh_frame_hdr index
 * when the image has one, else eh_frame_hdr is 0 and eh_frame spans the table);
 * NOT_FOUND when no image contains the address. readable reports OK when the
 * range can be read, NOT_FOUND when it cannot, UNSUPPORTED when the target
 * cannot probe. build_id copies the image's build identifier; NOT_FOUND without one. */
#define DOTNET_PAL_CAP_IMAGE UINT64_C(33554432)
typedef struct {
    uintptr_t base;
    uintptr_t text_start; size_t text_length;
    uintptr_t eh_frame_hdr; size_t eh_frame_hdr_length;
    uintptr_t eh_frame; size_t eh_frame_length;
} dotnet_pal_unwind_info;
typedef struct { uint64_t unwind_ok, readable_ok, build_id_ok, rejected_or_failed; } dotnet_pal_image_stats;
typedef struct {
    uint32_t (*unwind_info)(uintptr_t address, dotnet_pal_unwind_info *out, size_t size);
    uint32_t (*readable)(uintptr_t address, size_t size);
    uint32_t (*build_id)(uintptr_t base, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*read_stats)(dotnet_pal_image_stats *out, size_t size);
} dotnet_pal_image_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_image_ops ops; } dotnet_pal_host_image;

/* Standard streams (append-only group): 0 = input, 1 = output, 2 = error output.
 * Not a file API: no positions, no other descriptors, no sockets. write reports
 * the bytes accepted (never zero with OK); read reports zero bytes with OK at
 * end of input. is_terminal says whether the stream is an interactive console. */
#define DOTNET_PAL_CAP_STREAMS UINT64_C(67108864)
typedef struct { uint64_t write_ok, read_ok, query_ok, rejected_or_failed; } dotnet_pal_streams_stats;
typedef struct {
    uint32_t (*write)(uint32_t stream, const uint8_t *data, size_t size, size_t *written);
    uint32_t (*read)(uint32_t stream, uint8_t *data, size_t capacity, size_t *read);
    uint32_t (*is_terminal)(uint32_t stream, uint32_t *out);
    uint32_t (*read_stats)(dotnet_pal_streams_stats *out, size_t size);
} dotnet_pal_streams_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_streams_ops ops; } dotnet_pal_host_streams;

/* Portable I/O statuses shared by the files and sockets groups (append-only).
 * A provider reports the condition; the consumer translates it to whatever its
 * own error vocabulary is (System.Native maps them to errno values). Groups
 * that predate these statuses never return them. */
#define DOTNET_PAL_ALREADY_EXISTS 9u
#define DOTNET_PAL_ACCESS_DENIED 10u
#define DOTNET_PAL_IS_DIRECTORY 11u
#define DOTNET_PAL_NOT_DIRECTORY 12u
#define DOTNET_PAL_NOT_EMPTY 13u
#define DOTNET_PAL_NO_SPACE 14u
#define DOTNET_PAL_WOULD_BLOCK 15u
#define DOTNET_PAL_BROKEN_PIPE 16u
#define DOTNET_PAL_CONNECTION_REFUSED 17u
#define DOTNET_PAL_CONNECTION_RESET 18u
#define DOTNET_PAL_CONNECTION_ABORTED 19u
#define DOTNET_PAL_NOT_CONNECTED 20u
#define DOTNET_PAL_ALREADY_CONNECTED 21u
#define DOTNET_PAL_ADDRESS_IN_USE 22u
#define DOTNET_PAL_ADDRESS_NOT_AVAILABLE 23u
#define DOTNET_PAL_NETWORK_UNREACHABLE 24u
#define DOTNET_PAL_HOST_UNREACHABLE 25u
#define DOTNET_PAL_IN_PROGRESS 26u
#define DOTNET_PAL_TOO_MANY_HANDLES 27u
#define DOTNET_PAL_NAME_TOO_LONG 28u
#define DOTNET_PAL_READ_ONLY 29u
#define DOTNET_PAL_CROSS_DEVICE 30u
#define DOTNET_PAL_MESSAGE_TOO_LARGE 31u

/* Files and directories (append-only group). Paths are byte borrows without
 * NUL, 1..DOTNET_PAL_MAX_NAME bytes, '/'-separated, passed to the provider
 * verbatim. File handles carry no position: read_at and write_at name the
 * offset, and a consumer that needs a cursor keeps it itself. read_at reports
 * zero bytes with OK at end of file; write_at never reports zero bytes with OK
 * and extends the file when the range lies past its end. A transfer the
 * handle was not opened for (write_at without WRITE, read_at without READ) is
 * ACCESS_DENIED, and so is set_size without WRITE. open flags: READ
 * and/or WRITE, CREATE (mode applies to a file it creates), EXCLUSIVE (with
 * CREATE: ALREADY_EXISTS when the path exists), TRUNCATE (with WRITE). rename
 * replaces an existing destination file. remove deletes a non-directory.
 * directory_read writes one entry name (at most DOTNET_PAL_MAX_ENTRY_NAME
 * bytes, no NUL, no '/', never "." or "..") into a buffer of at least
 * DOTNET_PAL_MAX_ENTRY_NAME bytes and reports NOT_FOUND after the last entry.
 * current_directory follows environment_get: needed includes the NUL.
 * Times are nanoseconds since the Unix epoch, 0 when the target has none.
 * Closing consumes a handle even when it reports an error.
 *
 * The callbacks from set_mode on are optional per call: a target without the
 * facility answers UNSUPPORTED and the capability stays whole. set_times
 * leaves a time given as DOTNET_PAL_TIME_KEEP unchanged. link makes a second
 * name for a file, symlink a symbolic link whose target text is stored as
 * given; read_link and real_path follow the text contract of
 * current_directory. lock is an advisory lock on the whole file, held by the
 * open handle and released by UNLOCK or close: SHARED locks coexist, an
 * EXCLUSIVE one excludes every other; with wait = 0 a lock that is not free is
 * WOULD_BLOCK. A handle that asks for a lock while it holds one converts it,
 * and a conversion that is refused may have given the held lock up (flock(2)
 * on Linux does, other targets keep it): a consumer that needs the old lock
 * takes it again. lock_range locks bytes [offset, offset + length) the same
 * way and never waits; a SHARED range needs a handle opened with READ, an
 * EXCLUSIVE one a handle opened with WRITE (ACCESS_DENIED otherwise). Whether
 * whole-file locks and range locks exclude each other is the target's. */
#define DOTNET_PAL_CAP_FILES UINT64_C(134217728)
#define DOTNET_PAL_FILE_READ 1u
#define DOTNET_PAL_FILE_WRITE 2u
#define DOTNET_PAL_FILE_CREATE 4u
#define DOTNET_PAL_FILE_EXCLUSIVE 8u
#define DOTNET_PAL_FILE_TRUNCATE 16u
#define DOTNET_PAL_NODE_FILE 1u
#define DOTNET_PAL_NODE_DIRECTORY 2u
#define DOTNET_PAL_NODE_SYMLINK 3u
#define DOTNET_PAL_NODE_OTHER 4u
#define DOTNET_PAL_MAX_ENTRY_NAME 255u
#define DOTNET_PAL_TIME_KEEP UINT64_MAX
#define DOTNET_PAL_LOCK_SHARED 1u
#define DOTNET_PAL_LOCK_EXCLUSIVE 2u
#define DOTNET_PAL_LOCK_UNLOCK 3u
typedef struct {
    uint32_t kind;      /* DOTNET_PAL_NODE_* */
    uint32_t mode;      /* permission bits 0..07777; 0 when the target has none */
    uint64_t size;
    uint64_t modified_ns, accessed_ns, changed_ns, created_ns;
    uint64_t identity;  /* distinguishes nodes of one device; 0 when the target has none */
    uint64_t device;
} dotnet_pal_file_status;
typedef struct {
    uint64_t open_ok, close_ok, read_ok, write_ok, size_ok, flush_ok, status_ok;
    uint64_t remove_ok, rename_ok, directory_ok, rejected_or_failed;
    uint64_t attribute_ok, link_ok, lock_ok;
} dotnet_pal_files_stats;
typedef struct {
    uint32_t (*open)(const uint8_t *path, size_t path_length, uint32_t flags, uint32_t mode, void **out);
    uint32_t (*close)(void *file);
    uint32_t (*read_at)(void *file, uint64_t offset, uint8_t *data, size_t capacity, size_t *read);
    uint32_t (*write_at)(void *file, uint64_t offset, const uint8_t *data, size_t size, size_t *written);
    uint32_t (*set_size)(void *file, uint64_t size);
    uint32_t (*flush)(void *file);
    uint32_t (*status)(void *file, dotnet_pal_file_status *out, size_t size);
    uint32_t (*path_status)(const uint8_t *path, size_t path_length, uint32_t follow_links, dotnet_pal_file_status *out, size_t size);
    uint32_t (*remove)(const uint8_t *path, size_t path_length);
    uint32_t (*rename)(const uint8_t *from, size_t from_length, const uint8_t *to, size_t to_length);
    uint32_t (*directory_create)(const uint8_t *path, size_t path_length, uint32_t mode);
    uint32_t (*directory_remove)(const uint8_t *path, size_t path_length);
    uint32_t (*directory_open)(const uint8_t *path, size_t path_length, void **out);
    uint32_t (*directory_read)(void *directory, uint8_t *name, size_t capacity, size_t *name_length, uint32_t *kind);
    uint32_t (*directory_close)(void *directory);
    uint32_t (*current_directory)(uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*read_stats)(dotnet_pal_files_stats *out, size_t size);
    uint32_t (*set_mode)(const uint8_t *path, size_t path_length, uint32_t mode);
    uint32_t (*set_file_mode)(void *file, uint32_t mode);
    uint32_t (*set_times)(const uint8_t *path, size_t path_length, uint32_t follow_links, uint64_t accessed_ns, uint64_t modified_ns);
    uint32_t (*set_file_times)(void *file, uint64_t accessed_ns, uint64_t modified_ns);
    uint32_t (*link)(const uint8_t *existing, size_t existing_length, const uint8_t *created, size_t created_length);
    uint32_t (*symlink)(const uint8_t *target, size_t target_length, const uint8_t *created, size_t created_length);
    uint32_t (*read_link)(const uint8_t *path, size_t path_length, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*real_path)(const uint8_t *path, size_t path_length, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*set_current_directory)(const uint8_t *path, size_t path_length);
    uint32_t (*lock)(void *file, uint32_t mode, uint32_t wait);
    uint32_t (*lock_range)(void *file, uint64_t offset, uint64_t length, uint32_t mode);
} dotnet_pal_files_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_files_ops ops; } dotnet_pal_host_files;

/* Internet sockets (append-only group): TCP streams and UDP datagrams over
 * IPv4 and IPv6. Addresses use the boundary's own layout, never a platform
 * sockaddr: port is in host order, an IPv4 address occupies address[0..4].
 * Sockets start blocking; set_blocking(socket, 0) makes accept, connect, send
 * and receive report WOULD_BLOCK (IN_PROGRESS for connect) instead of waiting.
 * receive reports zero bytes with OK when a stream peer has shut down and
 * truncates a datagram longer than the buffer, discarding the rest; send never
 * raises a signal and reports BROKEN_PIPE. to and from may be NULL. On a
 * blocking socket an expired RECEIVE_TIMEOUT or SEND_TIMEOUT is TIMEOUT, never
 * WOULD_BLOCK. An option the socket's protocol does not have is UNSUPPORTED.
 * poll is level-triggered: it fills triggered for every entry and reports how
 * many entries have any bit set; ERROR, and HANGUP once both directions are
 * down, are reported whether or not they were requested; a peer's half close
 * shows as READ where READ was requested, and a provider may add HANGUP to
 * it. It returns OK with ready = 0 on timeout. A poll may
 * name a wake channel (0..DOTNET_PAL_POLL_CHANNELS-1): wake(channel) makes the
 * poll in progress on that channel return early, or the next one when none is
 * in progress, and no other channel's. One poll at a time uses a channel; a
 * poll on DOTNET_PAL_NO_CHANNEL cannot be woken. Option values are integers:
 * flags are 0/1, sizes are bytes, timeouts are milliseconds (0 = none),
 * LINGER is 0 when off and seconds + 1 when on, ERROR reads and clears the
 * pending error as a status code, AVAILABLE is the bytes readable now,
 * KEEP_ALIVE_IDLE and KEEP_ALIVE_INTERVAL are seconds and KEEP_ALIVE_COUNT a
 * number of probes (each at least 1; the upper limits are the target's), HOPS
 * is the hop limit of unicast traffic (1..255) and MULTICAST_HOPS that of
 * multicast traffic (0..255; 0 keeps it on the host), MULTICAST_INTERFACE is
 * an interface index of the network group (0: the target's choice;
 * ADDRESS_NOT_AVAILABLE when no interface has it) and reads back as what was
 * set through the boundary.
 * resolve writes up to capacity addresses for a host name and reports
 * NOT_FOUND when the name has none and TIMEOUT when the answer could not be
 * obtained for now; family 0 asks for both families.
 * host_name follows environment_get: needed includes the NUL. */
#define DOTNET_PAL_CAP_SOCKETS UINT64_C(268435456)
#define DOTNET_PAL_FAMILY_IPV4 1u
#define DOTNET_PAL_FAMILY_IPV6 2u
#define DOTNET_PAL_SOCKET_STREAM 1u
#define DOTNET_PAL_SOCKET_DATAGRAM 2u
#define DOTNET_PAL_SHUTDOWN_READ 1u
#define DOTNET_PAL_SHUTDOWN_WRITE 2u
#define DOTNET_PAL_SHUTDOWN_BOTH 3u
#define DOTNET_PAL_RECEIVE_PEEK 1u
#define DOTNET_PAL_POLL_READ 1u
#define DOTNET_PAL_POLL_WRITE 2u
#define DOTNET_PAL_POLL_ERROR 4u
#define DOTNET_PAL_POLL_HANGUP 8u
#define DOTNET_PAL_MAX_POLL 4096u
#define DOTNET_PAL_POLL_CHANNELS 64u
#define DOTNET_PAL_NO_CHANNEL UINT32_MAX
#define DOTNET_PAL_SOCKET_REUSE_ADDRESS 1u
#define DOTNET_PAL_SOCKET_NO_DELAY 2u
#define DOTNET_PAL_SOCKET_KEEP_ALIVE 3u
#define DOTNET_PAL_SOCKET_BROADCAST 4u
#define DOTNET_PAL_SOCKET_RECEIVE_BUFFER 5u
#define DOTNET_PAL_SOCKET_SEND_BUFFER 6u
#define DOTNET_PAL_SOCKET_IPV6_ONLY 7u
#define DOTNET_PAL_SOCKET_LINGER 8u
#define DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT 9u
#define DOTNET_PAL_SOCKET_SEND_TIMEOUT 10u
#define DOTNET_PAL_SOCKET_ERROR 11u
#define DOTNET_PAL_SOCKET_AVAILABLE 12u
#define DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE 13u
#define DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL 14u
#define DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT 15u
#define DOTNET_PAL_SOCKET_HOPS 16u
#define DOTNET_PAL_SOCKET_MULTICAST_HOPS 17u
#define DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK 18u
#define DOTNET_PAL_SOCKET_MULTICAST_INTERFACE 19u
typedef struct { uint16_t family; uint16_t port; uint32_t scope; uint8_t address[16]; } dotnet_pal_socket_address;
typedef struct { void *socket; uint32_t requested; uint32_t triggered; } dotnet_pal_poll_entry;
typedef struct {
    uint64_t create_ok, close_ok, bind_ok, listen_ok, accept_ok, connect_ok, send_ok, receive_ok;
    uint64_t shutdown_ok, address_ok, option_ok, poll_ok, wake_ok, resolve_ok, rejected_or_failed;
} dotnet_pal_sockets_stats;
typedef struct {
    uint32_t (*create)(uint32_t family, uint32_t kind, void **out);
    uint32_t (*close)(void *socket);
    uint32_t (*bind)(void *socket, const dotnet_pal_socket_address *address);
    uint32_t (*listen)(void *socket, uint32_t backlog);
    uint32_t (*accept)(void *socket, void **out, dotnet_pal_socket_address *peer);
    uint32_t (*connect)(void *socket, const dotnet_pal_socket_address *address);
    uint32_t (*send)(void *socket, const uint8_t *data, size_t size, const dotnet_pal_socket_address *to, size_t *sent);
    uint32_t (*receive)(void *socket, uint8_t *data, size_t capacity, uint32_t flags, dotnet_pal_socket_address *from, size_t *received);
    uint32_t (*shutdown)(void *socket, uint32_t how);
    uint32_t (*local_address)(void *socket, dotnet_pal_socket_address *out);
    uint32_t (*peer_address)(void *socket, dotnet_pal_socket_address *out);
    uint32_t (*set_blocking)(void *socket, uint32_t blocking);
    uint32_t (*get_option)(void *socket, uint32_t option, uint64_t *value);
    uint32_t (*set_option)(void *socket, uint32_t option, uint64_t value);
    uint32_t (*poll)(dotnet_pal_poll_entry *entries, size_t count, uint64_t timeout_ns, uint32_t channel, size_t *ready);
    uint32_t (*wake)(uint32_t channel);
    uint32_t (*resolve)(const uint8_t *name, size_t name_length, uint32_t family, dotnet_pal_socket_address *out, size_t capacity, size_t *count);
    uint32_t (*host_name)(uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*read_stats)(dotnet_pal_sockets_stats *out, size_t size);
} dotnet_pal_sockets_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_sockets_ops ops; } dotnet_pal_host_sockets;

/* Synchronous CPU faults without POSIX signals (append-only group). The port
 * reports a fault it took (a trap vector, an exception port, a signal it owns)
 * with the interrupted registers in the boundary's frame layout for the
 * architecture; frame_tag and frame_size name that layout, and a consumer
 * checks both before install. The handler runs in the faulting thread on the
 * port's trap path: it may not block, allocate, unwind or call the boundary.
 * It returns DOTNET_PAL_FAULT_RESUME after editing the frame (the port then
 * continues from the edited registers) or DOTNET_PAL_FAULT_UNHANDLED (the port
 * ends the run the way it would without a handler). install is once per
 * process: BUSY afterwards. address is meaningful for ACCESS and ALIGNMENT and
 * zero otherwise. A port reports the kinds it can recognize, and only faults
 * the CPU raised: an event that merely looks like one (a signal somebody sent)
 * is not a fault. A fault taken while the handler runs on the same thread ends
 * the run; telling that from a concurrent fault on another thread is the
 * port's job. The port may keep its own state below the interrupted stack
 * pointer while the handler runs: a handler that lowers sp writes nothing
 * there, except inside a red zone the target's ABI makes the port skip (128
 * bytes on x86-64 System V and in Apple's AArch64 ABI, none in the standard
 * AArch64 one). This group is not the
 * native context group: it neither delivers signals nor interrupts another
 * thread. */
#define DOTNET_PAL_CAP_FAULTS UINT64_C(536870912)
#define DOTNET_PAL_FAULT_ACCESS 1u
#define DOTNET_PAL_FAULT_ALIGNMENT 2u
#define DOTNET_PAL_FAULT_INTEGER_DIVIDE 3u
#define DOTNET_PAL_FAULT_INTEGER_OVERFLOW 4u
#define DOTNET_PAL_FAULT_FLOATING_POINT 5u
#define DOTNET_PAL_FAULT_ILLEGAL_INSTRUCTION 6u
#define DOTNET_PAL_FAULT_BREAKPOINT 7u
#define DOTNET_PAL_FAULT_STACK_OVERFLOW 8u
#define DOTNET_PAL_FAULT_UNHANDLED 0u
#define DOTNET_PAL_FAULT_RESUME 1u
#define DOTNET_PAL_FRAME_ARM64 UINT64_C(0x4652414d45000001)
#define DOTNET_PAL_FRAME_X64 UINT64_C(0x4652414d45000002)
typedef struct { uint64_t x[31]; uint64_t sp; uint64_t pc; uint64_t pstate; } dotnet_pal_fault_frame_arm64;
/* Registers in instruction-encoding order: rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15. */
typedef struct { uint64_t registers[16]; uint64_t rip; uint64_t rflags; } dotnet_pal_fault_frame_x64;
typedef uint32_t (*dotnet_pal_fault_handler)(uint32_t kind, uintptr_t address, void *frame, size_t frame_size, void *data);
typedef struct { uint64_t installs, delivered, resumed, unhandled, rejected; } dotnet_pal_faults_stats;
typedef struct {
    uint64_t (*frame_tag)(void);
    size_t (*frame_size)(void);
    uint32_t (*install)(dotnet_pal_fault_handler handler, void *data);
    uint32_t (*read_stats)(dotnet_pal_faults_stats *out, size_t size);
} dotnet_pal_faults_ops;
/* A host delivers a fault by calling the deliver callback it was given. */
typedef struct {
    uint64_t (*frame_tag)(void);
    size_t (*frame_size)(void);
    uint32_t (*enable)(dotnet_pal_fault_handler deliver);
} dotnet_pal_host_faults_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_host_faults_ops ops; } dotnet_pal_host_faults;

/* Facts about the process and the machine that the BCL surfaces through
 * Environment, RuntimeInformation and Process.GetCurrentProcess (append-only
 * group). environment_entry enumerates the environment as NAME=value texts by
 * index (each at most DOTNET_PAL_MAX_ENVIRONMENT_ENTRY bytes with its NUL) and
 * reports NOT_FOUND past the last one; like environment_get it must not run in
 * parallel with a change of the environment. text answers one
 * question per selector with the text contract of environment_get (needed
 * includes the NUL): the path of the running executable, the operating
 * system's name, release and version texts, the name and the home directory
 * of the user the process runs as. process_times is the CPU time this process
 * has consumed, uptime_ns the time since the machine started, user_ids the
 * numeric identity of the user and its primary group. Every question is
 * optional per call: a target that cannot answer reports UNSUPPORTED. */
#define DOTNET_PAL_CAP_SYSTEM UINT64_C(1073741824)
#define DOTNET_PAL_TEXT_EXECUTABLE_PATH 1u
#define DOTNET_PAL_TEXT_OS_NAME 2u
#define DOTNET_PAL_TEXT_OS_RELEASE 3u
#define DOTNET_PAL_TEXT_OS_VERSION 4u
#define DOTNET_PAL_TEXT_USER_NAME 5u
#define DOTNET_PAL_TEXT_HOME_DIRECTORY 6u
#define DOTNET_PAL_MAX_ENVIRONMENT_ENTRY 131072u
typedef struct { uint64_t environment_ok, text_ok, times_ok, identity_ok, rejected_or_failed; } dotnet_pal_system_stats;
typedef struct {
    uint32_t (*environment_entry)(size_t index, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*text)(uint32_t what, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*process_times)(uint64_t *user_ns, uint64_t *kernel_ns);
    uint32_t (*uptime_ns)(uint64_t *out);
    uint32_t (*user_ids)(uint32_t *user, uint32_t *group);
    uint32_t (*read_stats)(dotnet_pal_system_stats *out, size_t size);
} dotnet_pal_system_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_system_ops ops; } dotnet_pal_host_system;

/* Requests that reach the process from outside it (append-only group): the
 * interrupt key, a request to quit or terminate, a lost controlling terminal,
 * a resumed process, a resized terminal window and the job-control stops. The
 * port owns the mechanism (signals, a console control handler, a button). The
 * consumer installs one handler, once (BUSY afterwards), and enables the kinds
 * it wants; the port reports an enabled kind by calling the handler from a
 * thread of its own, never from an interrupt or signal context, so the handler
 * may use the whole boundary. A kind that is not enabled keeps the action it
 * had before. default_action performs that action for a kind the consumer saw
 * and chose not to handle (for TERMINATE, normally: end the process the way
 * the target would have); it may not return. A kind the target does not have is
 * UNSUPPORTED from enable. */
#define DOTNET_PAL_CAP_NOTIFICATIONS UINT64_C(2147483648)
#define DOTNET_PAL_NOTIFY_INTERRUPT 1u
#define DOTNET_PAL_NOTIFY_QUIT 2u
#define DOTNET_PAL_NOTIFY_TERMINATE 3u
#define DOTNET_PAL_NOTIFY_HANGUP 4u
#define DOTNET_PAL_NOTIFY_CONTINUE 5u
#define DOTNET_PAL_NOTIFY_WINDOW_CHANGE 6u
#define DOTNET_PAL_NOTIFY_STOP_INPUT 7u
#define DOTNET_PAL_NOTIFY_STOP_OUTPUT 8u
#define DOTNET_PAL_NOTIFY_STOP 9u
typedef void (*dotnet_pal_notification_handler)(uint32_t kind, void *data);
typedef struct { uint64_t installs, enabled, disabled, delivered, dropped, rejected; } dotnet_pal_notifications_stats;
typedef struct {
    uint32_t (*install)(dotnet_pal_notification_handler handler, void *data);
    uint32_t (*enable)(uint32_t kind);
    uint32_t (*disable)(uint32_t kind);
    uint32_t (*default_action)(uint32_t kind);
    uint32_t (*read_stats)(dotnet_pal_notifications_stats *out, size_t size);
} dotnet_pal_notifications_ops;
/* A host reports a notification by calling the deliver callback it was given. The callback answers 1 when
 * the consumer took the report and 0 when nobody wanted it (the kind was disabled meanwhile): the action the
 * kind had before is then the host's to take. */
typedef struct {
    uint32_t (*start)(uint32_t (*deliver)(uint32_t kind));
    uint32_t (*enable)(uint32_t kind);
    uint32_t (*disable)(uint32_t kind);
    uint32_t (*default_action)(uint32_t kind);
} dotnet_pal_host_notifications_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_host_notifications_ops ops; } dotnet_pal_host_notifications;

/* Child processes (append-only group). spawn starts a program: arguments and
 * environment are arrays of NUL-terminated texts (argument 0 is the program's
 * own name; a NULL environment inherits the parent's, and every entry is
 * NAME=value with a name that is not empty), directory is the child's working
 * directory or NULL, and pipes says which of the child's
 * standard streams (DOTNET_PAL_PIPE_INPUT/OUTPUT/ERROR) are pipes to the
 * parent instead of the parent's own streams. The result holds the process
 * handle, its identifier and one pipe handle per requested stream (the parent
 * writes to the child's input and reads its output and error). The program
 * is a path, never searched for; a relative one is resolved in the child's
 * working directory. The child starts with no signal blocked and every signal
 * at its default action, and inherits none of the boundary's handles but its
 * standard streams.
 * wait blocks
 * until the child has ended, or for timeout_ns (TIMEOUT), and reports the exit
 * code, or 128 plus the signal number for a child a signal ended; it may be
 * called again after it has reported. terminate asks the child to end
 * (forceful = 0) or ends it (forceful = 1), and reports NOT_FOUND for a child
 * it knows to have ended. release gives the handle back and does not end the
 * child; the exit of a child released while it runs is reported to nobody.
 * Pipes block: pipe_read reports zero bytes with OK once the other end is
 * closed; pipe_write reports BROKEN_PIPE then and never raises a signal. */
#define DOTNET_PAL_CAP_PROCESSES UINT64_C(4294967296)
#define DOTNET_PAL_PIPE_INPUT 1u
#define DOTNET_PAL_PIPE_OUTPUT 2u
#define DOTNET_PAL_PIPE_ERROR 4u
#define DOTNET_PAL_MAX_ARGUMENTS 65536u
typedef struct { void *process; uint64_t id; void *input; void *output; void *error; } dotnet_pal_spawned;
typedef struct { uint64_t spawn_ok, wait_ok, terminate_ok, release_ok, pipe_read_ok, pipe_write_ok, pipe_close_ok, rejected_or_failed; } dotnet_pal_processes_stats;
typedef struct {
    uint32_t (*spawn)(const uint8_t *program, size_t program_length, const uint8_t *const *arguments, size_t argument_count,
                      const uint8_t *const *environment, size_t environment_count, const uint8_t *directory, size_t directory_length,
                      uint32_t pipes, dotnet_pal_spawned *out, size_t size);
    uint32_t (*wait)(void *process, uint64_t timeout_ns, int32_t *exit_code);
    uint32_t (*terminate)(void *process, uint32_t forceful);
    uint32_t (*release)(void *process);
    uint32_t (*pipe_read)(void *pipe, uint8_t *data, size_t capacity, size_t *read);
    uint32_t (*pipe_write)(void *pipe, const uint8_t *data, size_t size, size_t *written);
    uint32_t (*pipe_close)(void *pipe);
    uint32_t (*read_stats)(dotnet_pal_processes_stats *out, size_t size);
} dotnet_pal_processes_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_processes_ops ops; } dotnet_pal_host_processes;

/* The interactive terminal behind the standard streams (append-only group).
 * window_size is the size of the terminal a stream is connected to, in
 * character cells. set_input_mode switches input between line mode (the
 * terminal edits and echoes a line and hands it over at Enter) and raw mode
 * (every byte is delivered as typed, without echo: a read returns once
 * min_bytes have arrived or timeout_ds tenths of a second have passed);
 * interrupt_as_input delivers the interrupt key as a byte instead of as a
 * notification. input_ready says whether a read would return now.
 * control_character is the byte the terminal uses for an editing function, or
 * NOT_FOUND when it has none. A stream that is no terminal is NOT_FOUND, and
 * a terminal that has no size (nobody set one) is UNSUPPORTED for
 * window_size. A process in the background of its terminal is subject to the
 * target's job control when it changes the mode: on a POSIX target it is
 * stopped until it is in the foreground. */
#define DOTNET_PAL_CAP_TERMINAL UINT64_C(8589934592)
#define DOTNET_PAL_CONTROL_ERASE 1u
#define DOTNET_PAL_CONTROL_END_OF_LINE 2u
#define DOTNET_PAL_CONTROL_END_OF_LINE_2 3u
#define DOTNET_PAL_CONTROL_END_OF_FILE 4u
typedef struct { uint64_t size_ok, mode_ok, ready_ok, control_ok, rejected_or_failed; } dotnet_pal_terminal_stats;
typedef struct {
    uint32_t (*window_size)(uint32_t stream, uint32_t *columns, uint32_t *rows);
    uint32_t (*set_input_mode)(uint32_t raw, uint32_t min_bytes, uint32_t timeout_ds, uint32_t interrupt_as_input);
    uint32_t (*input_ready)(uint32_t *ready);
    uint32_t (*control_character)(uint32_t which, uint32_t *value);
    uint32_t (*read_stats)(dotnet_pal_terminal_stats *out, size_t size);
} dotnet_pal_terminal_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_terminal_ops ops; } dotnet_pal_host_terminal;

/* Changes to files and directories, queued for a reader (append-only group).
 * A watcher is a queue of events. add watches one directory or file for the
 * requested events and returns an id that is unique among the watcher's live
 * watches; a node the watcher already watches returns its id and takes the
 * new events. A provider reports the kinds it can observe and answers
 * UNSUPPORTED to a request it can observe nothing of.
 * An event names the watch, what happened and, for a watched directory, the
 * entry it happened to (at most 255 bytes; empty when it happened to the
 * watched node itself). DIRECTORY marks an event whose subject is a directory.
 * A rename is MOVED_FROM and MOVED_TO with the same non-zero cookie, each
 * reported to the watch of its directory; a provider that cannot pair them
 * reports DELETE and CREATE. remove ends a watch and queues REMOVED for it, as
 * does the loss of the watched node; in which order that REMOVED and the
 * DELETE its parent's watch reports arrive, and whether a watch follows a
 * directory that is renamed or ends, is the target's. OVERFLOW (watch 0) says
 * events were lost, and a REMOVED may be among them.
 * read takes one event, waits up to timeout_ns (UINT64_MAX: without limit) and
 * answers TIMEOUT when none came. One thread reads at a time; add and remove
 * may run beside a read that waits, which is how a consumer ends that wait.
 * close needs the reader gone. NO_SPACE is the target's limit of watchers or
 * watches. */
#define DOTNET_PAL_CAP_WATCHES UINT64_C(17179869184)
#define DOTNET_PAL_WATCH_ACCESS 1u
#define DOTNET_PAL_WATCH_MODIFY 2u
#define DOTNET_PAL_WATCH_ATTRIBUTES 4u
#define DOTNET_PAL_WATCH_MOVED_FROM 8u
#define DOTNET_PAL_WATCH_MOVED_TO 16u
#define DOTNET_PAL_WATCH_CREATE 32u
#define DOTNET_PAL_WATCH_DELETE 64u
/* Reported, never requested. */
#define DOTNET_PAL_WATCH_OVERFLOW 128u
#define DOTNET_PAL_WATCH_REMOVED 256u
#define DOTNET_PAL_WATCH_DIRECTORY 512u
/* Requested with the events: the path must be a directory (NOT_DIRECTORY
 * otherwise); a symbolic link at the end of the path is watched itself. */
#define DOTNET_PAL_WATCH_ONLY_DIRECTORY 1024u
#define DOTNET_PAL_WATCH_NO_FOLLOW 2048u
typedef struct { uint32_t watch, events, cookie, name_length; uint8_t name[256]; } dotnet_pal_watch_event;
typedef struct { uint64_t open_ok, close_ok, add_ok, remove_ok, read_ok, rejected_or_failed; } dotnet_pal_watches_stats;
typedef struct {
    uint32_t (*open)(void **watcher);
    uint32_t (*close)(void *watcher);
    uint32_t (*add)(void *watcher, const uint8_t *path, size_t path_length, uint32_t events, uint32_t *watch);
    uint32_t (*remove)(void *watcher, uint32_t watch);
    uint32_t (*read)(void *watcher, uint64_t timeout_ns, dotnet_pal_watch_event *event, size_t event_size);
    uint32_t (*read_stats)(dotnet_pal_watches_stats *out, size_t size);
} dotnet_pal_watches_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_watches_ops ops; } dotnet_pal_host_watches;

/* Files mapped into memory (append-only group; it takes the handles of the
 * files group, so one provider implements both). map makes bytes
 * [offset, offset + length) of an open file accessible at the address it
 * returns; offset is a multiple of the page size. access is READ, WRITE and
 * EXECUTE as in the memory group. Writes through a SHARED mapping reach the
 * file and every other SHARED mapping of it; writes through a PRIVATE one stay
 * in the mapping. Every mapping needs a handle opened with READ, a SHARED one
 * with WRITE access a handle opened with WRITE as well (ACCESS_DENIED
 * otherwise). The mapping outlives the handle. It may reach past the end of
 * the file up to the end of the page that holds the last byte; those bytes
 * read as zero and what is written to them is lost. What a mapping that
 * reaches further does is the target's: a POSIX target grants it and faults
 * when a page wholly past the end is touched. sync returns once the
 * changes made through a SHARED mapping are in the file. unmap and sync take
 * exactly what one map returned. */
#define DOTNET_PAL_CAP_MAPPINGS UINT64_C(34359738368)
#define DOTNET_PAL_MAP_SHARED 1u
#define DOTNET_PAL_MAP_PRIVATE 2u
typedef struct { uint64_t map_ok, unmap_ok, sync_ok, rejected_or_failed; } dotnet_pal_mappings_stats;
typedef struct {
    uint32_t (*map)(void *file, uint64_t offset, size_t length, uint32_t access, uint32_t mode, void **address);
    uint32_t (*unmap)(void *address, size_t length);
    uint32_t (*sync)(void *address, size_t length);
    uint32_t (*read_stats)(dotnet_pal_mappings_stats *out, size_t size);
} dotnet_pal_mappings_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_mappings_ops ops; } dotnet_pal_host_mappings;

/* Mounted volumes (append-only group). entry enumerates the mount points as
 * paths and reports NOT_FOUND past the last; it follows environment_get.
 * status answers for the volume that holds a path: its capacity, its free
 * space, the part of that this process may use, and the name of its format
 * ("ext4", "tmpfs"; empty when the target has no name for it). */
#define DOTNET_PAL_CAP_VOLUMES UINT64_C(68719476736)
typedef struct { uint64_t total_bytes, free_bytes, available_bytes; uint8_t format[32]; } dotnet_pal_volume_status;
typedef struct { uint64_t entry_ok, status_ok, rejected_or_failed; } dotnet_pal_volumes_stats;
typedef struct {
    uint32_t (*entry)(size_t index, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*status)(const uint8_t *path, size_t path_length, dotnet_pal_volume_status *out, size_t out_size);
    uint32_t (*read_stats)(dotnet_pal_volumes_stats *out, size_t size);
} dotnet_pal_volumes_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_volumes_ops ops; } dotnet_pal_host_volumes;

/* Network interfaces, reverse lookup and multicast membership (append-only
 * group; membership takes the handles of the sockets group). interface_entry
 * and address_entry enumerate by index and report NOT_FOUND past the last; a
 * provider may answer from a snapshot it takes at index 0. An interface has
 * the target's index (never 0), a name, a kind, a link state, its MTU and
 * speed (0 when unknown) and a hardware address of up to 8 bytes. An address
 * entry is one IPv4 or IPv6 address with its prefix length and the index of
 * its interface; the scope of a link-local IPv6 address is that index.
 * reverse_lookup writes the host name of an address and follows
 * environment_get; NOT_FOUND when it has none, TIMEOUT when the answer could
 * not be obtained for now. membership joins (1) or leaves (0) a multicast
 * group on a datagram socket of the group's family; interface 0 lets the
 * target choose. Joining twice is ADDRESS_IN_USE, leaving a group the socket
 * is no member of ADDRESS_NOT_AVAILABLE, an interface that does not exist
 * NOT_FOUND; with interface 0 a target that has no route for the group
 * answers NOT_FOUND or ADDRESS_NOT_AVAILABLE. */
#define DOTNET_PAL_CAP_NETWORK UINT64_C(137438953472)
#define DOTNET_PAL_INTERFACE_UNKNOWN 0u
#define DOTNET_PAL_INTERFACE_ETHERNET 1u
#define DOTNET_PAL_INTERFACE_LOOPBACK 2u
#define DOTNET_PAL_INTERFACE_WIRELESS 3u
#define DOTNET_PAL_INTERFACE_POINT_TO_POINT 4u
#define DOTNET_PAL_INTERFACE_TUNNEL 5u
#define DOTNET_PAL_LINK_UNKNOWN 0u
#define DOTNET_PAL_LINK_UP 1u
#define DOTNET_PAL_LINK_DOWN 2u
#define DOTNET_PAL_INTERFACE_MULTICAST 1u
typedef struct {
    uint32_t index, kind, state, flags, mtu, hardware_address_length;
    uint64_t speed_bps;
    uint8_t hardware_address[8];
    uint8_t name[64];
} dotnet_pal_network_interface;
typedef struct { uint32_t interface_index, prefix_length; dotnet_pal_socket_address address; } dotnet_pal_network_address;
typedef struct { uint64_t interface_ok, address_ok, lookup_ok, membership_ok, rejected_or_failed; } dotnet_pal_network_stats;
typedef struct {
    uint32_t (*interface_entry)(size_t index, dotnet_pal_network_interface *out, size_t out_size);
    uint32_t (*address_entry)(size_t index, dotnet_pal_network_address *out, size_t out_size);
    uint32_t (*reverse_lookup)(const dotnet_pal_socket_address *address, uint8_t *out, size_t capacity, size_t *needed);
    uint32_t (*membership)(void *socket, const dotnet_pal_socket_address *group, uint32_t interface_index, uint32_t join);
    uint32_t (*read_stats)(dotnet_pal_network_stats *out, size_t size);
} dotnet_pal_network_ops;
typedef struct { dotnet_pal_header header; dotnet_pal_network_ops ops; } dotnet_pal_host_network;

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
    dotnet_pal_topology_ops topology;
    dotnet_pal_process_ops process;
    dotnet_pal_image_ops image;
    dotnet_pal_streams_ops streams;
    dotnet_pal_files_ops files;
    dotnet_pal_sockets_ops sockets;
    dotnet_pal_faults_ops faults;
    dotnet_pal_system_ops system;
    dotnet_pal_notifications_ops notifications;
    dotnet_pal_processes_ops processes;
    dotnet_pal_terminal_ops terminal;
    dotnet_pal_watches_ops watches;
    dotnet_pal_mappings_ops mappings;
    dotnet_pal_volumes_ops volumes;
    dotnet_pal_network_ops network;
} dotnet_pal_api;

/* Use size checks BEFORE reading a capability group from a foreign table.
 * Each size marks the END of that group, not sizeof a future extended API. */
#define DOTNET_PAL_NETWORK_API_SIZE (offsetof(dotnet_pal_api, network) + sizeof(dotnet_pal_network_ops))
#define DOTNET_PAL_VOLUMES_API_SIZE (offsetof(dotnet_pal_api, volumes) + sizeof(dotnet_pal_volumes_ops))
#define DOTNET_PAL_MAPPINGS_API_SIZE (offsetof(dotnet_pal_api, mappings) + sizeof(dotnet_pal_mappings_ops))
#define DOTNET_PAL_WATCHES_API_SIZE (offsetof(dotnet_pal_api, watches) + sizeof(dotnet_pal_watches_ops))
#define DOTNET_PAL_TERMINAL_API_SIZE (offsetof(dotnet_pal_api, terminal) + sizeof(dotnet_pal_terminal_ops))
#define DOTNET_PAL_PROCESSES_API_SIZE (offsetof(dotnet_pal_api, processes) + sizeof(dotnet_pal_processes_ops))
#define DOTNET_PAL_NOTIFICATIONS_API_SIZE (offsetof(dotnet_pal_api, notifications) + sizeof(dotnet_pal_notifications_ops))
#define DOTNET_PAL_SYSTEM_API_SIZE (offsetof(dotnet_pal_api, system) + sizeof(dotnet_pal_system_ops))
#define DOTNET_PAL_FAULTS_API_SIZE (offsetof(dotnet_pal_api, faults) + sizeof(dotnet_pal_faults_ops))
#define DOTNET_PAL_SOCKETS_API_SIZE (offsetof(dotnet_pal_api, sockets) + sizeof(dotnet_pal_sockets_ops))
#define DOTNET_PAL_FILES_API_SIZE (offsetof(dotnet_pal_api, files) + sizeof(dotnet_pal_files_ops))
#define DOTNET_PAL_STREAMS_API_SIZE (offsetof(dotnet_pal_api, streams) + sizeof(dotnet_pal_streams_ops))
#define DOTNET_PAL_IMAGE_API_SIZE (offsetof(dotnet_pal_api, image) + sizeof(dotnet_pal_image_ops))
#define DOTNET_PAL_PROCESS_API_SIZE (offsetof(dotnet_pal_api, process) + sizeof(dotnet_pal_process_ops))
#define DOTNET_PAL_TOPOLOGY_API_SIZE (offsetof(dotnet_pal_api, topology) + sizeof(dotnet_pal_topology_ops))
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
 * browser-heap defines both hooks itself from memory.grow; an embedding that
 * links another allocator into the same memory must not select browser-heap.
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
/* Required only by host-topology, host-process and host-image respectively. */
const dotnet_pal_host_topology *dotnet_pal_host_topology_v2(void);
const dotnet_pal_host_process *dotnet_pal_host_process_v2(void);
const dotnet_pal_host_image *dotnet_pal_host_image_v2(void);
/* Required only by host-streams. */
const dotnet_pal_host_streams *dotnet_pal_host_streams_v2(void);
/* Required only by host-files, host-sockets and host-faults respectively. */
const dotnet_pal_host_files *dotnet_pal_host_files_v2(void);
const dotnet_pal_host_sockets *dotnet_pal_host_sockets_v2(void);
const dotnet_pal_host_faults *dotnet_pal_host_faults_v2(void);
/* Required only by host-system, host-notifications, host-processes and host-terminal respectively. */
const dotnet_pal_host_system *dotnet_pal_host_system_v2(void);
const dotnet_pal_host_notifications *dotnet_pal_host_notifications_v2(void);
const dotnet_pal_host_processes *dotnet_pal_host_processes_v2(void);
const dotnet_pal_host_terminal *dotnet_pal_host_terminal_v2(void);
/* Required only by host-watches, host-mappings, host-volumes and host-network respectively. */
const dotnet_pal_host_watches *dotnet_pal_host_watches_v2(void);
const dotnet_pal_host_mappings *dotnet_pal_host_mappings_v2(void);
const dotnet_pal_host_volumes *dotnet_pal_host_volumes_v2(void);
const dotnet_pal_host_network *dotnet_pal_host_network_v2(void);
#if defined(__cplusplus)
[[noreturn]] void dotnet_pal_host_abort(void);
#else
_Noreturn void dotnet_pal_host_abort(void);
#endif
#ifdef __cplusplus
}
#endif
#endif
