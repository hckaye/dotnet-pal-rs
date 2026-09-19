/* System.Native over the boundary: child processes.
 *
 * The BCL starts a process with one call that also makes the pipes, learns that a
 * child has ended from SIGCHLD, and then collects the exit code by process id. The
 * boundary's processes group has no signals and no process-wide wait: it has a
 * handle per child and a blocking wait on it. So each child gets a watcher thread
 * here. The watcher blocks in the boundary's wait, records the exit code, and then
 * does what the reference implementation's signal thread does on SIGCHLD: it
 * calls the callback the Process class registered, which collects the code
 * through WaitPidExitedNoHang below. The pipes become ordinary descriptors of
 * the table in system_native_io.c.
 *
 * Only children started here can be waited for or signalled: the boundary names
 * a process by its handle, never by an identifier somebody else handed out.
 * Running a child under another user is not part of the boundary. */
#include "system_native_internal.h"
#include <signal.h>

typedef int32_t (*SigChldCallback)(int32_t reapAll, int32_t configureConsole);
typedef struct child {
    struct child *next;
    void *process;
    int32_t pid;
    int32_t exit_code;
    bool exited, reaped, watcher_done;
} child;
static child *children;            /* under the table lock */
static SigChldCallback on_child_end;

/* The unit that owns the managed signal registrations. */
void sn_signal_dispatch(int32_t code);
bool sn_signal_wanted(int32_t code);

static const dotnet_pal_kernel_ops *threads(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_KERNEL_API_SIZE, DOTNET_PAL_CAP_THREADS) && a->kernel.thread_create && a->kernel.thread_detach ? &a->kernel : NULL;
}
PALEXPORT void SystemNative_RegisterForSigChld(SigChldCallback callback) { __atomic_store_n(&on_child_end, callback, __ATOMIC_RELEASE); }

/* Under the table lock. The record goes once the code was collected AND the watcher is through with it. */
static child *detach_if_finished(child *c) {
    if (!(c->reaped && c->watcher_done)) return NULL;
    for (child **link = &children; *link; link = &(*link)->next) if (*link == c) { *link = c->next; break; }
    return c;
}
static void dispose(child *c) {
    const dotnet_pal_processes_ops *p = sn_processes();
    if (!c) return;
    if (p && c->process) (void)p->release(c->process);
    SystemNative_Free(c);
}
static void *watch(void *argument) {
    child *c = argument; const dotnet_pal_processes_ops *p = sn_processes(); int32_t code = 0;
    uint32_t status = p ? p->wait(c->process, DOTNET_PAL_INFINITE_NS, &code) : DOTNET_PAL_UNSUPPORTED;
    sn_lock();
    /* A wait that failed still ends the watch: the child counts as ended with the conventional "unknown" code. */
    c->exit_code = status == DOTNET_PAL_OK ? code : 255; c->exited = true;
    sn_unlock();
    SigChldCallback callback = __atomic_load_n(&on_child_end, __ATOMIC_ACQUIRE);
    /* The second argument lets the Process class restore the console itself unless a managed SIGCHLD registration may still cancel that. */
    if (callback) (void)callback(0, sn_signal_wanted(SIGCHLD) ? 0 : 1);
    sn_signal_dispatch(SIGCHLD);
    sn_lock();
    c->watcher_done = true;
    child *finished = detach_if_finished(c);
    sn_unlock();
    dispose(finished);
    return NULL;
}
static intptr_t pipe_descriptor(void *handle, int32_t access) {
    const dotnet_pal_processes_ops *p = sn_processes();
    sn_object *object = sn_new(SN_PIPE, handle);
    if (!object) { if (p) (void)p->pipe_close(handle); return -1; }
    object->open_flags = access;
    return sn_install(object);
}
static size_t count_texts(char *const list[]) { size_t n = 0; while (list && list[n]) n++; return n; }
/* 0 on success; -1 with errno otherwise. The descriptors are the parent's ends of the requested pipes, -1 otherwise. */
PALEXPORT int32_t SystemNative_ForkAndExecProcess(const char* filename, char* const argv[], char* const envp[], const char* cwd,
    int32_t redirectStdin, int32_t redirectStdout, int32_t redirectStderr, int32_t setCredentials, uint32_t userId, uint32_t groupId,
    uint32_t* groups, int32_t groupsLength, int32_t* childPid, int32_t* stdinFd, int32_t* stdoutFd, int32_t* stderrFd) {
    const dotnet_pal_processes_ops *p = sn_processes(); const dotnet_pal_kernel_ops *k = threads();
    const dotnet_pal_spawn_as_ops *as = sn_spawn_as();
    if (!childPid || !stdinFd || !stdoutFd || !stderrFd || !filename || !argv || groupsLength < 0 || (groupsLength > 0 && !groups)) return sn_fail(EINVAL);
    *childPid = -1; *stdinFd = -1; *stdoutFd = -1; *stderrFd = -1;
    if (!p || !k) return sn_fail(ENOTSUP);
    if (setCredentials && !as) return sn_fail(ENOTSUP); /* a child under another identity is a group of its own */
    size_t arguments = count_texts(argv);
    if (arguments == 0 || !*filename) return sn_fail(EINVAL);
    child *c = SystemNative_Calloc(1, sizeof *c);
    if (!c) return -1;
    uint32_t pipes = (redirectStdin ? DOTNET_PAL_PIPE_INPUT : 0) | (redirectStdout ? DOTNET_PAL_PIPE_OUTPUT : 0) | (redirectStderr ? DOTNET_PAL_PIPE_ERROR : 0);
    dotnet_pal_spawned spawned;
    dotnet_pal_identity identity = {userId, groupId, groupsLength > 0 ? groups : NULL, (size_t)groupsLength};
    uint32_t status = setCredentials
        ? as->spawn_as((const uint8_t*)filename, strlen(filename), (const uint8_t *const*)argv, arguments, (const uint8_t *const*)envp, envp ? count_texts(envp) : 0,
              (const uint8_t*)cwd, cwd ? strlen(cwd) : 0, pipes, &identity, &spawned, sizeof spawned)
        : p->spawn((const uint8_t*)filename, strlen(filename), (const uint8_t *const*)argv, arguments,
              (const uint8_t *const*)envp, envp ? count_texts(envp) : 0, (const uint8_t*)cwd, cwd ? strlen(cwd) : 0, pipes, &spawned, sizeof spawned);
    if (status != DOTNET_PAL_OK) { SystemNative_Free(c); return sn_fail(sn_errno(status)); }
    intptr_t in = spawned.input ? pipe_descriptor(spawned.input, PAL_O_WRONLY) : -1;
    intptr_t out = spawned.output ? pipe_descriptor(spawned.output, PAL_O_RDONLY) : -1;
    intptr_t err = spawned.error ? pipe_descriptor(spawned.error, PAL_O_RDONLY) : -1;
    void *thread = NULL;
    bool ok = spawned.id <= INT32_MAX && (!spawned.input || in >= 0) && (!spawned.output || out >= 0) && (!spawned.error || err >= 0);
    c->process = spawned.process; c->pid = (int32_t)spawned.id;
    if (ok) {
        sn_lock(); c->next = children; children = c; sn_unlock();
        ok = k->thread_create(watch, c, 0, &thread) == DOTNET_PAL_OK;
        if (!ok) { sn_lock(); c->reaped = c->watcher_done = true; (void)detach_if_finished(c); sn_unlock(); }
    }
    if (!ok) {
        /* A child nobody can watch is ended again rather than left running unobserved. */
        if (in >= 0) (void)SystemNative_Close(in);
        if (out >= 0) (void)SystemNative_Close(out);
        if (err >= 0) (void)SystemNative_Close(err);
        (void)p->terminate(c->process, 1);
        dispose(c);
        return sn_fail(EAGAIN);
    }
    (void)k->thread_detach(thread);
    *childPid = (int32_t)spawned.id; *stdinFd = (int32_t)in; *stdoutFd = (int32_t)out; *stderrFd = (int32_t)err;
    return 0;
}
/* The id of a child that has ended and was not collected yet, or 0: no child at all is "none has ended" too, as the reference has it. */
PALEXPORT int32_t SystemNative_WaitIdAnyExitedNoHangNoWait(void) {
    int32_t result = 0;
    sn_lock();
    for (child *c = children; c; c = c->next) if (!c->reaped && c->exited) { result = c->pid; break; }
    sn_unlock();
    return result;
}
/* Collects the exit code: the id when the child has ended, 0 while it runs, -1 (ECHILD) for a process that is no child of ours.
 * An id of -1 collects any child that has ended. */
PALEXPORT int32_t SystemNative_WaitPidExitedNoHang(int32_t pid, int32_t* exitCode) {
    int32_t result = -1; child *finished = NULL;
    if (!exitCode) return sn_fail(EFAULT);
    sn_lock();
    for (child *c = children; c; c = c->next) {
        if (c->reaped || (pid != -1 && c->pid != pid)) continue;
        if (!c->exited) { result = 0; if (pid != -1) break; continue; }
        *exitCode = c->exit_code; result = c->pid; c->reaped = true;
        finished = detach_if_finished(c);
        break;
    }
    sn_unlock();
    dispose(finished);
    return result < 0 ? sn_fail(ECHILD) : result;
}
/* Signal 0 asks whether the child still runs; SIGKILL ends it; the polite requests ask it to end.
 * Anything else, and any process this library did not start, is beyond what the boundary can do. */
PALEXPORT int32_t SystemNative_Kill(int32_t pid, int32_t signal) {
    const dotnet_pal_processes_ops *p = sn_processes(); uint32_t status = DOTNET_PAL_NOT_FOUND; bool known = false, polite;
    if (!p) return sn_fail(ENOTSUP);
    if (signal != 0 && signal != SIGKILL && signal != SIGTERM && signal != SIGINT && signal != SIGQUIT && signal != SIGHUP) return sn_fail(ENOTSUP);
    polite = signal != SIGKILL;
    sn_lock();
    for (child *c = children; c; c = c->next) {
        if (c->pid != pid || c->reaped) continue;
        known = true;
        /* Under the lock: the watcher cannot finish and release the handle while it is in use here. */
        if (!c->exited) status = signal == 0 ? DOTNET_PAL_OK : p->terminate(c->process, polite ? 0 : 1);
        break;
    }
    sn_unlock();
    if (!known) return sn_fail(EPERM);
    return status == DOTNET_PAL_OK ? 0 : sn_fail(status == DOTNET_PAL_NOT_FOUND ? ESRCH : sn_errno(status));
}
/* ---- reading a child's pipe without blocking --------------------------------------------------- */
/* The boundary's pipes block. The BCL reads a child's output asynchronously by switching the descriptor to
 * non-blocking and registering it with its socket engine, so a read end that is asked for that gets a reader
 * thread: it blocks in the boundary's pipe_read and fills a buffer, Read serves from the buffer (EAGAIN when it
 * is empty), and the event port learns about new bytes through sn_port_notify. Once the reader exists every
 * read of that pipe goes through the buffer, blocking ones included. The reader pins the pipe, so a pipe that
 * is closed early stays open underneath until the child writes or closes its end, which is when a blocked
 * pipe_read returns. */
#define READER_CAPACITY (64u * 1024u)
struct sn_pipe_reader {
    uint8_t *data; size_t head, count;
    bool ended, closing; int32_t error;
    void *room, *arrived; /* auto-reset events: the reader waits for room, a blocking Read for bytes */
};
static const dotnet_pal_kernel_ops *pipe_kernel(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_KERNEL_API_SIZE, DOTNET_PAL_CAP_THREADS | DOTNET_PAL_CAP_EVENTS) ? &a->kernel : NULL;
}
bool sn_pipe_readable(const sn_object *object) { return object->reader && (object->reader->count > 0 || object->reader->ended); }
static void *pipe_reader(void *argument) {
    sn_object *object = argument; struct sn_pipe_reader *r = object->reader;
    const dotnet_pal_processes_ops *p = sn_processes(); const dotnet_pal_kernel_ops *k = pipe_kernel();
    uint8_t chunk[4096];
    for (;;) {
        sn_lock();
        size_t room = READER_CAPACITY - r->count; bool closing = r->closing;
        sn_unlock();
        if (closing) break;
        if (room == 0) { (void)k->event_wait(r->room, DOTNET_PAL_INFINITE_NS); continue; }
        size_t got = 0;
        uint32_t status = p->pipe_read(object->handle, chunk, room < sizeof chunk ? room : sizeof chunk, &got);
        sn_lock();
        if (status != DOTNET_PAL_OK) { r->error = sn_errno(status); r->ended = true; }
        else if (got == 0) r->ended = true;
        else for (size_t i = 0; i < got; ++i) r->data[(r->head + r->count++) % READER_CAPACITY] = chunk[i];
        bool ended = r->ended;
        sn_unlock();
        (void)k->event_set(r->arrived);
        sn_port_notify(object);
        if (ended) break;
    }
    sn_unpin(object);
    return NULL;
}
/* Starts the reader once. False when the target has no threads or events, or no memory. */
bool sn_pipe_watch(sn_object *object) {
    const dotnet_pal_kernel_ops *k = pipe_kernel(); void *thread = NULL;
    sn_lock();
    bool have = object->reader != NULL;
    sn_unlock();
    if (have) return true;
    if (!k || !sn_processes()) return false;
    struct sn_pipe_reader *r = SystemNative_Calloc(1, sizeof *r);
    if (r) r->data = SystemNative_Malloc(READER_CAPACITY);
    bool ok = r && r->data && k->event_create(0, 0, &r->room) == DOTNET_PAL_OK && k->event_create(0, 0, &r->arrived) == DOTNET_PAL_OK;
    if (ok) {
        sn_lock();
        if (object->reader) { sn_unlock(); ok = false; have = true; }   /* another thread was first */
        else { object->reader = r; object->references++; sn_unlock(); }  /* the reader's pin */
    }
    if (ok && k->thread_create(pipe_reader, object, 0, &thread) == DOTNET_PAL_OK) { (void)k->thread_detach(thread); return true; }
    if (ok) { sn_lock(); object->reader = NULL; object->references--; sn_unlock(); }
    if (r) { if (r->room) (void)k->event_destroy(r->room); if (r->arrived) (void)k->event_destroy(r->arrived); SystemNative_Free(r->data); SystemNative_Free(r); }
    return have;
}
int32_t sn_pipe_read(sn_object *object, void *buffer, int32_t size) {
    const dotnet_pal_processes_ops *p = sn_processes(); const dotnet_pal_kernel_ops *k = pipe_kernel(); size_t done = 0;
    sn_lock();
    struct sn_pipe_reader *r = object->reader;
    sn_unlock();
    if (!r) {
        if (object->nonblocking && !sn_pipe_watch(object)) return sn_fail(ENOTSUP);
        if (!object->nonblocking) {
            uint32_t status = p ? p->pipe_read(object->handle, buffer, (size_t)size, &done) : DOTNET_PAL_UNSUPPORTED;
            return status == DOTNET_PAL_OK ? (int32_t)done : sn_fail(sn_errno(status));
        }
        sn_lock(); r = object->reader; sn_unlock();
    }
    for (;;) {
        sn_lock();
        size_t take = r->count < (size_t)size ? r->count : (size_t)size;
        for (size_t i = 0; i < take; ++i) ((uint8_t*)buffer)[i] = r->data[(r->head + i) % READER_CAPACITY];
        r->head = (r->head + take) % READER_CAPACITY; r->count -= take;
        bool ended = r->ended; int32_t error = r->error;
        sn_unlock();
        if (take > 0) { (void)k->event_set(r->room); return (int32_t)take; }
        if (ended) return error ? sn_fail(error) : 0;
        if (object->nonblocking) { sn_socket_would_block(object, 1 /* SocketEvents_SA_READ */); return sn_fail(EAGAIN); }
        (void)k->event_wait(r->arrived, DOTNET_PAL_INFINITE_NS);
    }
}
/* Under the table lock, when the last descriptor closes: a reader waiting for room is told to stop. */
void sn_pipe_closing(sn_object *object) {
    const dotnet_pal_kernel_ops *k = pipe_kernel();
    if (!object->reader) return;
    object->reader->closing = true;
    if (k) (void)k->event_set(object->reader->room);
}
void sn_pipe_destroy(sn_object *object) {
    const dotnet_pal_kernel_ops *k = pipe_kernel(); struct sn_pipe_reader *r = object->reader;
    if (!r) return;
    if (k) { (void)k->event_destroy(r->room); (void)k->event_destroy(r->arrived); }
    SystemNative_Free(r->data); SystemNative_Free(r);
    object->reader = NULL;
}

/* A pipe between two ends of this same process is not something the boundary makes. */
PALEXPORT int32_t SystemNative_Pipe(int32_t pipefd[2], int32_t flags) { (void)flags; if (pipefd) { pipefd[0] = -1; pipefd[1] = -1; } return sn_fail(ENOTSUP); }
