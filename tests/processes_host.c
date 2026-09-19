/* Independent POSIX reference provider for the host-processes conformance suite:
 * fork and exec, with a close-on-exec pipe that carries the errno of a failed
 * chdir or exec back to the parent, and waitpid at intervals. Fault 1 withholds
 * a callback; fault 2 breaks the output contracts so the front end's sanitizing
 * is observable. A pipe handle is the descriptor plus one.
 * pal_processes_host_start is the whole of a start. tests/spawn_as_host.c calls
 * it with an identity, so that its children are children of this provider. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
extern char **environ;
int pal_processes_fault;
typedef struct { pthread_mutex_t lock; pid_t pid; int ended; int32_t code; } child_t;
static uint32_t failure(int code) {
    switch (code) {
    case ENOENT: return DOTNET_PAL_NOT_FOUND;
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case EISDIR: return DOTNET_PAL_IS_DIRECTORY;
    case ENOTDIR: return DOTNET_PAL_NOT_DIRECTORY;
    case ENAMETOOLONG: return DOTNET_PAL_NAME_TOO_LONG;
    case EMFILE: case ENFILE: return DOTNET_PAL_TOO_MANY_HANDLES;
    case ENOMEM: case EAGAIN: return DOTNET_PAL_OUT_OF_MEMORY;
    case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
/* Paths are byte borrows without a terminator. */
static int terminated(char *out, const uint8_t *path, size_t length) {
    if (length >= PATH_MAX) return 0;
    memcpy(out, path, length); out[length] = 0; return 1;
}
static int descriptor(void *pipe) { return (int)(intptr_t)pipe - 1; }
static uint64_t now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * UINT64_C(1000000000) + (uint64_t)t.tv_nsec; }
/* A pipe whose ends are close-on-exec and out of the range the child's dup2 calls write to. */
static int channel(int ends[2]) {
    if (pipe2(ends, O_CLOEXEC) != 0) return 0;
    for (int i = 0; i < 2; ++i) {
        if (ends[i] > 2) continue;
        int moved = fcntl(ends[i], F_DUPFD_CLOEXEC, 3);
        if (moved < 0) { int code = errno; close(ends[0]); close(ends[1]); errno = code; return 0; }
        close(ends[i]); ends[i] = moved;
    }
    return 1;
}
/* Fault 2: one broken answer per call, each of them a result a consumer could not act on. */
static uint32_t breach(uint32_t pipes, dotnet_pal_spawned *out) {
    static int object, call;
    out->process = &object; out->id = 1;
    out->input = pipes & DOTNET_PAL_PIPE_INPUT ? &object : NULL; out->output = pipes & DOTNET_PAL_PIPE_OUTPUT ? &object : NULL;
    out->error = pipes & DOTNET_PAL_PIPE_ERROR ? &object : NULL;
    switch (call++) {
    case 0: out->process = NULL; return DOTNET_PAL_OK;  /* success without a handle */
    case 1: out->id = 0; return DOTNET_PAL_OK;          /* without an identifier */
    case 2: out->error = &object; return DOTNET_PAL_OK; /* with a pipe nobody asked for */
    case 3: out->output = NULL; return DOTNET_PAL_OK;   /* without a pipe that was asked for */
    default: return DOTNET_PAL_WOULD_BLOCK;             /* a status spawn does not have */
    }
}
/* Groups, group, user, in the forked child. As the runtime's own System.Native: a process without the privilege to set
 * groups may still start a child as its own user, so the refusal stands only when this process holds a group the list lacks. */
static int assume(const dotnet_pal_identity *identity, gid_t *held) {
    if (setgroups(identity->group_count, (const gid_t*)identity->groups) != 0) {
        if (errno != EPERM) return 0;
        int count = getgroups((int)identity->group_count, held); /* fails when this process holds more groups than the list names */
        int listed = count >= 0;
        for (int i = 0; listed && i < count; ++i) {
            size_t at = 0;
            while (at < identity->group_count && identity->groups[at] != held[i]) ++at;
            listed = at < identity->group_count;
        }
        if (!listed) { errno = EPERM; return 0; }
    }
    return setgid(identity->group_id) == 0 && setuid(identity->user_id) == 0;
}
uint32_t pal_processes_host_start(const uint8_t *program, size_t program_length, const uint8_t *const *arguments, size_t argument_count,
    const uint8_t *const *environment, size_t environment_count, const uint8_t *directory, size_t directory_length, uint32_t pipes,
    const dotnet_pal_identity *identity, dotnet_pal_spawned *out) {
    char path[PATH_MAX], home[PATH_MAX];
    if (!terminated(path, program, program_length) || (directory && !terminated(home, directory, directory_length))) return DOTNET_PAL_NAME_TOO_LONG;
    /* The vectors are counted; exec wants them terminated. */
    char **argv = calloc(argument_count + 1, sizeof *argv), **envp = environment ? calloc(environment_count + 1, sizeof *envp) : environ;
    child_t *child = calloc(1, sizeof *child);
    gid_t *held = calloc(identity && identity->group_count ? identity->group_count : 1, sizeof *held); /* the forked child may not allocate */
    int ends[4][2] = {{-1, -1}, {-1, -1}, {-1, -1}, {-1, -1}}, code = 0; /* per stream {read, write}; the last one reports */
    if (!argv || !envp || !child || !held) code = ENOMEM;
    for (int i = 0; i < 4 && code == 0; ++i) if ((i == 3 || (pipes & (1u << i))) && !channel(ends[i])) code = errno;
    pid_t pid = -1;
    if (code == 0) {
        memcpy(argv, arguments, argument_count * sizeof *argv);
        if (environment) memcpy(envp, environment, environment_count * sizeof *envp);
        /* No handler of this process may run in the child before it has its own image. */
        sigset_t all, before;
        sigfillset(&all); pthread_sigmask(SIG_SETMASK, &all, &before);
        pid = fork();
        if (pid == 0) {
            /* Only async-signal-safe calls from here. The child starts with default dispositions and nothing blocked. */
            struct sigaction standard; memset(&standard, 0, sizeof standard); standard.sa_handler = SIG_DFL;
            for (int number = 1; number < NSIG; ++number) sigaction(number, &standard, NULL);
            sigset_t none; sigemptyset(&none); sigprocmask(SIG_SETMASK, &none, NULL);
            for (int i = 0; i < 3; ++i) if (ends[i][0] >= 0 && dup2(ends[i][i == 0 ? 0 : 1], i) < 0) goto failed;
            /* The directory is entered as the new user. */
            if (identity && !assume(identity, held)) goto failed;
            if (directory && chdir(home) != 0) goto failed;
            execve(path, argv, envp);
        failed:
            code = errno;
            if (write(ends[3][1], &code, sizeof code) < 0) _exit(126);
            _exit(127);
        }
        if (pid < 0) code = errno;
        pthread_sigmask(SIG_SETMASK, &before, NULL);
    }
    if (pid > 0) {
        /* End of file on the report pipe is a successful exec: close-on-exec closed the child's end. */
        close(ends[3][1]); ends[3][1] = -1;
        ssize_t n; do n = read(ends[3][0], &code, sizeof code); while (n < 0 && errno == EINTR);
        if (n != (ssize_t)sizeof code) code = 0;
        else { int status; while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {} }
    }
    free(argv); free(held); if (environment) free(envp);
    for (int i = 0; i < 4; ++i) {
        /* The parent keeps the write end of the input and the read end of the others, and only when the child runs. */
        int kept = i == 3 || code != 0 ? -1 : i == 0 ? 1 : 0;
        for (int end = 0; end < 2; ++end) if (end != kept && ends[i][end] >= 0) close(ends[i][end]);
    }
    if (code != 0) { free(child); return failure(code); }
    pthread_mutex_init(&child->lock, NULL); child->pid = pid;
    out->process = child; out->id = (uint64_t)pid;
    if (pipes & DOTNET_PAL_PIPE_INPUT) out->input = (void*)(intptr_t)(ends[0][1] + 1);
    if (pipes & DOTNET_PAL_PIPE_OUTPUT) out->output = (void*)(intptr_t)(ends[1][0] + 1);
    if (pipes & DOTNET_PAL_PIPE_ERROR) out->error = (void*)(intptr_t)(ends[2][0] + 1);
    return DOTNET_PAL_OK;
}
static uint32_t process_spawn(const uint8_t *program, size_t program_length, const uint8_t *const *arguments, size_t argument_count,
    const uint8_t *const *environment, size_t environment_count, const uint8_t *directory, size_t directory_length, uint32_t pipes,
    dotnet_pal_spawned *out, size_t size) {
    (void)size;
    if (pal_processes_fault == 2) return breach(pipes, out);
    return pal_processes_host_start(program, program_length, arguments, argument_count, environment, environment_count, directory, directory_length, pipes, NULL, out);
}
/* One look under the lock: the only place that reaps, so the identifier is this child's for as long as `ended` is 0. */
static int look(child_t *child) {
    int status; pid_t pid;
    if (child->ended) return 1;
    do pid = waitpid(child->pid, &status, WNOHANG); while (pid < 0 && errno == EINTR);
    if (pid == 0) return 0;
    child->ended = pid < 0 ? 2 : 1; /* 2: somebody else reaped it and the code is lost */
    if (pid > 0) child->code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    return 1;
}
static uint32_t process_wait(void *process, uint64_t timeout_ns, int32_t *exit_code) {
    if (pal_processes_fault == 2) { *exit_code = 99; return DOTNET_PAL_BROKEN_PIPE; } /* a status wait does not have */
    child_t *child = process;
    uint64_t deadline = timeout_ns == DOTNET_PAL_INFINITE_NS || timeout_ns == 0 ? timeout_ns : now() + timeout_ns;
    for (;;) {
        pthread_mutex_lock(&child->lock);
        int state = look(child) ? child->ended : 0; int32_t code = child->code;
        pthread_mutex_unlock(&child->lock);
        if (state == 1) { *exit_code = code; return DOTNET_PAL_OK; }
        if (state == 2) return DOTNET_PAL_OS_ERROR;
        if (deadline != DOTNET_PAL_INFINITE_NS && now() >= deadline) return DOTNET_PAL_TIMEOUT;
        struct timespec pause = {0, 1000000};
        nanosleep(&pause, NULL);
    }
}
static uint32_t process_terminate(void *process, uint32_t forceful) {
    if (pal_processes_fault == 2) return DOTNET_PAL_TIMEOUT; /* nor does terminate have this one */
    child_t *child = process;
    pthread_mutex_lock(&child->lock);
    uint32_t status = look(child) ? DOTNET_PAL_NOT_FOUND : kill(child->pid, forceful ? SIGKILL : SIGTERM) == 0 ? DOTNET_PAL_OK
        : errno == ESRCH ? DOTNET_PAL_NOT_FOUND : failure(errno);
    pthread_mutex_unlock(&child->lock);
    return status;
}
static uint32_t process_release(void *process) {
    if (pal_processes_fault == 2) return DOTNET_PAL_BROKEN_PIPE;
    child_t *child = process;
    look(child); /* a child that has ended leaves no zombie; one that runs is left alone */
    pthread_mutex_destroy(&child->lock); free(child);
    return DOTNET_PAL_OK;
}
static uint32_t pipe_read(void *pipe, uint8_t *data, size_t capacity, size_t *got) {
    if (pal_processes_fault == 2) { *got = capacity + 1; return DOTNET_PAL_OK; } /* more than the buffer holds */
    for (;;) {
        ssize_t n = read(descriptor(pipe), data, capacity);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) return errno == EAGAIN ? DOTNET_PAL_WOULD_BLOCK : DOTNET_PAL_OS_ERROR;
        *got = (size_t)n; return DOTNET_PAL_OK;
    }
}
static uint32_t pipe_write(void *pipe, const uint8_t *data, size_t size, size_t *written) {
    static int call;
    if (pal_processes_fault == 2) { *written = call++ ? size + 1 : 0; return DOTNET_PAL_OK; } /* no progress, then more than was offered */
    /* SIGPIPE is kept from this thread while it writes and the one a failed write raised is taken back,
     * unless one was pending before: that one belongs to the caller. */
    sigset_t only, before, pending;
    sigemptyset(&only); sigaddset(&only, SIGPIPE);
    pthread_sigmask(SIG_BLOCK, &only, &before);
    int foreign = sigpending(&pending) == 0 && sigismember(&pending, SIGPIPE);
    ssize_t n; do n = write(descriptor(pipe), data, size); while (n < 0 && errno == EINTR);
    int code = n < 0 ? errno : 0;
    if (code == EPIPE && !foreign) { struct timespec none = {0, 0}; while (sigtimedwait(&only, NULL, &none) < 0 && errno == EINTR) {} }
    pthread_sigmask(SIG_SETMASK, &before, NULL);
    if (n < 0) return code == EPIPE ? DOTNET_PAL_BROKEN_PIPE : code == EAGAIN ? DOTNET_PAL_WOULD_BLOCK : DOTNET_PAL_OS_ERROR;
    *written = (size_t)n; return DOTNET_PAL_OK;
}
static uint32_t pipe_close(void *pipe) {
    if (pal_processes_fault == 2) return DOTNET_PAL_NOT_FOUND;
    return close(descriptor(pipe)) == 0 || errno == EINTR ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static const dotnet_pal_host_processes table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_processes), DOTNET_PAL_CAP_PROCESSES},
    {process_spawn, process_wait, process_terminate, process_release, pipe_read, pipe_write, pipe_close, NULL},
};
static const dotnet_pal_host_processes malformed = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_processes), DOTNET_PAL_CAP_PROCESSES},
    {process_spawn, process_wait, process_terminate, process_release, pipe_read, NULL, pipe_close, NULL},
};
const dotnet_pal_host_processes *dotnet_pal_host_processes_v2(void) { return pal_processes_fault == 1 ? &malformed : &table; }
