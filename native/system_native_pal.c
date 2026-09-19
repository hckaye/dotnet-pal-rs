/* System.Native over the boundary: the BCL's native layer implemented with the
 * negotiated dotnet_pal table instead of OS calls. This unit carries the native
 * heap, threads and monitors, clocks, entropy, environment
 * lookup, error-code translation and diagnostics. system_native_io.c carries
 * descriptors, streams, files and directories, system_native_net.c sockets,
 * system_native_sys.c system facts, notifications, the terminal and module loading,
 * system_native_proc.c child processes. What the boundary does not carry (other
 * users, groups, sessions, priorities) reports ENOTSUP, ENOENT, zero or null, so
 * managed code sees an honest failure rather than a fake success. */
#include "system_native_internal.h"

static const dotnet_pal_api *api(void) { return sn_api(); }
static bool has(const dotnet_pal_api *a, size_t size, uint64_t capability) { return sn_has(a, size, capability); }
static int fail(int error) { return sn_fail(error); }

/* ---- errno translation (pure) ------------------------------------------- */
PALEXPORT int32_t SystemNative_ConvertErrorPlatformToPal(int32_t e) {
    switch (e) {
        case 0: return Error_SUCCESS;
        case E2BIG: return Error_E2BIG; case EACCES: return Error_EACCES; case EADDRINUSE: return Error_EADDRINUSE;
        case EADDRNOTAVAIL: return Error_EADDRNOTAVAIL; case EAFNOSUPPORT: return Error_EAFNOSUPPORT; case EAGAIN: return Error_EAGAIN;
        case EALREADY: return Error_EALREADY; case EBADF: return Error_EBADF; case EBADMSG: return Error_EBADMSG; case EBUSY: return Error_EBUSY;
        case ECANCELED: return Error_ECANCELED; case ECHILD: return Error_ECHILD; case ECONNABORTED: return Error_ECONNABORTED;
        case ECONNREFUSED: return Error_ECONNREFUSED; case ECONNRESET: return Error_ECONNRESET; case EDEADLK: return Error_EDEADLK;
        case EDESTADDRREQ: return Error_EDESTADDRREQ; case EDOM: return Error_EDOM; case EDQUOT: return Error_EDQUOT; case EEXIST: return Error_EEXIST;
        case EFAULT: return Error_EFAULT; case EFBIG: return Error_EFBIG; case EHOSTUNREACH: return Error_EHOSTUNREACH; case EIDRM: return Error_EIDRM;
        case EILSEQ: return Error_EILSEQ; case EINPROGRESS: return Error_EINPROGRESS; case EINTR: return Error_EINTR; case EINVAL: return Error_EINVAL;
        case EIO: return Error_EIO; case EISCONN: return Error_EISCONN; case EISDIR: return Error_EISDIR; case ELOOP: return Error_ELOOP;
        case EMFILE: return Error_EMFILE; case EMLINK: return Error_EMLINK; case EMSGSIZE: return Error_EMSGSIZE; case EMULTIHOP: return Error_EMULTIHOP;
        case ENAMETOOLONG: return Error_ENAMETOOLONG; case ENETDOWN: return Error_ENETDOWN; case ENETRESET: return Error_ENETRESET;
        case ENETUNREACH: return Error_ENETUNREACH; case ENFILE: return Error_ENFILE; case ENOBUFS: return Error_ENOBUFS; case ENODEV: return Error_ENODEV;
        case ENOENT: return Error_ENOENT; case ENOEXEC: return Error_ENOEXEC; case ENOLCK: return Error_ENOLCK; case ENOLINK: return Error_ENOLINK;
        case ENOMEM: return Error_ENOMEM; case ENOMSG: return Error_ENOMSG; case ENOPROTOOPT: return Error_ENOPROTOOPT; case ENOSPC: return Error_ENOSPC;
        case ENOSYS: return Error_ENOSYS; case ENOTCONN: return Error_ENOTCONN; case ENOTDIR: return Error_ENOTDIR; case ENOTEMPTY: return Error_ENOTEMPTY;
        case ENOTRECOVERABLE: return Error_ENOTRECOVERABLE; case ENOTSOCK: return Error_ENOTSOCK; case ENOTSUP: return Error_ENOTSUP;
        case ENOTTY: return Error_ENOTTY; case ENXIO: return Error_ENXIO; case EOVERFLOW: return Error_EOVERFLOW; case EOWNERDEAD: return Error_EOWNERDEAD;
        case EPERM: return Error_EPERM; case EPIPE: return Error_EPIPE; case EPROTO: return Error_EPROTO; case EPROTONOSUPPORT: return Error_EPROTONOSUPPORT;
        case EPROTOTYPE: return Error_EPROTOTYPE; case ERANGE: return Error_ERANGE; case EROFS: return Error_EROFS; case ESPIPE: return Error_ESPIPE;
        case ESRCH: return Error_ESRCH; case ESTALE: return Error_ESTALE; case ETIMEDOUT: return Error_ETIMEDOUT; case ETXTBSY: return Error_ETXTBSY;
        case EXDEV: return Error_EXDEV; case ESOCKTNOSUPPORT: return Error_ESOCKTNOSUPPORT; case EPFNOSUPPORT: return Error_EPFNOSUPPORT;
        case ESHUTDOWN: return Error_ESHUTDOWN; case EHOSTDOWN: return Error_EHOSTDOWN; case ENODATA: return Error_ENODATA;
        case -Error_EHOSTNOTFOUND: return Error_EHOSTNOTFOUND; case -Error_ESOCKETERROR: return Error_ESOCKETERROR;
        default: return Error_ENONSTANDARD;
    }
}
PALEXPORT int32_t SystemNative_ConvertErrorPalToPlatform(int32_t e) {
    switch (e) {
        case Error_SUCCESS: return 0;
        case Error_E2BIG: return E2BIG; case Error_EACCES: return EACCES; case Error_EADDRINUSE: return EADDRINUSE; case Error_EADDRNOTAVAIL: return EADDRNOTAVAIL;
        case Error_EAFNOSUPPORT: return EAFNOSUPPORT; case Error_EAGAIN: return EAGAIN; case Error_EALREADY: return EALREADY; case Error_EBADF: return EBADF;
        case Error_EBADMSG: return EBADMSG; case Error_EBUSY: return EBUSY; case Error_ECANCELED: return ECANCELED; case Error_ECHILD: return ECHILD;
        case Error_ECONNABORTED: return ECONNABORTED; case Error_ECONNREFUSED: return ECONNREFUSED; case Error_ECONNRESET: return ECONNRESET;
        case Error_EDEADLK: return EDEADLK; case Error_EDESTADDRREQ: return EDESTADDRREQ; case Error_EDOM: return EDOM; case Error_EDQUOT: return EDQUOT;
        case Error_EEXIST: return EEXIST; case Error_EFAULT: return EFAULT; case Error_EFBIG: return EFBIG; case Error_EHOSTUNREACH: return EHOSTUNREACH;
        case Error_EIDRM: return EIDRM; case Error_EILSEQ: return EILSEQ; case Error_EINPROGRESS: return EINPROGRESS; case Error_EINTR: return EINTR;
        case Error_EINVAL: return EINVAL; case Error_EIO: return EIO; case Error_EISCONN: return EISCONN; case Error_EISDIR: return EISDIR;
        case Error_ELOOP: return ELOOP; case Error_EMFILE: return EMFILE; case Error_EMLINK: return EMLINK; case Error_EMSGSIZE: return EMSGSIZE;
        case Error_EMULTIHOP: return EMULTIHOP; case Error_ENAMETOOLONG: return ENAMETOOLONG; case Error_ENETDOWN: return ENETDOWN;
        case Error_ENETRESET: return ENETRESET; case Error_ENETUNREACH: return ENETUNREACH; case Error_ENFILE: return ENFILE; case Error_ENOBUFS: return ENOBUFS;
        case Error_ENODEV: return ENODEV; case Error_ENOENT: return ENOENT; case Error_ENOEXEC: return ENOEXEC; case Error_ENOLCK: return ENOLCK;
        case Error_ENOLINK: return ENOLINK; case Error_ENOMEM: return ENOMEM; case Error_ENOMSG: return ENOMSG; case Error_ENOPROTOOPT: return ENOPROTOOPT;
        case Error_ENOSPC: return ENOSPC; case Error_ENOSYS: return ENOSYS; case Error_ENOTCONN: return ENOTCONN; case Error_ENOTDIR: return ENOTDIR;
        case Error_ENOTEMPTY: return ENOTEMPTY; case Error_ENOTRECOVERABLE: return ENOTRECOVERABLE; case Error_ENOTSOCK: return ENOTSOCK;
        case Error_ENOTSUP: return ENOTSUP; case Error_ENOTTY: return ENOTTY; case Error_ENXIO: return ENXIO; case Error_EOVERFLOW: return EOVERFLOW;
        case Error_EOWNERDEAD: return EOWNERDEAD; case Error_EPERM: return EPERM; case Error_EPIPE: return EPIPE; case Error_EPROTO: return EPROTO;
        case Error_EPROTONOSUPPORT: return EPROTONOSUPPORT; case Error_EPROTOTYPE: return EPROTOTYPE; case Error_ERANGE: return ERANGE; case Error_EROFS: return EROFS;
        case Error_ESPIPE: return ESPIPE; case Error_ESRCH: return ESRCH; case Error_ESTALE: return ESTALE; case Error_ETIMEDOUT: return ETIMEDOUT;
        case Error_ETXTBSY: return ETXTBSY; case Error_EXDEV: return EXDEV; case Error_ESOCKTNOSUPPORT: return ESOCKTNOSUPPORT; case Error_EPFNOSUPPORT: return EPFNOSUPPORT;
        case Error_ESHUTDOWN: return ESHUTDOWN; case Error_EHOSTDOWN: return EHOSTDOWN; case Error_ENODATA: return ENODATA;
        case Error_EHOSTNOTFOUND: return -Error_EHOSTNOTFOUND; case Error_ESOCKETERROR: return -Error_ESOCKETERROR;
        case Error_ENONSTANDARD: return -1;
        default: return -1;
    }
}
static const char *error_text(int e) {
    switch (e) {
        case ENOENT: return "No such file or directory"; case EACCES: return "Permission denied"; case EBADF: return "Bad file descriptor";
        case EINVAL: return "Invalid argument"; case ENOMEM: return "Cannot allocate memory"; case ENOTSUP: return "Operation not supported";
        case ENOSYS: return "Function not implemented"; case EINTR: return "Interrupted system call"; case EAGAIN: return "Resource temporarily unavailable";
        case EEXIST: return "File exists"; case EISDIR: return "Is a directory"; case ENOTDIR: return "Not a directory"; case EPIPE: return "Broken pipe";
        case ERANGE: return "Numerical result out of range"; case ESPIPE: return "Illegal seek"; case ENOTTY: return "Inappropriate ioctl for device";
        case EIO: return "Input/output error"; case EPERM: return "Operation not permitted"; case ETIMEDOUT: return "Connection timed out";
        case EMFILE: return "Too many open files"; case ENOSPC: return "No space left on device"; case EBUSY: return "Device or resource busy";
        default: return NULL;
    }
}
PALEXPORT const char* SystemNative_StrErrorR(int32_t platformErrno, char* buffer, int32_t bufferSize) {
    const char *text = error_text(platformErrno);
    if (text) return text;
    if (buffer && bufferSize > 0) { snprintf(buffer, (size_t)bufferSize, "Unknown error %d", platformErrno); return buffer; }
    return "Unknown error";
}
PALEXPORT int32_t SystemNative_GetErrNo(void) { return errno; }
PALEXPORT void SystemNative_SetErrNo(int32_t errorCode) { errno = errorCode; }

/* ---- native heap ---------------------------------------------------------- */
static const dotnet_pal_support_ops *heap(void) {
    const dotnet_pal_api *a = api();
    if (!has(a, DOTNET_PAL_SUPPORT_API_SIZE, DOTNET_PAL_CAP_NATIVE_HEAP) || !a->support.allocate || !a->support.resize || !a->support.release) return NULL;
    return &a->support;
}
PALEXPORT void* SystemNative_Malloc(uintptr_t size) {
    const dotnet_pal_support_ops *h = heap(); void *p = NULL;
    if (!h) { errno = ENOMEM; return NULL; }
    if (h->allocate(size ? size : 1, 0, &p) != DOTNET_PAL_OK) { errno = ENOMEM; return NULL; }
    return p;
}
PALEXPORT void* SystemNative_Calloc(uintptr_t num, uintptr_t size) {
    const dotnet_pal_support_ops *h = heap(); void *p = NULL;
    if (!h || (size != 0 && num > SIZE_MAX / size)) { errno = ENOMEM; return NULL; }
    uintptr_t total = num * size;
    if (h->allocate(total ? total : 1, 1, &p) != DOTNET_PAL_OK) { errno = ENOMEM; return NULL; }
    return p;
}
PALEXPORT void* SystemNative_Realloc(void* ptr, uintptr_t new_size) {
    const dotnet_pal_support_ops *h = heap(); void *p = NULL;
    if (!h) { errno = ENOMEM; return NULL; }
    if (!ptr) return SystemNative_Malloc(new_size);
    if (new_size == 0) { h->release(ptr); return NULL; }
    if (h->resize(ptr, new_size, &p) != DOTNET_PAL_OK) { errno = ENOMEM; return NULL; }
    return p;
}
PALEXPORT void SystemNative_Free(void* ptr) { const dotnet_pal_support_ops *h = heap(); if (ptr && h) h->release(ptr); }
/* Over-aligned blocks carry the original pointer just before the aligned start;
 * the marker distinguishes them from plain 16-byte blocks in AlignedFree. */
#define ALIGNED_MARK UINT64_C(0x50414c414c49474e)
PALEXPORT void* SystemNative_AlignedAlloc(uintptr_t alignment, uintptr_t size) {
    if (alignment == 0 || (alignment & (alignment - 1)) != 0) { errno = EINVAL; return NULL; }
    if (alignment <= 16) return SystemNative_Malloc(size);
    if (size > SIZE_MAX - alignment - 32) { errno = ENOMEM; return NULL; }
    uint8_t *raw = SystemNative_Malloc(size + alignment + 32);
    if (!raw) return NULL;
    uintptr_t start = ((uintptr_t)raw + 32 + alignment - 1) & ~(alignment - 1);
    ((uint64_t*)start)[-1] = ALIGNED_MARK; ((void**)start)[-2] = raw;
    return (void*)start;
}
PALEXPORT void SystemNative_AlignedFree(void* ptr) {
    if (!ptr) return;
    uint64_t *mark = (uint64_t*)ptr - 1;
    /* A plain block from Malloc has no readable word before it we could trust;
     * callers pair AlignedFree with AlignedAlloc, whose blocks always carry the mark. */
    if (*mark == ALIGNED_MARK) SystemNative_Free(((void**)ptr)[-2]); else SystemNative_Free(ptr);
}
PALEXPORT void* SystemNative_AlignedRealloc(void* ptr, uintptr_t alignment, uintptr_t size) {
    void *fresh = SystemNative_AlignedAlloc(alignment, size);
    if (!fresh || !ptr) return fresh;
    /* The old block size is not recorded; the BCL uses AlignedRealloc only for growth of its own buffers. */
    memcpy(fresh, ptr, size);
    SystemNative_AlignedFree(ptr);
    return fresh;
}

/* ---- threads and monitors ------------------------------------------------- */
static const dotnet_pal_kernel_ops *kernel(void) {
    const dotnet_pal_api *a = api();
    if (!has(a, DOTNET_PAL_KERNEL_API_SIZE, DOTNET_PAL_CAP_MUTEX | DOTNET_PAL_CAP_EVENTS | DOTNET_PAL_CAP_TLS)) return NULL;
    return &a->kernel;
}
typedef struct Waiter { struct Waiter *next; void *event; bool signaled; } Waiter;
struct LowLevelMonitor { void *mutex; Waiter *head; Waiter *tail; };
/* One auto-reset event per thread, created on first wait and released with the thread. */
static void *waiter_slot;
static void release_waiter_event(void *value) { const dotnet_pal_kernel_ops *k = kernel(); if (k && value) k->event_destroy(value); }
static void *thread_event(void) {
    const dotnet_pal_kernel_ops *k = kernel(); void *event = NULL;
    if (!k) return NULL;
    if (!waiter_slot) {
        void *slot = NULL;
        if (k->tls_create(release_waiter_event, &slot) != DOTNET_PAL_OK) return NULL;
        /* Two threads may race here; the loser destroys its slot and uses the winner's. */
        if (!__atomic_compare_exchange_n(&waiter_slot, &(void*){NULL}, slot, false, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) k->tls_destroy(slot);
    }
    if (k->tls_get(waiter_slot, &event) != DOTNET_PAL_OK) return NULL;
    if (event) return event;
    if (k->event_create(0, 0, &event) != DOTNET_PAL_OK) return NULL;
    if (k->tls_set(waiter_slot, event) != DOTNET_PAL_OK) { k->event_destroy(event); return NULL; }
    return event;
}
PALEXPORT LowLevelMonitor *SystemNative_LowLevelMonitor_Create(void) {
    const dotnet_pal_kernel_ops *k = kernel();
    LowLevelMonitor *m = k ? SystemNative_Malloc(sizeof *m) : NULL;
    if (!m) return NULL;
    m->head = m->tail = NULL;
    if (k->mutex_create(0, &m->mutex) != DOTNET_PAL_OK) { SystemNative_Free(m); return NULL; }
    return m;
}
PALEXPORT void SystemNative_LowLevelMonitor_Destroy(LowLevelMonitor* m) {
    const dotnet_pal_kernel_ops *k = kernel();
    if (!m || !k) return;
    if (k->mutex_destroy(m->mutex) != DOTNET_PAL_OK) __builtin_trap();
    SystemNative_Free(m);
}
PALEXPORT void SystemNative_LowLevelMonitor_Acquire(LowLevelMonitor* m) { if (kernel()->mutex_lock(m->mutex) != DOTNET_PAL_OK) __builtin_trap(); }
PALEXPORT void SystemNative_LowLevelMonitor_Release(LowLevelMonitor* m) { if (kernel()->mutex_unlock(m->mutex) != DOTNET_PAL_OK) __builtin_trap(); }
/* Called with the monitor held; returns with it held. */
static int32_t monitor_wait(LowLevelMonitor *m, uint64_t timeout_ns) {
    const dotnet_pal_kernel_ops *k = kernel();
    Waiter self = {NULL, thread_event(), false};
    if (!self.event) __builtin_trap();
    if (m->tail) m->tail->next = &self; else m->head = &self;
    m->tail = &self;
    SystemNative_LowLevelMonitor_Release(m);
    uint32_t status = k->event_wait(self.event, timeout_ns);
    SystemNative_LowLevelMonitor_Acquire(m);
    if (self.signaled) {
        /* Signaled after a timeout raced with the wake-up: consume the event so the next wait starts clean. */
        if (status != DOTNET_PAL_OK) (void)k->event_wait(self.event, 0);
        return 1;
    }
    /* Timed out (or failed) while still queued: unlink ourselves. */
    Waiter **link = &m->head; Waiter *previous = NULL;
    while (*link && *link != &self) { previous = *link; link = &(*link)->next; }
    if (*link == &self) { *link = self.next; if (m->tail == &self) m->tail = previous; }
    if (status != DOTNET_PAL_OK && status != DOTNET_PAL_TIMEOUT) __builtin_trap();
    return 0;
}
PALEXPORT void SystemNative_LowLevelMonitor_Wait(LowLevelMonitor* m) { (void)monitor_wait(m, DOTNET_PAL_INFINITE_NS); }
PALEXPORT int32_t SystemNative_LowLevelMonitor_TimedWait(LowLevelMonitor* m, int32_t timeoutMilliseconds) {
    return monitor_wait(m, timeoutMilliseconds < 0 ? DOTNET_PAL_INFINITE_NS : (uint64_t)timeoutMilliseconds * 1000000u);
}
PALEXPORT void SystemNative_LowLevelMonitor_Signal_Release(LowLevelMonitor* m) {
    Waiter *w = m->head;
    if (w) {
        m->head = w->next; if (!m->head) m->tail = NULL;
        w->signaled = true;
        if (kernel()->event_set(w->event) != DOTNET_PAL_OK) __builtin_trap();
    }
    SystemNative_LowLevelMonitor_Release(m);
}
typedef struct { void *(*start)(void*); void *parameter; } ThreadStart;
static void *thread_entry(void *argument) {
    ThreadStart start = *(ThreadStart*)argument;
    SystemNative_Free(argument);
    return start.start(start.parameter);
}
PALEXPORT int32_t SystemNative_CreateThread(uintptr_t stackSize, void *(*startAddress)(void*), void *parameter) {
    const dotnet_pal_kernel_ops *k = kernel();
    if (!k || !k->thread_create || !k->thread_detach) { errno = ENOTSUP; return 0; }
    ThreadStart *start = SystemNative_Malloc(sizeof *start);
    if (!start) return 0;
    start->start = startAddress; start->parameter = parameter;
    void *handle = NULL;
    if (k->thread_create(thread_entry, start, stackSize, &handle) != DOTNET_PAL_OK) { SystemNative_Free(start); errno = EAGAIN; return 0; }
    if (k->thread_detach(handle) != DOTNET_PAL_OK) __builtin_trap();
    return 1;
}
PALEXPORT int32_t SystemNative_SchedGetCpu(void) {
    const dotnet_pal_api *a = api(); uint32_t cpu = 0;
    if (!has(a, DOTNET_PAL_TOPOLOGY_API_SIZE, DOTNET_PAL_CAP_TOPOLOGY) || !a->topology.current_cpu || a->topology.current_cpu(&cpu) != DOTNET_PAL_OK) return -1;
    return cpu > INT32_MAX ? -1 : (int32_t)cpu;
}
static uint64_t identity(bool thread) {
    const dotnet_pal_api *a = api(); uint64_t id = 0;
    if (!has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_IDENTITY)) return 0;
    return (thread ? a->runtime.thread_id : a->runtime.process_id)(&id) == DOTNET_PAL_OK ? id : 0;
}
PALEXPORT uint64_t SystemNative_GetUInt64OSThreadId(void) { return identity(true); }
PALEXPORT uint32_t SystemNative_TryGetUInt32OSThreadId(void) { uint64_t id = identity(true); return id > UINT32_MAX ? 0 : (uint32_t)id; }
PALEXPORT int32_t SystemNative_GetPid(void) { uint64_t id = identity(false); return id > INT32_MAX ? 1 : (int32_t)id; }
PALEXPORT __attribute__((noreturn)) void SystemNative_Exit(int32_t exitCode) {
    const dotnet_pal_api *a = api();
    if (has(a, DOTNET_PAL_PROCESS_API_SIZE, DOTNET_PAL_CAP_PROCESS) && a->process.exit) a->process.exit(exitCode);
    exit(exitCode);
}
PALEXPORT __attribute__((noreturn)) void SystemNative_Abort(void) { abort(); }

/* ---- clocks ---------------------------------------------------------------- */
static uint64_t monotonic_ns(void) {
    const dotnet_pal_api *a = api(); uint64_t ns = 0;
    if (!has(a, DOTNET_PAL_SERVICES_API_SIZE, DOTNET_PAL_CAP_CLOCK) || a->services.monotonic_ns(&ns) != DOTNET_PAL_OK) __builtin_trap();
    return ns;
}
PALEXPORT int64_t SystemNative_GetTimestamp(void) { uint64_t ns = monotonic_ns(); return ns > INT64_MAX ? INT64_MAX : (int64_t)ns; }
PALEXPORT int64_t SystemNative_GetLowResolutionTimestamp(void) { return SystemNative_GetTimestamp() / 1000000; }
PALEXPORT int64_t SystemNative_GetSystemTimeAsTicks(void) {
    const dotnet_pal_api *a = api(); uint64_t ns = 0;
    if (!has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_REALTIME) || a->runtime.realtime_ns(&ns) != DOTNET_PAL_OK) return -1;
    /* 100 ns ticks since 1601, as the BCL expects. */
    return (int64_t)(ns / 100 + UINT64_C(116444736000000000));
}
PALEXPORT char* SystemNative_GetDefaultTimeZone(void) { return NULL; }
PALEXPORT const char* SystemNative_GetTimeZoneData(const char* name, int* length) { (void)name; *length = 0; return NULL; }

/* ---- entropy -------------------------------------------------------------- */
PALEXPORT int32_t SystemNative_GetCryptographicallySecureRandomBytes(uint8_t* buffer, int32_t bufferLength) {
    const dotnet_pal_api *a = api();
    if (bufferLength < 0) return -1;
    if (!has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_ENTROPY) || !a->runtime.random_bytes) return -1;
    return a->runtime.random_bytes(buffer, (size_t)bufferLength) == DOTNET_PAL_OK ? 0 : -1;
}
PALEXPORT void SystemNative_GetNonCryptographicallySecureRandomBytes(uint8_t* buffer, int32_t bufferLength) {
    if (SystemNative_GetCryptographicallySecureRandomBytes(buffer, bufferLength) == 0) return;
    /* No entropy source: a clock-seeded xorshift stream, explicitly not secure. */
    static uint64_t state;
    if (state == 0) state = monotonic_ns() | 1;
    for (int32_t i = 0; i < bufferLength; ++i) {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        buffer[i] = (uint8_t)state;
    }
}

/* ---- environment ------------------------------------------------------------ */
/* GetEnv returns pointers that stay valid for the process, so looked-up values
 * are copied once into the native heap and cached by name. */
typedef struct EnvEntry { struct EnvEntry *next; char *name; char *value; } EnvEntry;
static EnvEntry *env_cache;
PALEXPORT char* SystemNative_GetEnv(const char* variable) {
    const dotnet_pal_api *a = api();
    if (!variable || !has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_ENVIRONMENT)) return NULL;
    size_t length = strlen(variable);
    for (EnvEntry *e = __atomic_load_n(&env_cache, __ATOMIC_ACQUIRE); e; e = e->next) if (strcmp(e->name, variable) == 0) return e->value;
    size_t needed = 0;
    uint32_t status = a->runtime.environment_get((const uint8_t*)variable, length, NULL, 0, &needed);
    if (status != DOTNET_PAL_BUFFER_TOO_SMALL || needed == 0) return NULL;
    EnvEntry *entry = SystemNative_Malloc(sizeof *entry + length + 1 + needed);
    if (!entry) return NULL;
    entry->name = (char*)(entry + 1); memcpy(entry->name, variable, length + 1);
    entry->value = entry->name + length + 1;
    if (a->runtime.environment_get((const uint8_t*)variable, length, (uint8_t*)entry->value, needed, &needed) != DOTNET_PAL_OK) { SystemNative_Free(entry); return NULL; }
    EnvEntry *head;
    do { head = __atomic_load_n(&env_cache, __ATOMIC_ACQUIRE); entry->next = head; }
    while (!__atomic_compare_exchange_n(&env_cache, &head, entry, false, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE));
    return entry->value;
}

/* ---- process facilities the boundary does not have --------------------------------- */
PALEXPORT int32_t SystemNative_SetEUid(uint32_t euid) { (void)euid; return fail(ENOTSUP); }
PALEXPORT char* SystemNative_GetGroupName(uint32_t gid) { (void)gid; return NULL; }
PALEXPORT const char* SystemNative_SearchPath(int32_t folderId) { (void)folderId; return NULL; }
PALEXPORT const char* SystemNative_SearchPath_TempDirectory(void) { return NULL; }
PALEXPORT int32_t SystemNative_GetSid(int32_t pid) { (void)pid; return fail(ENOTSUP); }
PALEXPORT int64_t SystemNative_PathConf(const char* path, int32_t name) { (void)path; (void)name; return fail(ENOTSUP); }

/* ---- diagnostics output -------------------------------------------------------- */
static void diagnostics(const uint8_t *buffer, size_t length) {
    const dotnet_pal_api *a = api(); size_t written = 0;
    if (has(a, DOTNET_PAL_SUPPORT_API_SIZE, DOTNET_PAL_CAP_DIAGNOSTICS) && a->support.write_stderr) (void)a->support.write_stderr(buffer, length, &written);
}
PALEXPORT void SystemNative_Log(uint8_t* buffer, int32_t length) {
    const dotnet_pal_streams_ops *s = sn_streams(); size_t written = 0;
    if (length <= 0) return;
    if (s && s->write(1, buffer, (size_t)length, &written) == DOTNET_PAL_OK) return;
    diagnostics(buffer, (size_t)length);
}
PALEXPORT void SystemNative_LogError(uint8_t* buffer, int32_t length) { if (length > 0) diagnostics(buffer, (size_t)length); }
PALEXPORT void SystemNative_SysLog(SysLogPriority priority, const char* message, const char* arg1) {
    char line[512]; (void)priority;
    int n = snprintf(line, sizeof line, message ? message : "%s", arg1 ? arg1 : "");
    if (n > 0) { size_t length = (size_t)n < sizeof line - 1 ? (size_t)n : sizeof line - 2; line[length++] = '\n'; diagnostics((const uint8_t*)line, length); }
}
PALEXPORT int32_t SystemNative_SNPrintF(char* string, int32_t size, const char* format, ...) {
    va_list args; va_start(args, format);
    int result = vsnprintf(string, size < 0 ? 0 : (size_t)size, format, args);
    va_end(args);
    return result;
}
PALEXPORT int32_t SystemNative_SNPrintF_1S(char* string, int32_t size, const char* format, char* str) { return snprintf(string, size < 0 ? 0 : (size_t)size, format, str); }
PALEXPORT int32_t SystemNative_SNPrintF_1I(char* string, int32_t size, const char* format, int arg) { return snprintf(string, size < 0 ? 0 : (size_t)size, format, arg); }
