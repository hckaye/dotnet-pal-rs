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
PALEXPORT int32_t SystemNative_GetPwUidR(uint32_t uid, Passwd* pwd, char* buf, int32_t buflen) { return current_user(pwd, buf, buflen, NULL, uid, false); }
PALEXPORT int32_t SystemNative_GetPwNamR(const char* name, Passwd* pwd, char* buf, int32_t buflen) {
    if (!name) { memset(pwd, 0, sizeof *pwd); return EINVAL; }
    return current_user(pwd, buf, buflen, name, 0, true);
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
static TerminalInvalidationCallback terminal_invalidation;
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
    if (n && signals[i].kind != 0 && __atomic_load_n(&installed, __ATOMIC_ACQUIRE) == 2) (void)n->disable(signals[i].kind);
}

/* ---- the terminal --------------------------------------------------------------------------- */
static const dotnet_pal_terminal_ops *terminal(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_TERMINAL_API_SIZE, DOTNET_PAL_CAP_TERMINAL) ? &a->terminal : NULL;
}
static int32_t break_is_signal = 1; /* Ctrl+C interrupts unless the program asked to read it as input */
static int32_t reading_raw;
static int32_t mode_changed;
static void apply_input_mode(uint32_t raw, uint32_t minimum, uint32_t timeout) {
    const dotnet_pal_terminal_ops *t = terminal();
    if (!t || !t->set_input_mode) return;
    __atomic_store_n(&mode_changed, 1, __ATOMIC_RELEASE);
    (void)t->set_input_mode(raw, minimum, timeout, __atomic_load_n(&break_is_signal, __ATOMIC_ACQUIRE) ? 0 : 1);
}
/* A program that ends while it reads keys, or with Ctrl+C taken as input, would leave its terminal without echo, line
 * editing or an interrupt key for whoever uses it next. The reference implementation restores the terminal from
 * atexit as well. */
static void restore_terminal(void) {
    const dotnet_pal_terminal_ops *t = terminal();
    if (t && t->set_input_mode && __atomic_exchange_n(&mode_changed, 0, __ATOMIC_ACQ_REL)) (void)t->set_input_mode(0, 0, 0, 0);
}
PALEXPORT int32_t SystemNative_InitializeTerminalAndSignalHandling(void) {
    static int32_t registered;
    if (!__atomic_exchange_n(&registered, 1, __ATOMIC_ACQ_REL)) (void)atexit(restore_terminal);
    return 1;
}
PALEXPORT void SystemNative_UninitializeTerminal(void) { if (__atomic_exchange_n(&reading_raw, 0, __ATOMIC_ACQ_REL)) apply_input_mode(0, 0, 0); }
PALEXPORT void SystemNative_InitializeConsoleBeforeRead(uint8_t minChars, uint8_t decisecondsTimeout) {
    __atomic_store_n(&reading_raw, 1, __ATOMIC_RELEASE);
    apply_input_mode(1, minChars, decisecondsTimeout);
}
PALEXPORT void SystemNative_UninitializeConsoleAfterRead(void) { __atomic_store_n(&reading_raw, 0, __ATOMIC_RELEASE); apply_input_mode(0, 0, 0); }
PALEXPORT int32_t SystemNative_GetSignalForBreak(void) { return __atomic_load_n(&break_is_signal, __ATOMIC_ACQUIRE); }
PALEXPORT int32_t SystemNative_SetSignalForBreak(int32_t signalForBreak) {
    __atomic_store_n(&break_is_signal, signalForBreak != 0, __ATOMIC_RELEASE);
    /* Takes effect now, in whichever mode the terminal is: the default timing of line mode is the terminal's own. */
    apply_input_mode(__atomic_load_n(&reading_raw, __ATOMIC_ACQUIRE) ? 1 : 0, 1, 0);
    return 1;
}
PALEXPORT int32_t SystemNative_StdinReady(void) {
    const dotnet_pal_terminal_ops *t = terminal(); uint32_t ready = 0;
    return t && t->input_ready && t->input_ready(&ready) == DOTNET_PAL_OK && ready;
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
PALEXPORT void SystemNative_ConfigureTerminalForChildProcess(int32_t enable) { (void)enable; }
PALEXPORT void SystemNative_SetDelayedSigChildConsoleConfigurationHandler(void (*callback)(void)) { (void)callback; }
