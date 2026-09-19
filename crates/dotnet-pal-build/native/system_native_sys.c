/* System.Native over the boundary: facts about the process and the machine,
 * requests from outside the process, the terminal, and module loading.
 *
 * The BCL sees a POSIX face here: an environ array, passwd entries, uname texts,
 * signal numbers, termios-shaped console calls. Behind it are the boundary's
 * system, notifications and terminal groups, none of which knows about any of
 * those. Signal numbers in particular are only names on this side: the
 * notifications group reports kinds, and which mechanism produced one is the
 * port's business. Without a group, the corresponding entry points say what
 * they said before it existed: no environment to enumerate, user 0, no terminal,
 * signal registrations that are accepted and never fire. */
#include "system_native_internal.h"
#include <signal.h>

static const dotnet_pal_system_ops *sys(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_SYSTEM_API_SIZE, DOTNET_PAL_CAP_SYSTEM) ? &a->system : NULL;
}
/* A text of the system group in fresh native-heap storage, or NULL with errno. */
static char *system_text(uint32_t what) {
    const dotnet_pal_system_ops *s = sys(); size_t needed = 0;
    if (!s || !s->text) { errno = ENOTSUP; return NULL; }
    uint32_t status = s->text(what, NULL, 0, &needed);
    if (status != DOTNET_PAL_BUFFER_TOO_SMALL) { errno = sn_errno(status); return NULL; }
    char *text = SystemNative_Malloc(needed);
    if (!text) return NULL;
    status = s->text(what, (uint8_t*)text, needed, &needed);
    if (status != DOTNET_PAL_OK) { SystemNative_Free(text); errno = sn_errno(status); return NULL; }
    return text;
}

/* ---- environment enumeration ------------------------------------------------------ */
/* A NULL-terminated array of NAME=value texts, each its own allocation: FreeEnviron releases what GetEnviron built. */
PALEXPORT void SystemNative_FreeEnviron(char** environ) {
    if (!environ) return;
    for (char **entry = environ; *entry; ++entry) SystemNative_Free(*entry);
    SystemNative_Free(environ);
}
PALEXPORT char** SystemNative_GetEnviron(void) {
    const dotnet_pal_system_ops *s = sys(); size_t capacity = 64, count = 0;
    if (!s || !s->environment_entry) return NULL;
    char **list = SystemNative_Calloc(capacity + 1, sizeof *list);
    /* An entry the boundary cannot hand over (longer than its limit) is left out; the rest of the environment is not. */
    for (size_t index = 0; list && index < 65536; ++index) {
        size_t needed = 0;
        uint32_t status = s->environment_entry(index, NULL, 0, &needed);
        if (status == DOTNET_PAL_NOT_FOUND) return list;
        if (status != DOTNET_PAL_BUFFER_TOO_SMALL) continue;
        char *entry = SystemNative_Malloc(needed);
        /* The environment can change between the two calls only if somebody breaks the group's contract; give up then. */
        if (!entry || s->environment_entry(index, (uint8_t*)entry, needed, &needed) != DOTNET_PAL_OK) { SystemNative_Free(entry); break; }
        if (count == capacity) {
            char **grown = SystemNative_Calloc(capacity * 2 + 1, sizeof *grown);
            if (!grown) { SystemNative_Free(entry); break; }
            memcpy(grown, list, count * sizeof *list);
            SystemNative_Free(list); list = grown; capacity *= 2;
        }
        list[count++] = entry;
    }
    SystemNative_FreeEnviron(list);
    return NULL;
}

/* ---- the process, the machine and the user ------------------------------------------ */
PALEXPORT char* SystemNative_GetProcessPath(void) { return system_text(DOTNET_PAL_TEXT_EXECUTABLE_PATH); }
PALEXPORT char* SystemNative_GetUnixRelease(void) { return system_text(DOTNET_PAL_TEXT_OS_RELEASE); }
/* "name release version", as uname prints it. -1 with the capacity needed when the buffer is too small. */
PALEXPORT int32_t SystemNative_GetUnixVersion(char* version, int* capacity) {
    if (!version || !capacity || *capacity <= 0) return -1;
    char *name = system_text(DOTNET_PAL_TEXT_OS_NAME), *release = system_text(DOTNET_PAL_TEXT_OS_RELEASE), *build = system_text(DOTNET_PAL_TEXT_OS_VERSION);
    int written = snprintf(version, (size_t)*capacity, "%s %s %s", name ? name : "Unknown", release ? release : "", build ? build : "");
    SystemNative_Free(name); SystemNative_Free(release); SystemNative_Free(build);
    if (written >= *capacity) { *capacity = written + 1; return -1; }
    return 0;
}
PALEXPORT int32_t SystemNative_GetOSArchitecture(void) {
    enum { ARCH_X86, ARCH_X64, ARCH_ARM, ARCH_ARM64, ARCH_WASM, ARCH_S390X, ARCH_LOONGARCH64, ARCH_ARMV6, ARCH_POWERPC64, ARCH_RISCV64 };
#if defined(__aarch64__)
    return ARCH_ARM64;
#elif defined(__x86_64__)
    return ARCH_X64;
#elif defined(__i386__)
    return ARCH_X86;
#elif defined(__arm__)
    return ARCH_ARM;
#elif defined(__riscv) && __riscv_xlen == 64
    return ARCH_RISCV64;
#elif defined(__wasm__)
    return ARCH_WASM;
#else
#error "architecture not listed in System.Runtime.InteropServices.Architecture"
#endif
}
static bool process_times(uint64_t *user, uint64_t *kernel) {
    const dotnet_pal_system_ops *s = sys();
    return s && s->process_times && s->process_times(user, kernel) == DOTNET_PAL_OK;
}
static uint64_t monotonic(void) {
    const dotnet_pal_api *a = sn_api(); uint64_t ns = 0;
    if (sn_has(a, DOTNET_PAL_SERVICES_API_SIZE, DOTNET_PAL_CAP_CLOCK) && a->services.monotonic_ns) (void)a->services.monotonic_ns(&ns);
    return ns;
}
/* The share of one processor this process used since the previous call, in percent, as the reference computes it.
 * Without process times the thread pool sees an idle process. */
PALEXPORT double SystemNative_GetCpuUtilization(ProcessCpuInformation* previous) {
    uint64_t user = 0, kernel = 0, now = monotonic();
    if (!previous || !process_times(&user, &kernel)) return 0;
    uint64_t elapsed = now > previous->lastRecordedCurrentTime ? now - previous->lastRecordedCurrentTime : 0;
    uint64_t busy = user >= previous->lastRecordedUserTime && kernel >= previous->lastRecordedKernelTime
        ? (user - previous->lastRecordedUserTime) + (kernel - previous->lastRecordedKernelTime) : 0;
    previous->lastRecordedCurrentTime = now; previous->lastRecordedUserTime = user; previous->lastRecordedKernelTime = kernel;
    return elapsed > 0 && busy > 0 ? (double)busy * 100.0 / (double)elapsed : 0.0;
}
/* The moment the machine started, in 100 ns ticks since year 1; -1 when the boundary cannot say. */
PALEXPORT int64_t SystemNative_GetBootTimeTicks(void) {
    const dotnet_pal_api *a = sn_api(); const dotnet_pal_system_ops *s = sys(); uint64_t uptime = 0, wall = 0;
    if (!s || !s->uptime_ns || s->uptime_ns(&uptime) != DOTNET_PAL_OK) return -1;
    if (!sn_has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_REALTIME) || a->runtime.realtime_ns(&wall) != DOTNET_PAL_OK || wall < uptime) return -1;
    return INT64_C(621355968000000000) + (int64_t)((wall - uptime) / 100);
}
static bool user_ids(uint32_t *user, uint32_t *group) {
    const dotnet_pal_system_ops *s = sys();
    *user = 0; *group = 0;
    return s && s->user_ids && s->user_ids(user, group) == DOTNET_PAL_OK;
}
PALEXPORT uint32_t SystemNative_GetEUid(void) { uint32_t user, group; (void)user_ids(&user, &group); return user; }
PALEXPORT uint32_t SystemNative_GetEGid(void) { uint32_t user, group; (void)user_ids(&user, &group); return group; }
/* The boundary knows one user, the one the process runs as. Its entry is packed into the caller's buffer:
 * 0 on success, -1 for any other user (the shim's "no such entry"), ERANGE when the buffer is too small. */
static int32_t current_user(Passwd *pwd, char *buffer, int32_t capacity, const char *wanted_name, uint32_t wanted_id, bool by_name) {
    uint32_t user, group;
    memset(pwd, 0, sizeof *pwd);
    if (!buffer || capacity < 0) return EINVAL;
    if (!user_ids(&user, &group)) return -1;
    char *name = system_text(DOTNET_PAL_TEXT_USER_NAME), *home = system_text(DOTNET_PAL_TEXT_HOME_DIRECTORY);
    int32_t result = -1;
    if (name && (by_name ? strcmp(name, wanted_name) == 0 : user == wanted_id)) {
        size_t name_size = strlen(name) + 1, home_size = home ? strlen(home) + 1 : 1;
        if (name_size + home_size + 1 > (size_t)capacity) result = ERANGE;
        else {
            char *empty = buffer + name_size + home_size;
            memcpy(buffer, name, name_size);
            if (home) memcpy(buffer + name_size, home, home_size); else buffer[name_size] = 0;
            *empty = 0;
            pwd->Name = buffer; pwd->HomeDirectory = buffer + name_size; pwd->Password = empty; pwd->UserInfo = empty; pwd->Shell = empty;
            pwd->UserId = user; pwd->GroupId = group;
            result = 0;
        }
    }
    SystemNative_Free(name); SystemNative_Free(home);
    return result;
}
/* Any account, through the accounts group. -1: no such account; an errno otherwise, ERANGE when the buffer is short. */
static int32_t any_user(Passwd *pwd, char *buffer, int32_t capacity, uint32_t status, const dotnet_pal_account *account) {
    if (status == DOTNET_PAL_NOT_FOUND) return -1;
    if (status != DOTNET_PAL_OK) return sn_errno(status);
    size_t name = strlen((const char*)account->name) + 1, home = strlen((const char*)account->home) + 1, shell = strlen((const char*)account->shell) + 1;
    if (name + home + shell + 1 > (size_t)capacity) return ERANGE;
    char *empty = buffer + name + home + shell;
    memcpy(buffer, account->name, name); memcpy(buffer + name, account->home, home); memcpy(buffer + name + home, account->shell, shell);
    *empty = 0;
    pwd->Name = buffer; pwd->HomeDirectory = buffer + name; pwd->Shell = buffer + name + home; pwd->Password = empty; pwd->UserInfo = empty;
    pwd->UserId = account->user_id; pwd->GroupId = account->group_id;
    return 0;
}
/* The account record is 1.5 KiB: it lives on the heap, because managed threads may run on small stacks. */
PALEXPORT int32_t SystemNative_GetPwUidR(uint32_t uid, Passwd* pwd, char* buf, int32_t buflen) {
    const dotnet_pal_accounts_ops *a = sn_accounts();
    if (!a || !a->user_by_id) return current_user(pwd, buf, buflen, NULL, uid, false);
    memset(pwd, 0, sizeof *pwd);
    if (!buf || buflen < 0) return EINVAL;
    dotnet_pal_account *account = SystemNative_Calloc(1, sizeof *account);
    if (!account) return ENOMEM;
    int32_t result = any_user(pwd, buf, buflen, a->user_by_id(uid, account, sizeof *account), account);
    SystemNative_Free(account);
    return result;
}
PALEXPORT int32_t SystemNative_GetPwNamR(const char* name, Passwd* pwd, char* buf, int32_t buflen) {
    const dotnet_pal_accounts_ops *a = sn_accounts();
    if (!name) { memset(pwd, 0, sizeof *pwd); return EINVAL; }
    if (!a || !a->user_by_name) return current_user(pwd, buf, buflen, name, 0, true);
    memset(pwd, 0, sizeof *pwd);
    if (!buf || buflen < 0) return EINVAL;
    dotnet_pal_account *account = SystemNative_Calloc(1, sizeof *account);
    if (!account) return ENOMEM;
    int32_t result = any_user(pwd, buf, buflen, a->user_by_name((const uint8_t*)name, strlen(name), account, sizeof *account), account);
    SystemNative_Free(account);
    return result;
}
/* getgrouplist: the count on success; -1 with the count needed in *ngroups when the list does not fit. */
PALEXPORT int32_t SystemNative_GetGroupList(const char* name, uint32_t group, uint32_t* groups, int32_t* ngroups) {
    const dotnet_pal_accounts_ops *a = sn_accounts(); size_t count = 0;
    if (!name || !groups || !ngroups || *ngroups < 0) return sn_fail(EINVAL);
    if (!a || !a->user_groups) return sn_fail(ENOTSUP);
    uint32_t status = a->user_groups((const uint8_t*)name, strlen(name), group, groups, (size_t)*ngroups, &count);
    if (status == DOTNET_PAL_BUFFER_TOO_SMALL) { *ngroups = count > INT32_MAX ? INT32_MAX : (int32_t)count; return -1; }
    if (status != DOTNET_PAL_OK) return sn_status(status);
    *ngroups = (int32_t)count;
    return (int32_t)count;
}
/* getgroups: the count, which is also the answer to a request with no room at all; EINVAL when the list does not fit. */
PALEXPORT int32_t SystemNative_GetGroups(int32_t ngroups, uint32_t* groups) {
    const dotnet_pal_accounts_ops *a = sn_accounts(); size_t count = 0;
    if (ngroups < 0 || (ngroups > 0 && !groups)) return sn_fail(EINVAL);
    if (!a || !a->process_groups) return sn_fail(ENOTSUP);
    uint32_t status = a->process_groups(groups, (size_t)ngroups, &count);
    if (status == DOTNET_PAL_BUFFER_TOO_SMALL) return ngroups == 0 ? (int32_t)count : sn_fail(EINVAL);
    return status == DOTNET_PAL_OK ? (int32_t)count : sn_status(status);
}
/* Priorities of single processes. -1 is a niceness like any other, so success is told by errno, which the managed side
 * clears before it asks. */
PALEXPORT int32_t SystemNative_GetPriority(int32_t which, int32_t who) {
    const dotnet_pal_priority_ops *p = sn_priority(); int32_t value = 0;
    if (which != 0 /* PRIO_PROCESS */ || who < 0) return sn_fail(EINVAL);
    if (!p) return sn_fail(ENOTSUP);
    uint32_t status = p->get((uint64_t)who, &value);
    if (status != DOTNET_PAL_OK) return sn_fail(status == DOTNET_PAL_NOT_FOUND ? ESRCH : sn_errno(status));
    errno = 0;
    return value;
}
PALEXPORT int32_t SystemNative_SetPriority(int32_t which, int32_t who, int32_t nice) {
    const dotnet_pal_priority_ops *p = sn_priority();
    if (which != 0 /* PRIO_PROCESS */ || who < 0) return sn_fail(EINVAL);
    if (!p) return sn_fail(ENOTSUP);
    /* setpriority clamps a value outside the range where the boundary rejects it. */
    uint32_t status = p->set((uint64_t)who, nice < -20 ? -20 : nice > 19 ? 19 : nice);
    return status == DOTNET_PAL_OK ? 0 : sn_fail(status == DOTNET_PAL_NOT_FOUND ? ESRCH : sn_errno(status));
}

/* The processors this process may run on, as the first bits of one machine word. Only the process itself can be asked,
 * and the boundary pins threads, not processes, so the mask cannot be set. */
PALEXPORT int32_t SystemNative_SchedGetAffinity(int32_t pid, intptr_t* mask) {
    const dotnet_pal_api *a = sn_api(); uint8_t bits[512] = {0}; size_t needed = 0; uint64_t self = 0; /* room for the boundary's largest bitmap */
    if (!mask) return sn_fail(EFAULT);
    *mask = 0;
    if (pid != 0 && !(sn_has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_IDENTITY) && a->runtime.process_id(&self) == DOTNET_PAL_OK && self == (uint64_t)pid)) return sn_fail(ESRCH);
    if (!sn_has(a, DOTNET_PAL_TOPOLOGY_API_SIZE, DOTNET_PAL_CAP_TOPOLOGY) || !a->topology.process_affinity) return sn_fail(ENOTSUP);
    uint32_t status = a->topology.process_affinity(bits, sizeof bits, &needed);
    if (status != DOTNET_PAL_OK) return sn_fail(sn_errno(status));
    uintptr_t word = 0;
    for (size_t i = 0; i < sizeof word; ++i) word |= (uintptr_t)bits[i] << (8 * i);
    *mask = (intptr_t)word;
    return 0;
}
PALEXPORT int32_t SystemNative_SchedSetAffinity(int32_t pid, intptr_t* mask) { (void)pid; (void)mask; return sn_fail(ENOTSUP); }

/* ---- module loading ------------------------------------------------------------------- */
static const dotnet_pal_runtime_ops *modules(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_MODULES) && a->runtime.module_open ? &a->runtime : NULL;
}
static __thread const char *load_error;
PALEXPORT void* SystemNative_LoadLibrary(const char* filename) {
    const dotnet_pal_runtime_ops *m = modules(); void *handle = NULL;
    if (!m) { load_error = "module loading is not provided"; return NULL; }
    if (!filename || !*filename) { load_error = "no module name"; return NULL; }
    uint32_t status = m->module_open((const uint8_t*)filename, strlen(filename), &handle);
    load_error = status == DOTNET_PAL_OK ? NULL : status == DOTNET_PAL_NOT_FOUND ? "module not found" : "module could not be loaded";
    return status == DOTNET_PAL_OK ? handle : NULL;
}
/* The text of the last failure on this thread, read once, as dlerror has it. */
PALEXPORT void* SystemNative_GetLoadLibraryError(void) { const char *text = load_error; load_error = NULL; return (void*)(uintptr_t)text; }
PALEXPORT void* SystemNative_GetProcAddress(void* handle, const char* symbol) {
    const dotnet_pal_runtime_ops *m = modules(); void *address = NULL;
    if (!m || !handle || !symbol || !*symbol) return NULL;
    return m->module_symbol(handle, (const uint8_t*)symbol, strlen(symbol), &address) == DOTNET_PAL_OK ? address : NULL;
}
PALEXPORT void SystemNative_FreeLibrary(void* handle) { const dotnet_pal_runtime_ops *m = modules(); if (m && handle) (void)m->module_close(handle); }
/* The process image itself: module_open without a name. */
PALEXPORT void* SystemNative_GetDefaultSearchOrderPseudoHandle(void) {
    static void *image;
    void *handle = __atomic_load_n(&image, __ATOMIC_ACQUIRE);
    const dotnet_pal_runtime_ops *m = modules();
    if (handle || !m || m->module_open(NULL, 0, &handle) != DOTNET_PAL_OK) return handle;
    void *expected = NULL;
    if (!__atomic_compare_exchange_n(&image, &expected, handle, false, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) { (void)m->module_close(handle); handle = expected; }
    return handle;
}

/* ---- requests from outside the process -------------------------------------------------- */
typedef int32_t (*PosixSignalHandler)(int32_t signalCode, int32_t signal);
enum { PosixSignalSIGHUP = -1, PosixSignalSIGINT = -2, PosixSignalSIGQUIT = -3, PosixSignalSIGTERM = -4, PosixSignalSIGCHLD = -5, PosixSignalSIGCONT = -6,
       PosixSignalSIGWINCH = -7, PosixSignalSIGTTIN = -8, PosixSignalSIGTTOU = -9, PosixSignalSIGTSTP = -10 };
/* Signal numbers are the vocabulary of the managed side (the Linux values of the headers this is compiled with). */
static const struct { int32_t code; int32_t posix; uint32_t kind; } signals[] = {
    {SIGHUP, PosixSignalSIGHUP, DOTNET_PAL_NOTIFY_HANGUP}, {SIGINT, PosixSignalSIGINT, DOTNET_PAL_NOTIFY_INTERRUPT},
    {SIGQUIT, PosixSignalSIGQUIT, DOTNET_PAL_NOTIFY_QUIT}, {SIGTERM, PosixSignalSIGTERM, DOTNET_PAL_NOTIFY_TERMINATE},
    {SIGCHLD, PosixSignalSIGCHLD, 0}, {SIGCONT, PosixSignalSIGCONT, DOTNET_PAL_NOTIFY_CONTINUE},
    {SIGWINCH, PosixSignalSIGWINCH, DOTNET_PAL_NOTIFY_WINDOW_CHANGE}, {SIGTTIN, PosixSignalSIGTTIN, DOTNET_PAL_NOTIFY_STOP_INPUT},
    {SIGTTOU, PosixSignalSIGTTOU, DOTNET_PAL_NOTIFY_STOP_OUTPUT}, {SIGTSTP, PosixSignalSIGTSTP, DOTNET_PAL_NOTIFY_STOP},
};
#define SIGNAL_COUNT ((int)(sizeof signals / sizeof signals[0]))
static PosixSignalHandler posix_handler;
void sn_terminal_reapply(void);
static TerminalInvalidationCallback terminal_invalidation;
static int32_t console_listens; /* the console asked to hear of window changes and continuations, whatever managed code registers */
static uint32_t registered; /* bit i: managed code registered for signals[i] */
static int32_t installed;   /* 0 none, 1 in progress, 2 the notification handler is installed, 3 it could not be */
static int index_of_code(int32_t code) { for (int i = 0; i < SIGNAL_COUNT; ++i) if (signals[i].code == code) return i; return -1; }
static const dotnet_pal_notifications_ops *notifications(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_NOTIFICATIONS_API_SIZE, DOTNET_PAL_CAP_NOTIFICATIONS) ? &a->notifications : NULL;
}
PALEXPORT void SystemNative_SetPosixSignalHandler(PosixSignalHandler handler) { __atomic_store_n(&posix_handler, handler, __ATOMIC_RELEASE); }
PALEXPORT void SystemNative_SetTerminalInvalidationHandler(TerminalInvalidationCallback callback) { __atomic_store_n(&terminal_invalidation, callback, __ATOMIC_RELEASE); }
PALEXPORT int32_t SystemNative_GetPlatformSignalNumber(int32_t signal) {
    for (int i = 0; i < SIGNAL_COUNT; ++i) if (signals[i].posix == signal) return signals[i].code;
    return 0;
}
/* What the runtime does with a signal no managed handler cancelled: the target's own default action. */
PALEXPORT void SystemNative_HandleNonCanceledPosixSignal(int32_t signalCode) {
    const dotnet_pal_notifications_ops *n = notifications(); int i = index_of_code(signalCode);
    if (n && n->default_action && i >= 0 && signals[i].kind != 0) (void)n->default_action(signals[i].kind);
}
/* Whether managed code holds a registration for the signal. */
bool sn_signal_wanted(int32_t code) { int i = index_of_code(code); return i >= 0 && (__atomic_load_n(&registered, __ATOMIC_ACQUIRE) & (1u << i)); }
/* Called by the process unit when a child ended: managed code may have registered for SIGCHLD too. */
void sn_signal_dispatch(int32_t code) {
    int i = index_of_code(code);
    if (i < 0) return;
    if (code == SIGCHLD || code == SIGCONT || code == SIGWINCH) {
        if (code != SIGWINCH) sn_terminal_reapply();
        TerminalInvalidationCallback invalidate = __atomic_load_n(&terminal_invalidation, __ATOMIC_ACQUIRE);
        if (invalidate) invalidate();
    }
    PosixSignalHandler handler = __atomic_load_n(&posix_handler, __ATOMIC_ACQUIRE);
    bool wanted = handler && (__atomic_load_n(&registered, __ATOMIC_ACQUIRE) & (1u << i));
    if (!(wanted && handler(code, signals[i].posix) != 0)) SystemNative_HandleNonCanceledPosixSignal(code);
}
/* Runs on the port's own thread, never in a signal context, so managed code may be entered from here. */
static void on_notification(uint32_t kind, void *data) {
    (void)data;
    for (int i = 0; i < SIGNAL_COUNT; ++i) if (signals[i].kind == kind) { sn_signal_dispatch(signals[i].code); return; }
}
static bool ensure_installed(const dotnet_pal_notifications_ops *n) {
    for (;;) {
        int32_t state = __atomic_load_n(&installed, __ATOMIC_ACQUIRE), none = 0;
        if (state >= 2) return state == 2;
        if (state == 0 && __atomic_compare_exchange_n(&installed, &none, 1, false, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
            bool ok = n->install && n->install(on_notification, NULL) == DOTNET_PAL_OK;
            __atomic_store_n(&installed, ok ? 2 : 3, __ATOMIC_RELEASE);
        }
    }
}
static void sn_console_listen(void) {
    const dotnet_pal_notifications_ops *n = notifications();
    if (!n || !ensure_installed(n)) return;
    __atomic_store_n(&console_listens, 1, __ATOMIC_RELEASE);
    (void)n->enable(DOTNET_PAL_NOTIFY_WINDOW_CHANGE); (void)n->enable(DOTNET_PAL_NOTIFY_CONTINUE);
}
/* 1 when the registration is in effect. SIGCHLD needs no notification: child ends are reported by the process unit. */
PALEXPORT int32_t SystemNative_EnablePosixSignalHandling(int signalCode) {
    const dotnet_pal_notifications_ops *n = notifications(); int i = index_of_code(signalCode);
    if (i < 0) { errno = EINVAL; return 0; }
    if (signals[i].kind != 0) {
        if (!n || !ensure_installed(n)) { errno = ENOTSUP; return 0; }
        uint32_t status = n->enable(signals[i].kind);
        if (status != DOTNET_PAL_OK) { errno = sn_errno(status); return 0; }
    }
    (void)__atomic_or_fetch(&registered, 1u << i, __ATOMIC_ACQ_REL);
    return 1;
}
PALEXPORT void SystemNative_DisablePosixSignalHandling(int signalCode) {
    const dotnet_pal_notifications_ops *n = notifications(); int i = index_of_code(signalCode);
    if (i < 0) return;
    (void)__atomic_and_fetch(&registered, ~(1u << i), __ATOMIC_ACQ_REL);
    /* The console keeps listening to the two kinds that tell it its picture of the terminal is out of date. */
    bool console = __atomic_load_n(&console_listens, __ATOMIC_ACQUIRE) && (signals[i].kind == DOTNET_PAL_NOTIFY_WINDOW_CHANGE || signals[i].kind == DOTNET_PAL_NOTIFY_CONTINUE);
    if (n && signals[i].kind != 0 && !console && __atomic_load_n(&installed, __ATOMIC_ACQUIRE) == 2) (void)n->disable(signals[i].kind);
}

static void sn_console_listen(void);
/* ---- the terminal --------------------------------------------------------------------------- */
static const dotnet_pal_terminal_ops *terminal(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_TERMINAL_API_SIZE, DOTNET_PAL_CAP_TERMINAL) ? &a->terminal : NULL;
}
/* The console of the BCL owns the terminal the way the reference implementation's does. The first read, or the first
 * change of the break key, puts the terminal into raw mode, and there it stays: the BCL edits and echoes lines itself.
 * It goes back to line mode while a child process uses it, and for good when the process exits. */
static int32_t break_is_signal = 1; /* Ctrl+C interrupts unless the program asked to read it as input */
static int32_t reading;             /* a read is in progress: a child started now is assumed not to use the terminal */
static int32_t mode_changed;        /* this process has set a mode, so there is one to give back */
static int32_t child_uses_terminal;
static int32_t last_minimum = 1, last_timeout;
static void apply_input_mode(uint32_t raw, uint32_t minimum, uint32_t timeout) {
    const dotnet_pal_terminal_ops *t = terminal();
    if (!t || !t->set_input_mode) return;
    __atomic_store_n(&mode_changed, 1, __ATOMIC_RELEASE);
    if (raw) { __atomic_store_n(&last_minimum, (int32_t)minimum, __ATOMIC_RELEASE); __atomic_store_n(&last_timeout, (int32_t)timeout, __ATOMIC_RELEASE); }
    (void)t->set_input_mode(raw, minimum, timeout, __atomic_load_n(&break_is_signal, __ATOMIC_ACQUIRE) ? 0 : 1);
}
/* A program that ends while its terminal is in raw mode would leave it without echo, line editing or an interrupt key
 * for whoever uses it next. The reference implementation restores the terminal from atexit as well. */
static void restore_terminal(void) {
    const dotnet_pal_terminal_ops *t = terminal();
    if (t && t->set_input_mode && __atomic_exchange_n(&mode_changed, 0, __ATOMIC_ACQ_REL)) (void)t->set_input_mode(0, 0, 0, 0);
}
/* A continued process finds its terminal as whoever ran meanwhile left it; a child that used it may have changed it. */
void sn_terminal_reapply(void) {
    if (!__atomic_load_n(&mode_changed, __ATOMIC_ACQUIRE) || __atomic_load_n(&child_uses_terminal, __ATOMIC_ACQUIRE)) return;
    apply_input_mode(1, (uint32_t)__atomic_load_n(&last_minimum, __ATOMIC_ACQUIRE), (uint32_t)__atomic_load_n(&last_timeout, __ATOMIC_ACQUIRE));
}
PALEXPORT int32_t SystemNative_InitializeTerminalAndSignalHandling(void) {
    static int32_t registered;
    if (!__atomic_exchange_n(&registered, 1, __ATOMIC_ACQ_REL)) {
        (void)atexit(restore_terminal);
        /* The console caches the window size and the terminal settings. A resized window and a continued process
         * invalidate both, so these two kinds are listened to from here on, as the reference implementation installs
         * its SIGWINCH and SIGCONT handlers at this point. A port without notifications keeps the first answer. */
        sn_console_listen();
    }
    return 1;
}
PALEXPORT void SystemNative_UninitializeTerminal(void) { restore_terminal(); }
PALEXPORT void SystemNative_InitializeConsoleBeforeRead(uint8_t minChars, uint8_t decisecondsTimeout) {
    __atomic_store_n(&reading, 1, __ATOMIC_RELEASE);
    apply_input_mode(1, minChars, decisecondsTimeout);
}
PALEXPORT void SystemNative_UninitializeConsoleAfterRead(void) { __atomic_store_n(&reading, 0, __ATOMIC_RELEASE); }
PALEXPORT int32_t SystemNative_GetSignalForBreak(void) { return __atomic_load_n(&break_is_signal, __ATOMIC_ACQUIRE); }
PALEXPORT int32_t SystemNative_SetSignalForBreak(int32_t signalForBreak) {
    __atomic_store_n(&break_is_signal, signalForBreak != 0, __ATOMIC_RELEASE);
    apply_input_mode(1, 1, 0);
    return 1;
}
/* Line mode while a child uses the terminal, raw mode again once none does; only when this process set a mode at all. */
PALEXPORT void SystemNative_ConfigureTerminalForChildProcess(int32_t childUsesTerminal) {
    if (__atomic_load_n(&reading, __ATOMIC_ACQUIRE)) return;
    __atomic_store_n(&child_uses_terminal, childUsesTerminal != 0, __ATOMIC_RELEASE);
    if (__atomic_load_n(&mode_changed, __ATOMIC_ACQUIRE)) apply_input_mode(childUsesTerminal ? 0 : 1, 1, 0);
}
/* In line mode a typed key is not input until its line ends, so the question is asked in raw mode. */
PALEXPORT int32_t SystemNative_StdinReady(void) {
    const dotnet_pal_terminal_ops *t = terminal(); uint32_t ready = 0;
    if (!t || !t->input_ready) return 0;
    SystemNative_InitializeConsoleBeforeRead(1, 0);
    int32_t result = t->input_ready(&ready) == DOTNET_PAL_OK && ready;
    SystemNative_UninitializeConsoleAfterRead();
    return result;
}
PALEXPORT int32_t SystemNative_GetWindowSize(intptr_t fd, WinSize* windowSize) {
    const dotnet_pal_terminal_ops *t = terminal(); uint32_t columns = 0, rows = 0;
    if (!windowSize) return sn_fail(EFAULT);
    memset(windowSize, 0, sizeof *windowSize);
    sn_object *object = sn_pin(fd, SN_STREAM, ENOTTY);
    if (!object) return -1;
    uint32_t status = t && t->window_size ? t->window_size((uint32_t)object->stream, &columns, &rows) : DOTNET_PAL_UNSUPPORTED;
    sn_unpin(object);
    if (status != DOTNET_PAL_OK) return sn_fail(status == DOTNET_PAL_NOT_FOUND ? ENOTTY : sn_errno(status));
    windowSize->Col = columns > UINT16_MAX ? UINT16_MAX : (uint16_t)columns; windowSize->Row = rows > UINT16_MAX ? UINT16_MAX : (uint16_t)rows;
    return 0;
}
/* names are System.Native's ControlCharacterNames; a function the terminal lacks reads as the disable value. */
PALEXPORT void SystemNative_GetControlCharacters(int32_t* names, uint8_t* values, int32_t length, uint8_t* posixDisableValue) {
    enum { PAL_VERASE = 2, PAL_VEOF = 4, PAL_VEOL = 11, PAL_VEOL2 = 16 };
    const dotnet_pal_terminal_ops *t = terminal();
    if (posixDisableValue) *posixDisableValue = 0;
    for (int32_t i = 0; i < length && names && values; ++i) {
        uint32_t which = names[i] == PAL_VERASE ? DOTNET_PAL_CONTROL_ERASE : names[i] == PAL_VEOF ? DOTNET_PAL_CONTROL_END_OF_FILE
            : names[i] == PAL_VEOL ? DOTNET_PAL_CONTROL_END_OF_LINE : names[i] == PAL_VEOL2 ? DOTNET_PAL_CONTROL_END_OF_LINE_2 : 0, value = 0;
        values[i] = which != 0 && t && t->control_character && t->control_character(which, &value) == DOTNET_PAL_OK ? (uint8_t)value : 0;
    }
}
/* The keypad-transmit sequence of the terminal description goes to the terminal once, when the console starts using it. */
PALEXPORT void SystemNative_SetKeypadXmit(intptr_t fd, const char* terminfoString) {
    const dotnet_pal_streams_ops *s = sn_streams(); uint32_t is_terminal = 0; size_t written = 0;
    if (!s || !terminfoString || !*terminfoString) return;
    sn_object *object = sn_pin(fd, SN_STREAM, EBADF);
    if (!object) return;
    if (object->stream != 0 && s->is_terminal((uint32_t)object->stream, &is_terminal) == DOTNET_PAL_OK && is_terminal)
        (void)s->write((uint32_t)object->stream, (const uint8_t*)terminfoString, strlen(terminfoString), &written);
    sn_unpin(object);
}
/* A child shares the parent's terminal settings; the boundary's input mode is per process, so there is nothing to hand over. */
PALEXPORT void SystemNative_SetDelayedSigChildConsoleConfigurationHandler(void (*callback)(void)) { (void)callback; }
