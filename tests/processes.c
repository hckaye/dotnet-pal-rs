/* Conformance test of the processes group on Linux: real children (sh, cat, sleep,
 * env) started through the boundary and checked against what the kernel reports
 * about them. With -DPAL_HOST_TEST the same run goes against the C host table;
 * faults 1 and 2 check rejection and sanitizing. The Linux build takes "nopidfd"
 * to run with pidfd_open refused, as on a kernel before 5.3. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_processes_fault;
#else
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <sys/prctl.h>
#endif
enum { IN = DOTNET_PAL_PIPE_INPUT, OUT = DOTNET_PAL_PIPE_OUTPUT, ERR = DOTNET_PAL_PIPE_ERROR };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define FOREVER DOTNET_PAL_INFINITE_NS
#define MS UINT64_C(1000000)
#define TEXT(text) (const uint8_t*)(text), strlen(text)
static const dotnet_pal_processes_ops *p;
static unsigned children, pipes_made, refused;
static uint64_t now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * UINT64_C(1000000000) + (uint64_t)t.tv_nsec; }
static int empty(const dotnet_pal_spawned *s) { return !s->process && s->id == 0 && !s->input && !s->output && !s->error; }
/* Open descriptors of this process. */
static int descriptors(void) {
    DIR *list = opendir("/proc/self/fd"); assert(list);
    int count = 0;
    for (struct dirent *e; (e = readdir(list));) if (e->d_name[0] != '.' && atoi(e->d_name) != dirfd(list)) ++count;
    closedir(list);
    return count;
}
static void no_children(void) { errno = 0; assert(waitpid(-1, NULL, WNOHANG) == -1 && errno == ECHILD); }
/* Starts a program from C strings. What the boundary takes is counted, not terminated: the paths are followed by
 * other bytes and the vectors by one more text, which a provider that read on would hand to the child. */
static uint32_t start(const char *program, const char *const *argv, const char *const *envp, const char *directory, uint32_t pipes, dotnet_pal_spawned *out) {
    const uint8_t *arguments[16], *environment[16]; size_t argc = 0, envc = 0;
    uint8_t path[PATH_MAX], home[PATH_MAX];
    memset(path, 'X', sizeof path); memcpy(path, program, strlen(program));
    memset(home, 'X', sizeof home); if (directory) memcpy(home, directory, strlen(directory));
    for (; argv[argc]; ++argc) arguments[argc] = (const uint8_t*)argv[argc];
    arguments[argc] = (const uint8_t*)"OVERRUN";
    for (; envp && envp[envc]; ++envc) environment[envc] = (const uint8_t*)envp[envc];
    environment[envc] = (const uint8_t*)"OVERRUN=1";
    memset(out, 0x55, sizeof *out);
    uint32_t status = p->spawn(path, strlen(program), arguments, argc, envp ? environment : NULL, envc, directory ? home : NULL,
        directory ? strlen(directory) : 0, pipes, out, sizeof *out);
    if (status != 0) { ++refused; assert(empty(out)); return status; }
    /* A handle, the kernel's identifier and exactly the pipes that were asked for. */
    assert(out->process && out->id != 0 && kill((pid_t)out->id, 0) == 0);
    assert(!out->input == !(pipes & IN) && !out->output == !(pipes & OUT) && !out->error == !(pipes & ERR));
    ++children; pipes_made += !!(pipes & IN) + !!(pipes & OUT) + !!(pipes & ERR);
    return 0;
}
/* Reads a pipe to its end, which is zero bytes with OK, as often as it is asked. */
static size_t drain(void *pipe, char *buffer, size_t capacity) {
    size_t total = 0, got = 7;
    for (;;) {
        assert(total + 1 < capacity);
        assert(p->pipe_read(pipe, (uint8_t*)buffer + total, capacity - 1 - total, &got) == 0 && got <= capacity - 1 - total);
        if (got == 0) break;
        total += got;
    }
    assert(p->pipe_read(pipe, (uint8_t*)buffer + total, capacity - 1 - total, &got) == 0 && got == 0);
    buffer[total] = 0;
    return total;
}
static void feed(void *pipe, const void *data, size_t size) {
    size_t done = 0, written = 7;
    while (done < size) {
        assert(p->pipe_write(pipe, (const uint8_t*)data + done, size - done, &written) == 0 && written > 0 && written <= size - done);
        done += written;
    }
}
/* Closes the pipes, waits for the end, gives the handle back. */
static int32_t finish(dotnet_pal_spawned *c) {
    int32_t code = 7;
    if (c->input) assert(p->pipe_close(c->input) == 0);
    if (c->output) assert(p->pipe_close(c->output) == 0);
    if (c->error) assert(p->pipe_close(c->error) == 0);
    assert(p->wait(c->process, FOREVER, &code) == 0 && p->release(c->process) == 0);
    return code;
}
/* Runs a program to its end and keeps what it wrote to its output. */
static int32_t run(const char *program, const char *const *argv, const char *const *envp, const char *directory, char *output, size_t capacity) {
    dotnet_pal_spawned c;
    assert(start(program, argv, envp, directory, OUT, &c) == 0);
    drain(c.output, output, capacity);
    return finish(&c);
}
static int32_t shell(const char *script, char *output, size_t capacity) {
    const char *argv[] = {"sh", "-c", script, NULL};
    return run("/bin/sh", argv, NULL, NULL, output, capacity);
}

static void codes_and_arguments(void) {
    char text[256];
    assert(shell("exit 0", text, sizeof text) == 0 && shell("exit 7", text, sizeof text) == 7 && shell("exit 255", text, sizeof text) == 255);
    /* Argument 0 is what the program sees as its own name, whatever file was started. */
    const char *renamed[] = {"another name", "-c", "echo \"$0\"", NULL};
    assert(run("/bin/sh", renamed, NULL, NULL, text, sizeof text) == 0 && strcmp(text, "another name\n") == 0);
    /* Arguments arrive as they are: with spaces, empty, and no more of them than were counted. */
    const char *words[] = {"sh", "-c", "for a; do printf '[%s]' \"$a\"; done; echo $#", "zero", "two words", "", "tab\there", "*", NULL};
    assert(run("/bin/sh", words, NULL, NULL, text, sizeof text) == 0 && strcmp(text, "[two words][][tab\there][*]4\n") == 0);
    dotnet_pal_spawned c;
    const char *self[] = {"sh", "-c", "echo $$", NULL};
    assert(start("/bin/sh", self, NULL, NULL, OUT, &c) == 0);
    drain(c.output, text, sizeof text);
    assert(strtoull(text, NULL, 10) == c.id && finish(&c) == 0);
}
static void environment_and_directory(void) {
    char text[8192];
    assert(setenv("PAL_PARENT", "inherited", 1) == 0);
    /* A given environment is the whole environment. */
    const char *env[] = {"env", NULL}, *given[] = {"PAL_GIVEN=yes", "PAL_EMPTY=", "PAL_SPACED=a b=c", NULL}, *nothing[] = {NULL};
    assert(run("/usr/bin/env", env, given, NULL, text, sizeof text) == 0 && strcmp(text, "PAL_GIVEN=yes\nPAL_EMPTY=\nPAL_SPACED=a b=c\n") == 0);
    assert(run("/usr/bin/env", env, nothing, NULL, text, sizeof text) == 0 && strcmp(text, "") == 0);
    /* None given is the parent's, as it is now. */
    assert(run("/usr/bin/env", env, NULL, NULL, text, sizeof text) == 0 && strstr(text, "PAL_PARENT=inherited\n") && !strstr(text, "PAL_GIVEN"));
    char here[PATH_MAX], scratch[] = "/tmp/pal-processes-XXXXXX";
    assert(getcwd(here, sizeof here) && mkdtemp(scratch));
    const char *where[] = {"sh", "-c", "pwd -P", NULL};
    assert(run("/bin/sh", where, NULL, scratch, text, sizeof text) == 0 && strncmp(text, scratch, strlen(scratch)) == 0 && text[strlen(scratch)] == '\n');
    assert(run("/bin/sh", where, NULL, NULL, text, sizeof text) == 0 && strncmp(text, here, strlen(here)) == 0 && text[strlen(here)] == '\n');
    char after[PATH_MAX];
    assert(getcwd(after, sizeof after) && strcmp(here, after) == 0 && rmdir(scratch) == 0); /* the parent stayed where it was */
}

struct waiter { void *process; uint64_t timeout; uint32_t status; int32_t code; };
static void *waiter(void *arg) {
    struct waiter *w = arg;
    w->code = 7; w->status = p->wait(w->process, w->timeout, &w->code);
    return NULL;
}
struct feeder { void *pipe; const uint8_t *data; size_t size; };
static void *feeder(void *arg) {
    struct feeder *f = arg;
    feed(f->pipe, f->data, f->size);
    assert(p->pipe_close(f->pipe) == 0);
    return NULL;
}
static int same_mask(const sigset_t *a, const sigset_t *b) {
    for (int number = 1; number < NSIG; ++number) if (sigismember(a, number) != sigismember(b, number)) return 0;
    return 1;
}
static void transfers(void) {
    dotnet_pal_spawned c; char text[256], errors[256]; size_t written = 7; int32_t code = 7;
    /* All three: the child reads what the parent writes until the parent closes, and its two outputs stay apart. */
    const char *both[] = {"sh", "-c", "cat; echo problem >&2", NULL};
    assert(start("/bin/sh", both, NULL, NULL, IN | OUT | ERR, &c) == 0);
    feed(c.input, "to the child\n", 13);
    assert(p->pipe_close(c.input) == 0); c.input = NULL;
    assert(drain(c.output, text, sizeof text) == 13 && strcmp(text, "to the child\n") == 0);
    assert(drain(c.error, errors, sizeof errors) == 8 && strcmp(errors, "problem\n") == 0);
    assert(finish(&c) == 0);
    /* A mebibyte through cat: more than any pipe holds, so writer and reader have to run at the same time. A third
     * thread waits for the child's end meanwhile, as the consumer's watcher does. */
    enum { LARGE = 1 << 20 };
    uint8_t *sent = malloc(LARGE), *back = malloc(LARGE + 1); assert(sent && back);
    for (size_t i = 0; i < LARGE; ++i) sent[i] = (uint8_t)(i * 31 + (i >> 8));
    const char *cat[] = {"cat", NULL};
    assert(start("/bin/cat", cat, NULL, NULL, IN | OUT, &c) == 0);
    struct feeder job = {c.input, sent, LARGE}; struct waiter watcher = {c.process, FOREVER, 7, 7}; pthread_t thread, watching;
    assert(pthread_create(&thread, NULL, feeder, &job) == 0 && pthread_create(&watching, NULL, waiter, &watcher) == 0);
    size_t total = 0, got = 7;
    while (p->pipe_read(c.output, back + total, LARGE + 1 - total, &got) == 0 && got != 0) total += got;
    assert(pthread_join(thread, NULL) == 0 && pthread_join(watching, NULL) == 0); c.input = NULL;
    assert(total == LARGE && memcmp(sent, back, LARGE) == 0 && watcher.status == 0 && watcher.code == 0 && finish(&c) == 0);
    free(sent); free(back);
    /* The reader is gone: the write says so, and the default action of SIGPIPE does not end this process. Nothing
     * of it is left on the thread either. */
    const char *leave[] = {"sh", "-c", "exit 3", NULL};
    assert(start("/bin/sh", leave, NULL, NULL, IN, &c) == 0);
    assert(p->wait(c.process, FOREVER, &code) == 0 && code == 3);
    sigset_t before, after, pending, only;
    assert(pthread_sigmask(SIG_SETMASK, NULL, &before) == 0 && !sigismember(&before, SIGPIPE));
    assert(p->pipe_write(c.input, TEXT("x"), &written) == DOTNET_PAL_BROKEN_PIPE && written == 0);
    assert(pthread_sigmask(SIG_SETMASK, NULL, &after) == 0 && same_mask(&before, &after) && sigpending(&pending) == 0 && !sigismember(&pending, SIGPIPE));
    /* One that was pending behind the caller's own block is the caller's: it is still there afterwards. */
    sigemptyset(&only); sigaddset(&only, SIGPIPE);
    assert(pthread_sigmask(SIG_BLOCK, &only, NULL) == 0 && raise(SIGPIPE) == 0);
    assert(p->pipe_write(c.input, TEXT("x"), &written) == DOTNET_PAL_BROKEN_PIPE && written == 0);
    struct timespec none = {0, 0};
    assert(sigpending(&pending) == 0 && sigismember(&pending, SIGPIPE) && sigtimedwait(&only, NULL, &none) == SIGPIPE);
    assert(pthread_sigmask(SIG_SETMASK, &before, NULL) == 0 && finish(&c) == 3);
    /* A stream that is not a pipe is the parent's own: the child writes to this process's output and reads its input. */
    int captured[2], supplied[2];
    assert(pipe(captured) == 0 && pipe(supplied) == 0);
    fflush(stdout);
    int saved_out = dup(STDOUT_FILENO), saved_in = dup(STDIN_FILENO);
    assert(saved_out >= 0 && saved_in >= 0 && dup2(captured[1], STDOUT_FILENO) >= 0 && dup2(supplied[0], STDIN_FILENO) >= 0);
    assert(write(supplied[1], "from the parent's input\n", 24) == 24);
    close(supplied[1]); close(supplied[0]); close(captured[1]);
    uint32_t status = start("/bin/cat", cat, NULL, NULL, 0, &c);
    assert(dup2(saved_out, STDOUT_FILENO) >= 0 && dup2(saved_in, STDIN_FILENO) >= 0);
    close(saved_out); close(saved_in);
    assert(status == 0 && finish(&c) == 0);
    ssize_t n = read(captured[0], text, sizeof text - 1);
    assert(n == 24 && memcmp(text, "from the parent's input\n", 24) == 0 && read(captured[0], text, sizeof text) == 0);
    close(captured[0]);
}

static void waiting(void) {
    dotnet_pal_spawned c; int32_t code = 7; char text[64]; size_t got = 7;
    const char *nap[] = {"sleep", "5", NULL};
    assert(start("/bin/sleep", nap, NULL, NULL, 0, &c) == 0);
    assert(p->wait(c.process, 0, &code) == DOTNET_PAL_TIMEOUT && code == 0);
    uint64_t begin = now();
    code = 7;
    assert(p->wait(c.process, 100 * MS, &code) == DOTNET_PAL_TIMEOUT && code == 0);
    uint64_t waited = now() - begin;
    assert(waited >= 100 * MS && waited < 2000 * MS && kill((pid_t)c.id, 0) == 0);
    /* Asked to end, the child ends by the signal; every later wait says the same at once, and there is nothing left to end. */
    assert(p->terminate(c.process, 0) == 0);
    assert(p->wait(c.process, FOREVER, &code) == 0 && code == 128 + SIGTERM);
    code = 7; assert(p->wait(c.process, 0, &code) == 0 && code == 128 + SIGTERM);
    code = 7; assert(p->wait(c.process, FOREVER, &code) == 0 && code == 128 + SIGTERM);
    assert(p->terminate(c.process, 0) == DOTNET_PAL_NOT_FOUND && p->terminate(c.process, 1) == DOTNET_PAL_NOT_FOUND);
    assert(p->release(c.process) == 0);
    /* A child that ignores the request runs on until it is ended. */
    const char *stubborn[] = {"sh", "-c", "trap '' TERM; echo ready; exec sleep 5", NULL};
    assert(start("/bin/sh", stubborn, NULL, NULL, OUT, &c) == 0);
    assert(p->pipe_read(c.output, (uint8_t*)text, sizeof text, &got) == 0 && got == 6 && memcmp(text, "ready\n", 6) == 0);
    assert(p->terminate(c.process, 0) == 0 && p->wait(c.process, 200 * MS, &code) == DOTNET_PAL_TIMEOUT);
    begin = now();
    assert(p->terminate(c.process, 1) == 0 && p->wait(c.process, FOREVER, &code) == 0 && code == 128 + SIGKILL && now() - begin < 2000 * MS);
    assert(finish(&c) == 128 + SIGKILL);
    /* Several waits on one child, with and without a limit, all learn the code. */
    const char *brief[] = {"sh", "-c", "sleep 0.3; exit 9", NULL};
    assert(start("/bin/sh", brief, NULL, NULL, 0, &c) == 0);
    struct waiter first = {c.process, FOREVER, 7, 7}, second = {c.process, FOREVER, 7, 7}, third = {c.process, 5000 * MS, 7, 7}; pthread_t threads[3];
    assert(pthread_create(&threads[0], NULL, waiter, &first) == 0 && pthread_create(&threads[1], NULL, waiter, &second) == 0 && pthread_create(&threads[2], NULL, waiter, &third) == 0);
    code = 7; assert(p->wait(c.process, 5000 * MS, &code) == 0 && code == 9);
    for (int i = 0; i < 3; ++i) assert(pthread_join(threads[i], NULL) == 0);
    assert(first.status == 0 && first.code == 9 && second.status == 0 && second.code == 9 && third.status == 0 && third.code == 9 && finish(&c) == 9);
    /* A thread that waits without a limit, as the consumer's watcher does, does not keep another one from ending the child. */
    assert(start("/bin/sleep", nap, NULL, NULL, 0, &c) == 0);
    struct waiter watcher = {c.process, FOREVER, 7, 7};
    assert(pthread_create(&threads[0], NULL, waiter, &watcher) == 0);
    struct timespec pause = {0, 50000000}; nanosleep(&pause, NULL);
    begin = now();
    assert(p->terminate(c.process, 1) == 0 && pthread_join(threads[0], NULL) == 0 && now() - begin < 2000 * MS);
    assert(watcher.status == 0 && watcher.code == 128 + SIGKILL && finish(&c) == 128 + SIGKILL);
}

static const char *failures(void) {
    dotnet_pal_spawned c; const char *argv[] = {"program", NULL};
    int opened = descriptors();
    assert(start("/nonexistent/program", argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    assert(start("/bin/sh/program", argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_DIRECTORY);
    /* The path is used as given: a bare name is a file in the working directory, not a search of PATH. */
    const char *shell_argv[] = {"sh", "-c", "exit 0", NULL};
    assert(access("sh", F_OK) != 0 && start("sh", shell_argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    /* The kernel refuses to execute a directory with EACCES, not EISDIR. */
    uint32_t directory = start("/bin", argv, NULL, NULL, IN | OUT | ERR, &c);
    assert(directory == DOTNET_PAL_ACCESS_DENIED || directory == DOTNET_PAL_IS_DIRECTORY);
    char plain[] = "/tmp/pal-processes-XXXXXX";
    int file = mkstemp(plain);
    assert(file >= 0 && write(file, "#!/bin/sh\nexit 0\n", 17) == 17 && fchmod(file, 0644) == 0 && close(file) == 0);
    assert(start(plain, argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_ACCESS_DENIED);
    assert(chmod(plain, 0755) == 0 && start(plain, argv, NULL, NULL, 0, &c) == 0 && finish(&c) == 0);
    assert(start("/bin/sh", shell_argv, NULL, "/nonexistent/directory", IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    assert(start("/bin/sh", shell_argv, NULL, plain, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_DIRECTORY);
    char lengthy[300 + 2] = "/";
    memset(lengthy + 1, 'n', 300);
    assert(start(lengthy, argv, NULL, NULL, 0, &c) == DOTNET_PAL_NAME_TOO_LONG && unlink(plain) == 0);
    /* A child that did not start leaves nothing: no descriptor here and nothing to wait for. */
    assert(descriptors() == opened);
    no_children();
    return directory == DOTNET_PAL_ACCESS_DENIED ? "ACCESS_DENIED" : "IS_DIRECTORY";
}

/* Returns how many descriptors a live child without pipes costs this process: the Linux provider's pidfd. */
static int descriptors_and_zombies(void) {
    dotnet_pal_spawned held[2], c; char text[256]; int32_t code = 7;
    int opened = descriptors();
    const char *cat[] = {"cat", NULL}, *nap[] = {"sleep", "5", NULL};
    /* Children with every pipe are alive while another one lists what it was given: the three streams and the listing's own. */
    for (int i = 0; i < 2; ++i) assert(start("/bin/cat", cat, NULL, NULL, IN | OUT | ERR, &held[i]) == 0);
    assert(shell("for f in /proc/self/fd/*; do echo \"${f##*/}\"; done", text, sizeof text) == 0);
    int listed = 0, extra = 0;
    for (char *line = strtok(text, "\n"); line; line = strtok(NULL, "\n")) { ++listed; if (atoi(line) > 2) ++extra; }
    assert(listed - extra == 3 && extra <= 1);
    for (int i = 0; i < 2; ++i) assert(finish(&held[i]) == 0);
    assert(descriptors() == opened);
    assert(start("/bin/sleep", nap, NULL, NULL, 0, &c) == 0);
    int cost = descriptors() - opened;
    /* Giving the handle back does not end the child: it is this test that ends it, and that reaps it. */
    pid_t pid = (pid_t)c.id; int status = 0;
    assert(p->release(c.process) == 0 && descriptors() == opened);
    struct timespec pause = {0, 50000000}; nanosleep(&pause, NULL);
    assert(kill(pid, 0) == 0 && waitpid(pid, &status, WNOHANG) == 0);
    assert(kill(pid, SIGKILL) == 0 && waitpid(pid, &status, 0) == pid && WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
    /* A child that had already ended when its handle was given back, without a wait, leaves no zombie. */
    const char *leave[] = {"sh", "-c", "exit 0", NULL};
    assert(start("/bin/sh", leave, NULL, NULL, 0, &c) == 0);
    siginfo_t info;
    pid = (pid_t)c.id;
    assert(waitid(P_PID, (id_t)pid, &info, WEXITED | WNOWAIT) == 0 && p->release(c.process) == 0);
    errno = 0; assert(waitpid(pid, &status, WNOHANG) == -1 && errno == ECHILD);
    for (int i = 0; i < 50; ++i) {
        char script[16]; snprintf(script, sizeof script, "exit %d", i % 8);
        const char *argv[] = {"sh", "-c", script, NULL};
        assert(start("/bin/sh", argv, NULL, NULL, 0, &c) == 0 && p->wait(c.process, FOREVER, &code) == 0 && code == i % 8 && p->release(c.process) == 0);
    }
    assert(descriptors() == opened);
    no_children();
    return cost;
}

/* What the runtime arranges for itself stays with it: the child starts with nothing blocked and nothing ignored. */
static void signals(void) {
    char text[4096]; sigset_t blocked, before; unsigned long long mask = 7, ignored = 7;
    sigemptyset(&blocked); sigaddset(&blocked, SIGUSR1); sigaddset(&blocked, SIGTERM);
    assert(pthread_sigmask(SIG_BLOCK, &blocked, &before) == 0);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR && signal(SIGUSR2, SIG_IGN) != SIG_ERR && signal(SIGINT, SIG_IGN) != SIG_ERR);
    const char *argv[] = {"cat", "/proc/self/status", NULL};
    assert(run("/bin/cat", argv, NULL, NULL, text, sizeof text) == 0);
    assert(signal(SIGPIPE, SIG_DFL) != SIG_ERR && signal(SIGUSR2, SIG_DFL) != SIG_ERR && signal(SIGINT, SIG_DFL) != SIG_ERR);
    assert(pthread_sigmask(SIG_SETMASK, &before, NULL) == 0);
    const char *line = strstr(text, "SigBlk:"); assert(line && sscanf(line, "SigBlk: %llx", &mask) == 1 && mask == 0);
    /* Signals 32 and 33 are the C library's own; its posix_spawn leaves them ignored in the child. */
    line = strstr(text, "SigIgn:"); assert(line && sscanf(line, "SigIgn: %llx", &ignored) == 1 && (ignored & ~(3ull << 31)) == 0);
}

static void validation(void) {
    dotnet_pal_spawned c, out; int32_t code = 7; size_t done = 7; uint8_t byte = 0;
    dotnet_pal_processes_stats before, after;
    const char *cat[] = {"cat", NULL};
    const uint8_t *arguments[] = {(const uint8_t*)"sh", (const uint8_t*)"-c", (const uint8_t*)"exit 0"}, *holed[] = {(const uint8_t*)"sh", NULL, (const uint8_t*)"exit 0"};
    const uint8_t *environment[] = {(const uint8_t*)"A=1", NULL};
    static uint8_t lengthy[4097];
    memset(lengthy, 'n', sizeof lengthy); lengthy[0] = '/';
    assert(start("/bin/cat", cat, NULL, NULL, IN | OUT, &c) == 0);
    assert(p->read_stats(&before, sizeof before) == 0);
#define SPAWN(...) do { memset(&out, 0x55, sizeof out); assert(p->spawn(__VA_ARGS__) == INVALID); } while (0)
    SPAWN(NULL, 7, arguments, 3, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT(""), arguments, 3, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN((const uint8_t*)"/bin/sh\0x", 9, arguments, 3, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out)); /* a terminator inside the path */
    SPAWN(lengthy, sizeof lengthy - 1, arguments, 3, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 0, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out)); /* no argument 0 */
    SPAWN(TEXT("/bin/sh"), NULL, 3, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), holed, 3, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, DOTNET_PAL_MAX_ARGUMENTS + 1, NULL, 0, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 1, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, environment, 2, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    /* An environment entry is NAME=value with a name: no target can hand anything else to a child. */
    const uint8_t *nameless[] = {(const uint8_t*)"A=1", (const uint8_t*)"=1"}, *valueless[] = {(const uint8_t*)"A"}, *blank[] = {(const uint8_t*)""};
    SPAWN(TEXT("/bin/sh"), arguments, 3, nameless, 2, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, valueless, 1, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, blank, 1, NULL, 0, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 4, 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, TEXT(""), 0, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 8, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, IN | OUT | ERR | 8, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, NULL, sizeof out);
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, (dotnet_pal_spawned*)((char*)&out + 1), sizeof out);
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, &out, sizeof out - 1); assert(out.id == UINT64_C(0x5555555555555555)); /* too small to write to */
#undef SPAWN
    assert(p->wait(NULL, 0, &code) == INVALID && code == 0 && p->wait(c.process, 0, NULL) == INVALID);
    assert(p->terminate(NULL, 0) == INVALID && p->terminate(c.process, 2) == INVALID && p->release(NULL) == INVALID);
    assert(p->pipe_read(NULL, &byte, 1, &done) == INVALID && done == 0 && p->pipe_read(c.output, NULL, 1, &done) == INVALID && p->pipe_read(c.output, &byte, 1, NULL) == INVALID);
    assert(p->pipe_write(NULL, &byte, 1, &done) == INVALID && p->pipe_write(c.input, NULL, 1, &done) == INVALID && p->pipe_write(c.input, &byte, 1, NULL) == INVALID);
    assert(p->pipe_close(NULL) == INVALID && p->read_stats(NULL, sizeof after) == INVALID && p->read_stats(&after, sizeof after - 1) == INVALID);
    assert(p->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + 32);
    after.rejected_or_failed = before.rejected_or_failed;
    assert(memcmp(&before, &after, sizeof after) == 0);
    refused += 32;
    /* Nothing to transfer is no transfer, and no answer about the other end either. */
    done = 7; assert(p->pipe_read(c.output, &byte, 0, &done) == 0 && done == 0);
    done = 7; assert(p->pipe_write(c.input, &byte, 0, &done) == 0 && done == 0);
    assert(kill((pid_t)c.id, 0) == 0 && finish(&c) == 0);
}

#ifndef PAL_HOST_TEST
/* Makes pidfd_open answer as a kernel before 5.3 does, for this process and its children. */
static void without_pidfd(void) {
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_pidfd_open, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {sizeof filter / sizeof filter[0], filter};
    assert(prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0 && prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) == 0);
}
#endif
int main(int argc, char **argv) {
    alarm(20);
    /* A write to a child that has ended must not depend on the process ignoring SIGPIPE. */
    signal(SIGPIPE, SIG_DFL);
#ifdef PAL_HOST_TEST
    pal_processes_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    if (argc > 1 && strcmp(argv[1], "nopidfd") == 0) without_pidfd();
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_processes_fault == 1) { assert(!api); puts("PROCESSES malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_PROCESSES_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_PROCESSES);
    p = &api->processes;
    assert(p->spawn && p->wait && p->terminate && p->release && p->pipe_read && p->pipe_write && p->pipe_close && p->read_stats);
    dotnet_pal_processes_stats stats;
#ifdef PAL_HOST_TEST
    if (pal_processes_fault == 2) {
        int handle = 0; /* any non-null handle: this provider answers before it looks at one */
        dotnet_pal_spawned out; const uint8_t *arguments[] = {(const uint8_t*)"sh"}; uint8_t data[8]; size_t done = 7; int32_t code = 7;
        /* No handle, no identifier, a pipe nobody asked for, no pipe where one was asked for, a status spawn does not have. */
        for (int i = 0; i < 5; ++i) {
            memset(&out, 0x55, sizeof out);
            assert(p->spawn(TEXT("/bin/sh"), arguments, 1, NULL, 0, NULL, 0, IN | OUT, &out, sizeof out) == DOTNET_PAL_OS_ERROR && empty(&out));
        }
        assert(p->wait(&handle, 0, &code) == DOTNET_PAL_OS_ERROR && code == 0);
        assert(p->terminate(&handle, 0) == DOTNET_PAL_OS_ERROR && p->release(&handle) == DOTNET_PAL_OS_ERROR && p->pipe_close(&handle) == DOTNET_PAL_OS_ERROR);
        assert(p->pipe_read(&handle, data, sizeof data, &done) == DOTNET_PAL_OS_ERROR && done == 0); /* more than fits */
        for (int i = 0; i < 2; ++i) { done = 7; assert(p->pipe_write(&handle, data, sizeof data, &done) == DOTNET_PAL_OS_ERROR && done == 0); } /* nothing, then more than offered */
        assert(p->read_stats(&stats, sizeof stats) == 0 && stats.rejected_or_failed == 12);
        assert(stats.spawn_ok + stats.wait_ok + stats.terminate_ok + stats.release_ok + stats.pipe_read_ok + stats.pipe_write_ok + stats.pipe_close_ok == 0);
        puts("PROCESSES host errors sanitized"); return 0;
    }
#endif
    /* The standard streams are this test's to pass on, all three of them; whatever else it was started with is closed. */
    for (int fd = 0; fd < 3; ++fd) if (fcntl(fd, F_GETFD) < 0) assert(open("/dev/null", O_RDWR) == fd);
    for (int fd = 3; fd < 256; ++fd) close(fd);
    int opened = descriptors();
    /* Each part gets its own watchdog: a child that never ends or a pipe that never closes fails the run instead of hanging it. */
    alarm(20); codes_and_arguments();
    alarm(20); environment_and_directory();
    alarm(20); transfers();
    alarm(20); waiting();
    alarm(20); const char *directory = failures();
    alarm(20); int cost = descriptors_and_zombies();
    alarm(20); signals();
    alarm(20); validation();
#ifndef PAL_HOST_TEST
    /* One descriptor per live child where the kernel has pidfd_open, none where it has not. */
    int pidfd = (int)syscall(SYS_pidfd_open, getpid(), 0);
    assert(cost == (pidfd >= 0) && (argc == 1 || pidfd < 0));
    if (pidfd >= 0) close(pidfd);
#endif
    assert(descriptors() == opened);
    no_children();
    assert(p->read_stats(&stats, sizeof stats) == 0);
    /* Every child was given back and every pipe closed; the refusals are at least the ones counted here. */
    assert(stats.spawn_ok == children && stats.release_ok == children && stats.pipe_close_ok == pipes_made && children >= 75);
    assert(stats.wait_ok >= children - 2 && stats.terminate_ok == 4 && stats.pipe_read_ok >= 40 && stats.pipe_write_ok >= 2);
    assert(stats.rejected_or_failed == refused + 7); /* and three waits that ran out, two ends of a child that was gone, two writes to nobody */
    alarm(0);
    printf("PROCESSES PASS children=%u pipes=%u directory=%s descriptors_per_child=%d refused=%llu\n", children, pipes_made, directory, cost,
        (unsigned long long)stats.rejected_or_failed);
    return 0;
}
