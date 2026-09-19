/* Conformance test of the spawn_as group on Linux: real children (sh, id, cat, sleep)
 * started under another identity and checked against what the kernel reports about
 * them, and handled afterwards through the processes group, whose children they are.
 * The main part needs the privilege to change identity (root in the test container);
 * what a process without it may and may not do is checked from a forked copy of this
 * test that has given the privilege up, and by the whole run when it is not root.
 * With -DPAL_HOST_TEST the same run goes against the C host tables; faults 1 and 2
 * check rejection and sanitizing. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_spawn_as_fault;
#endif
enum { IN = DOTNET_PAL_PIPE_INPUT, OUT = DOTNET_PAL_PIPE_OUTPUT, ERR = DOTNET_PAL_PIPE_ERROR };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define DENIED DOTNET_PAL_ACCESS_DENIED
#define FOREVER DOTNET_PAL_INFINITE_NS
#define MS UINT64_C(1000000)
#define TEXT(text) (const uint8_t*)(text), strlen(text)
static const dotnet_pal_processes_ops *p;
static const dotnet_pal_spawn_as_ops *s;
/* Children and refusals of spawn_as, children of spawn, and the pipes of both; threads start children too. */
static atomic_uint children, refused, plain, pipes_made;
static const uint32_t GROUPS[] = {23456, 34567, 45678};
static const dotnet_pal_identity OTHER = {12345, 23456, GROUPS, 3};
static uint64_t now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * UINT64_C(1000000000) + (uint64_t)t.tv_nsec; }
static int empty(const dotnet_pal_spawned *c) { return !c->process && c->id == 0 && !c->input && !c->output && !c->error; }
/* Open descriptors of this process. */
static int descriptors(void) {
    DIR *list = opendir("/proc/self/fd"); assert(list);
    int count = 0;
    for (struct dirent *e; (e = readdir(list));) if (e->d_name[0] != '.' && atoi(e->d_name) != dirfd(list)) ++count;
    closedir(list);
    return count;
}
static void no_children(void) { errno = 0; assert(waitpid(-1, NULL, WNOHANG) == -1 && errno == ECHILD); }
/* Starts a program from C strings, under an identity or, without one, through the processes group. What the boundary
 * takes is counted, not terminated: the paths are followed by other bytes and the vectors by one more text. */
static uint32_t start(const dotnet_pal_identity *identity, const char *program, const char *const *argv, const char *const *envp, const char *directory,
    uint32_t pipes, dotnet_pal_spawned *out) {
    const uint8_t *arguments[16], *environment[16]; size_t argc = 0, envc = 0;
    uint8_t path[PATH_MAX], home[PATH_MAX];
    memset(path, 'X', sizeof path); memcpy(path, program, strlen(program));
    memset(home, 'X', sizeof home); if (directory) memcpy(home, directory, strlen(directory));
    for (; argv[argc]; ++argc) arguments[argc] = (const uint8_t*)argv[argc];
    arguments[argc] = (const uint8_t*)"OVERRUN";
    for (; envp && envp[envc]; ++envc) environment[envc] = (const uint8_t*)envp[envc];
    environment[envc] = (const uint8_t*)"OVERRUN=1";
    memset(out, 0x55, sizeof *out);
    uint32_t status = identity
        ? s->spawn_as(path, strlen(program), arguments, argc, envp ? environment : NULL, envc, directory ? home : NULL, directory ? strlen(directory) : 0, pipes, identity, out, sizeof *out)
        : p->spawn(path, strlen(program), arguments, argc, envp ? environment : NULL, envc, directory ? home : NULL, directory ? strlen(directory) : 0, pipes, out, sizeof *out);
    if (status != 0) { assert(identity && empty(out)); ++refused; return status; }
    /* A handle, the kernel's identifier and exactly the pipes that were asked for. */
    assert(out->process && out->id != 0 && (kill((pid_t)out->id, 0) == 0 || errno == EPERM));
    assert(!out->input == !(pipes & IN) && !out->output == !(pipes & OUT) && !out->error == !(pipes & ERR));
    if (identity) ++children; else ++plain;
    pipes_made += (unsigned)(!!(pipes & IN) + !!(pipes & OUT) + !!(pipes & ERR));
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
static int32_t run(const dotnet_pal_identity *identity, const char *program, const char *const *argv, const char *const *envp, const char *directory, char *output, size_t capacity) {
    dotnet_pal_spawned c;
    assert(start(identity, program, argv, envp, directory, OUT, &c) == 0);
    drain(c.output, output, capacity);
    return finish(&c);
}
static int32_t shell(const dotnet_pal_identity *identity, const char *script, char *output, size_t capacity) {
    const char *argv[] = {"sh", "-c", script, NULL};
    return run(identity, "/bin/sh", argv, NULL, NULL, output, capacity);
}
/* One line of a status file of procfs, without its name and the tab after it. */
static void status_line(const char *file, const char *name, char *out, size_t capacity) {
    char text[8192]; int fd = open(file, O_RDONLY | O_CLOEXEC); assert(fd >= 0);
    ssize_t n = read(fd, text, sizeof text - 1); assert(n > 0 && close(fd) == 0);
    text[n] = 0;
    const char *line = strstr(text, name); assert(line && (line == text || line[-1] == '\n'));
    line += strlen(name); assert(*line == '\t');
    size_t length = strcspn(++line, "\n"); assert(length < capacity);
    memcpy(out, line, length); out[length] = 0;
}
#define WHO "id -u; id -g; id -G"

/* What id and the kernel say about the child: every one of its user and group ids is the new one, and its groups are the list. */
static void identity_of_the_child(void) {
    char text[256], file[64], line[256]; dotnet_pal_spawned c;
    assert(shell(&OTHER, WHO, text, sizeof text) == 0 && strcmp(text, "12345\n23456\n23456 34567 45678\n") == 0);
    const char *nap[] = {"sleep", "5", NULL};
    assert(start(&OTHER, "/bin/sleep", nap, NULL, NULL, 0, &c) == 0);
    snprintf(file, sizeof file, "/proc/%llu/status", (unsigned long long)c.id);
    /* The program has replaced the forked child by the time the call returns. Real, effective, saved and file system id. */
    status_line(file, "Name:", line, sizeof line); assert(strcmp(line, "sleep") == 0);
    status_line(file, "Uid:", line, sizeof line); assert(strcmp(line, "12345\t12345\t12345\t12345") == 0);
    status_line(file, "Gid:", line, sizeof line); assert(strcmp(line, "23456\t23456\t23456\t23456") == 0);
    status_line(file, "Groups:", line, sizeof line); assert(strcmp(line, "23456 34567 45678 ") == 0);
    /* Nothing of the privilege is left to take up again. */
    status_line(file, "CapPrm:", line, sizeof line); assert(strcmp(line, "0000000000000000") == 0);
    status_line(file, "CapEff:", line, sizeof line); assert(strcmp(line, "0000000000000000") == 0);
    assert(p->terminate(c.process, 1) == 0 && finish(&c) == 128 + SIGKILL);
    /* The list is the whole list: the primary group is a supplementary group only when the list names it, the kernel
     * keeps the list in order, and an empty list leaves none. id -G begins with the primary group either way. */
    static const uint32_t without[] = {45678, 34567};
    const dotnet_pal_identity unlisted = {12345, 23456, without, 2}, none = {12345, 23456, NULL, 0}, same = {0, 0, NULL, 0};
    assert(shell(&unlisted, WHO "; grep Groups: /proc/self/status", text, sizeof text) == 0 && strcmp(text, "12345\n23456\n23456 34567 45678\nGroups:\t34567 45678 \n") == 0);
    assert(shell(&none, WHO "; grep Groups: /proc/self/status", text, sizeof text) == 0 && strcmp(text, "12345\n23456\n23456\nGroups:\t \n") == 0);
    /* The identity this process has is one it may take, without its groups too. */
    assert(shell(&same, WHO, text, sizeof text) == 0 && strcmp(text, "0\n0\n0\n") == 0);
}

/* The child cannot do what only the parent's user may. That it cannot become that user again is in the saved ids and the
 * empty capability sets above. */
static void no_way_back(void) {
    char text[256], errors[512]; dotnet_pal_spawned c;
    const char *argv[] = {"sh", "-c", "id -u; cat /etc/shadow", NULL};
    assert(access("/etc/shadow", R_OK) == 0);
    assert(start(&OTHER, "/bin/sh", argv, NULL, NULL, OUT | ERR, &c) == 0);
    drain(c.output, text, sizeof text); drain(c.error, errors, sizeof errors);
    assert(strcmp(text, "12345\n") == 0 && strstr(errors, "Permission denied") && finish(&c) != 0);
}

struct waiter { void *process; uint64_t timeout; uint32_t status; int32_t code; };
static void *waiter(void *arg) {
    struct waiter *w = arg;
    w->code = 7; w->status = p->wait(w->process, w->timeout, &w->code);
    return NULL;
}
/* A child of spawn_as is a child of the processes group: its handle and its pipes are that group's. */
static void children_of_the_processes_group(void) {
    dotnet_pal_spawned c; char text[8192], errors[256]; int32_t code = 7; size_t got = 7, written = 7;
    assert(shell(&OTHER, "exit 0", text, sizeof text) == 0 && shell(&OTHER, "exit 7", text, sizeof text) == 7 && shell(&OTHER, "exit 255", text, sizeof text) == 255);
    const char *renamed[] = {"another name", "-c", "echo \"$0\"", NULL};
    assert(run(&OTHER, "/bin/sh", renamed, NULL, NULL, text, sizeof text) == 0 && strcmp(text, "another name\n") == 0);
    const char *words[] = {"sh", "-c", "for a; do printf '[%s]' \"$a\"; done; echo $#", "zero", "two words", "", "*", NULL};
    assert(run(&OTHER, "/bin/sh", words, NULL, NULL, text, sizeof text) == 0 && strcmp(text, "[two words][][*]3\n") == 0);
    const char *self[] = {"sh", "-c", "echo $$", NULL};
    assert(start(&OTHER, "/bin/sh", self, NULL, NULL, OUT, &c) == 0);
    drain(c.output, text, sizeof text);
    assert(strtoull(text, NULL, 10) == c.id && finish(&c) == 0);
    /* A given environment is the whole environment; none given is the parent's, not the new user's. */
    assert(setenv("PAL_PARENT", "inherited", 1) == 0);
    const char *env[] = {"env", NULL}, *given[] = {"PAL_GIVEN=yes", "PAL_EMPTY=", "PAL_SPACED=a b=c", NULL}, *nothing[] = {NULL};
    assert(run(&OTHER, "/usr/bin/env", env, given, NULL, text, sizeof text) == 0 && strcmp(text, "PAL_GIVEN=yes\nPAL_EMPTY=\nPAL_SPACED=a b=c\n") == 0);
    assert(run(&OTHER, "/usr/bin/env", env, nothing, NULL, text, sizeof text) == 0 && strcmp(text, "") == 0);
    assert(run(&OTHER, "/usr/bin/env", env, NULL, NULL, text, sizeof text) == 0 && strstr(text, "PAL_PARENT=inherited\n") && !strstr(text, "PAL_GIVEN"));
    /* All three pipes: the child reads what the parent writes until the parent closes, and its two outputs stay apart. */
    const char *both[] = {"sh", "-c", "cat; echo problem >&2", NULL};
    assert(start(&OTHER, "/bin/sh", both, NULL, NULL, IN | OUT | ERR, &c) == 0);
    feed(c.input, "to the child\n", 13);
    assert(p->pipe_close(c.input) == 0); c.input = NULL;
    assert(drain(c.output, text, sizeof text) == 13 && strcmp(text, "to the child\n") == 0);
    assert(drain(c.error, errors, sizeof errors) == 8 && strcmp(errors, "problem\n") == 0 && finish(&c) == 0);
    /* A write to a child that has ended is a status. */
    const char *leave[] = {"sh", "-c", "exit 3", NULL};
    assert(start(&OTHER, "/bin/sh", leave, NULL, NULL, IN, &c) == 0 && p->wait(c.process, FOREVER, &code) == 0 && code == 3);
    assert(p->pipe_write(c.input, TEXT("x"), &written) == DOTNET_PAL_BROKEN_PIPE && written == 0 && finish(&c) == 3);
    /* Waits run out, a request to end reaches the other user's process, and every later wait says the same. */
    const char *nap[] = {"sleep", "5", NULL};
    assert(start(&OTHER, "/bin/sleep", nap, NULL, NULL, 0, &c) == 0);
    assert(p->wait(c.process, 0, &code) == DOTNET_PAL_TIMEOUT && code == 0);
    uint64_t begin = now();
    assert(p->wait(c.process, 100 * MS, &code) == DOTNET_PAL_TIMEOUT && now() - begin >= 100 * MS && now() - begin < 2000 * MS);
    assert(p->terminate(c.process, 0) == 0 && p->wait(c.process, FOREVER, &code) == 0 && code == 128 + SIGTERM);
    code = 7; assert(p->wait(c.process, 0, &code) == 0 && code == 128 + SIGTERM);
    assert(p->terminate(c.process, 0) == DOTNET_PAL_NOT_FOUND && p->terminate(c.process, 1) == DOTNET_PAL_NOT_FOUND && p->release(c.process) == 0);
    /* A child that ignores the request runs on until it is ended, while another thread waits without a limit. */
    const char *stubborn[] = {"sh", "-c", "trap '' TERM; echo ready; exec sleep 5", NULL};
    assert(start(&OTHER, "/bin/sh", stubborn, NULL, NULL, OUT, &c) == 0);
    assert(p->pipe_read(c.output, (uint8_t*)text, sizeof text, &got) == 0 && got == 6 && memcmp(text, "ready\n", 6) == 0);
    assert(p->terminate(c.process, 0) == 0 && p->wait(c.process, 200 * MS, &code) == DOTNET_PAL_TIMEOUT);
    struct waiter watcher = {c.process, FOREVER, 7, 7}; pthread_t thread;
    assert(pthread_create(&thread, NULL, waiter, &watcher) == 0);
    begin = now();
    assert(p->terminate(c.process, 1) == 0 && pthread_join(thread, NULL) == 0 && now() - begin < 2000 * MS);
    assert(watcher.status == 0 && watcher.code == 128 + SIGKILL && finish(&c) == 128 + SIGKILL);
}

/* What the runtime arranges for itself stays with it: the child starts with nothing blocked and nothing ignored. */
static void signals(void) {
    char text[4096]; sigset_t blocked, before; unsigned long long mask = 7, ignored = 7;
    sigemptyset(&blocked); sigaddset(&blocked, SIGUSR1); sigaddset(&blocked, SIGTERM);
    assert(pthread_sigmask(SIG_BLOCK, &blocked, &before) == 0);
    assert(signal(SIGPIPE, SIG_IGN) != SIG_ERR && signal(SIGUSR2, SIG_IGN) != SIG_ERR && signal(SIGINT, SIG_IGN) != SIG_ERR);
    const char *argv[] = {"cat", "/proc/self/status", NULL};
    assert(run(&OTHER, "/bin/cat", argv, NULL, NULL, text, sizeof text) == 0);
    /* The start gave this thread its own mask back. */
    sigset_t during; assert(pthread_sigmask(SIG_SETMASK, NULL, &during) == 0);
    for (int number = 1; number < NSIG; ++number) assert(sigismember(&during, number) == (number == SIGUSR1 || number == SIGTERM));
    assert(signal(SIGPIPE, SIG_DFL) != SIG_ERR && signal(SIGUSR2, SIG_DFL) != SIG_ERR && signal(SIGINT, SIG_DFL) != SIG_ERR);
    assert(pthread_sigmask(SIG_SETMASK, &before, NULL) == 0);
    const char *line = strstr(text, "SigBlk:"); assert(line && sscanf(line, "SigBlk: %llx", &mask) == 1 && mask == 0);
    /* Signals 32 and 33 are the C library's own; it does not let a program change them. */
    line = strstr(text, "SigIgn:"); assert(line && sscanf(line, "SigIgn: %llx", &ignored) == 1 && (ignored & ~(3ull << 31)) == 0);
}

/* A program started with its input and output closed gets pipe descriptors below 3, the report of a failed start among
 * them; the child's streams are the pipes all the same, and a start that fails still says why. The streams come back
 * once the child is given back: until then a descriptor below 3 may be the provider's own. */
static void closed_streams(void) {
    dotnet_pal_spawned c, gone; char text[256], errors[256];
    int saved[2];
    fflush(stdout);
    for (int fd = 0; fd < 2; ++fd) { saved[fd] = fcntl(fd, F_DUPFD_CLOEXEC, 3); assert(saved[fd] >= 0 && close(fd) == 0); }
    const char *both[] = {"sh", "-c", "cat; echo problem >&2", NULL}, *argv[] = {"program", NULL};
    /* The start that fails comes first, while descriptors 0 and 1 are both free for the pipe that reports it. */
    assert(start(&OTHER, "/nonexistent/program", argv, NULL, NULL, IN | OUT | ERR, &gone) == DOTNET_PAL_NOT_FOUND);
    assert(start(&OTHER, "/bin/sh", both, NULL, NULL, IN | OUT | ERR, &c) == 0);
    feed(c.input, "to the child\n", 13);
    assert(p->pipe_close(c.input) == 0); c.input = NULL;
    assert(drain(c.output, text, sizeof text) == 13 && strcmp(text, "to the child\n") == 0);
    assert(drain(c.error, errors, sizeof errors) == 8 && strcmp(errors, "problem\n") == 0 && finish(&c) == 0);
    assert(fcntl(0, F_GETFD) < 0 && fcntl(1, F_GETFD) < 0);
    for (int fd = 0; fd < 2; ++fd) assert(dup2(saved[fd], fd) == fd && close(saved[fd]) == 0);
}

/* A start that fails is named as spawn names it, whichever step of the child failed, and leaves nothing. The program is
 * found and the directory entered as the new user. */
static void failures(void) {
    dotnet_pal_spawned c; const char *argv[] = {"program", NULL}, *shell_argv[] = {"sh", "-c", "exit 0", NULL};
    int opened = descriptors();
    assert(start(&OTHER, "/nonexistent/program", argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    assert(start(&OTHER, "/bin/sh/program", argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_DIRECTORY);
    assert(access("sh", F_OK) != 0 && start(&OTHER, "sh", shell_argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    uint32_t directory = start(&OTHER, "/bin", argv, NULL, NULL, IN | OUT | ERR, &c);
    assert(directory == DENIED || directory == DOTNET_PAL_IS_DIRECTORY);
    /* A directory and a program of the parent's user alone: the parent's user gets in, the child's does not. */
    char home[] = "/tmp/pal-spawn-as-XXXXXX", tool[64], relative[64];
    assert(mkdtemp(home));
    snprintf(tool, sizeof tool, "%s/tool", home); snprintf(relative, sizeof relative, "%s/tool", strrchr(home, '/') + 1);
    int file = open(tool, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0700);
    assert(file >= 0 && write(file, "#!/bin/sh\nexit 4\n", 17) == 17 && close(file) == 0);
    assert(start(NULL, "/bin/sh", shell_argv, NULL, home, 0, &c) == 0 && finish(&c) == 0);
    assert(start(&OTHER, "/bin/sh", shell_argv, NULL, home, IN | OUT | ERR, &c) == DENIED);
    assert(chmod(home, 0755) == 0 && start(&OTHER, "/bin/sh", shell_argv, NULL, home, 0, &c) == 0 && finish(&c) == 0);
    assert(start(NULL, tool, argv, NULL, NULL, 0, &c) == 0 && finish(&c) == 4);
    assert(start(&OTHER, tool, argv, NULL, NULL, IN | OUT | ERR, &c) == DENIED);
    assert(chmod(tool, 0755) == 0 && start(&OTHER, tool, argv, NULL, NULL, 0, &c) == 0 && finish(&c) == 4);
    /* A relative program is a file in the child's working directory, never a search of PATH. */
    assert(start(&OTHER, "tool", argv, NULL, home, 0, &c) == 0 && finish(&c) == 4);
    assert(start(&OTHER, relative, argv, NULL, "/tmp", 0, &c) == 0 && finish(&c) == 4);
    assert(start(&OTHER, "/bin/sh", shell_argv, NULL, "/nonexistent/directory", IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    assert(start(&OTHER, "/bin/sh", shell_argv, NULL, tool, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_DIRECTORY);
    char lengthy[300 + 2] = "/";
    memset(lengthy + 1, 'n', 300);
    assert(start(&OTHER, lengthy, argv, NULL, NULL, 0, &c) == DOTNET_PAL_NAME_TOO_LONG);
    /* An id the kernel has no user for. */
    const dotnet_pal_identity nobody = {UINT32_MAX, 23456, GROUPS, 3}, nothing = {12345, UINT32_MAX, GROUPS, 3};
    assert(start(&nobody, "/bin/sh", shell_argv, NULL, NULL, IN | OUT | ERR, &c) == INVALID && start(&nothing, "/bin/sh", shell_argv, NULL, NULL, IN | OUT | ERR, &c) == INVALID);
    assert(unlink(tool) == 0 && rmdir(home) == 0);
    assert(descriptors() == opened);
    no_children();
}

/* The child is given what a child of spawn is given, the descriptors other code of this process left inheritable included. */
static void descriptors_and_zombies(void) {
    dotnet_pal_spawned held[2], c; char text[256], same[256]; int32_t code = 7;
    int opened = descriptors();
    const char *cat[] = {"cat", NULL}, *nap[] = {"sleep", "5", NULL};
    const char *listing = "for f in /proc/self/fd/*; do echo \"${f##*/}\"; done";
    for (int i = 0; i < 2; ++i) assert(start(&OTHER, "/bin/cat", cat, NULL, NULL, IN | OUT | ERR, &held[i]) == 0);
    assert(shell(&OTHER, listing, text, sizeof text) == 0 && shell(NULL, listing, same, sizeof same) == 0 && strcmp(text, same) == 0);
    int listed = 0, extra = 0;
    for (char *line = strtok(text, "\n"); line; line = strtok(NULL, "\n")) { ++listed; if (atoi(line) > 2) ++extra; }
    assert(listed - extra == 3 && extra <= 1);
    int foreign = open("/dev/null", O_RDONLY); assert(foreign > 2);
    assert(shell(&OTHER, listing, text, sizeof text) == 0 && shell(NULL, listing, same, sizeof same) == 0 && strcmp(text, same) == 0 && close(foreign) == 0);
    for (int i = 0; i < 2; ++i) assert(finish(&held[i]) == 0);
    assert(descriptors() == opened);
    /* Giving the handle back does not end the child: it is this test that ends it, and that reaps it. */
    assert(start(&OTHER, "/bin/sleep", nap, NULL, NULL, 0, &c) == 0);
    pid_t pid = (pid_t)c.id; int status = 0;
    assert(p->release(c.process) == 0 && descriptors() == opened);
    assert(kill(pid, 0) == 0 && waitpid(pid, &status, WNOHANG) == 0);
    assert(kill(pid, SIGKILL) == 0 && waitpid(pid, &status, 0) == pid && WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
    for (int i = 0; i < 50; ++i) {
        char script[16]; snprintf(script, sizeof script, "exit %d", i % 8);
        const char *argv[] = {"sh", "-c", script, NULL};
        assert(start(&OTHER, "/bin/sh", argv, NULL, NULL, 0, &c) == 0 && p->wait(c.process, FOREVER, &code) == 0 && code == i % 8 && p->release(c.process) == 0);
    }
    assert(descriptors() == opened);
    no_children();
}

/* What a process without the privilege may do: be what it is. `held` are the groups it holds. */
static void without_privilege(const uint32_t *held, size_t count) {
    dotnet_pal_spawned c; char text[256], same[256]; const char *argv[] = {"sh", "-c", "exit 0", NULL};
    uint32_t more[66], user = (uint32_t)getuid(), group = (uint32_t)getgid();
    assert(user != 0 && count <= 64);
    int opened = descriptors();
    /* Its own identity, and a list that names more groups than it holds: the child gains none of them. */
    const dotnet_pal_identity own = {user, group, held, count};
    assert(shell(NULL, WHO, same, sizeof same) == 0 && shell(&own, WHO, text, sizeof text) == 0 && strcmp(text, same) == 0);
    more[0] = 45678; memcpy(more + 1, held, count * sizeof *held); more[count + 1] = 7;
    const dotnet_pal_identity generous = {user, group, more, count + 2};
    assert(shell(&generous, WHO, text, sizeof text) == 0 && strcmp(text, same) == 0);
    /* Another user, another group, and, for a process that holds groups, a list that lacks one of them: it cannot put a group down. */
    const dotnet_pal_identity foreign = {12345, 23456, GROUPS, 3}, user_only = {user + 1, group, held, count}, group_only = {user, group + 1, held, count},
        root = {0, 0, held, count}, fewer = {user, group, held, count ? count - 1 : 0}, none = {user, group, NULL, 0};
    const dotnet_pal_identity *denied[] = {&foreign, &user_only, &group_only, &root, &fewer, &none};
    for (size_t i = 0; i < (count ? 6 : 4); ++i) assert(start(denied[i], "/bin/sh", argv, NULL, NULL, IN | OUT | ERR, &c) == DENIED);
    /* The other failures keep their names. */
    assert(start(&own, "/nonexistent/program", argv, NULL, NULL, IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    assert(start(&own, "/bin/sh", argv, NULL, "/nonexistent/directory", IN | OUT | ERR, &c) == DOTNET_PAL_NOT_FOUND);
    assert(descriptors() == opened);
    no_children();
}
/* A copy of this test gives the privilege up and asks for what it no longer may. Its counts stay in the copy. */
static void from_an_unprivileged_process(void) {
    static const gid_t held[] = {54321, 54322};
    fflush(stdout);
    pid_t pid = fork(); assert(pid >= 0);
    if (pid == 0) {
        alarm(20);
        assert(setgroups(2, held) == 0 && setgid(54321) == 0 && setuid(54321) == 0 && getuid() == 54321 && geteuid() == 54321);
        /* A process that changed its user is not dumpable, and procfs gives its files to root. */
        assert(prctl(PR_SET_DUMPABLE, 1) == 0);
        without_privilege((const uint32_t*)held, 2);
        _exit(0);
    }
    int status = 0;
    assert(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

/* Several threads start children at once, each under an identity of its own. The identity is the child's alone: every
 * thread of this process keeps the ids and groups it had, at every moment a look is taken, and the process stays dumpable. */
enum { THREADS = 5, EACH = 10 };
static atomic_int running;
static void *spawner(void *arg) {
    uint32_t number = (uint32_t)(uintptr_t)arg, groups[] = {34567 + number, 45678 + number};
    const dotnet_pal_identity identity = {12345 + number, 23456 + number, groups, 2};
    char text[256], expected[256];
    snprintf(expected, sizeof expected, "%u\n%u\n%u %u %u\n", 12345 + number, 23456 + number, 23456 + number, 34567 + number, 45678 + number);
    for (int i = 0; i < EACH; ++i) assert(shell(&identity, WHO, text, sizeof text) == 0 && strcmp(text, expected) == 0);
    --running;
    return NULL;
}
static unsigned look_at_every_thread(const char *uid, const char *gid, const char *groups) {
    DIR *tasks = opendir("/proc/self/task"); assert(tasks);
    unsigned seen = 0;
    for (struct dirent *e; (e = readdir(tasks));) {
        if (e->d_name[0] == '.') continue;
        char file[320], text[8192]; snprintf(file, sizeof file, "/proc/self/task/%s/status", e->d_name);
        int fd = open(file, O_RDONLY | O_CLOEXEC);
        if (fd < 0) { assert(errno == ENOENT || errno == ESRCH); continue; } /* a thread that has ended since */
        ssize_t n = read(fd, text, sizeof text - 1); close(fd);
        if (n <= 0) continue;
        text[n] = 0;
        const char *names[] = {"\nUid:\t", "\nGid:\t", "\nGroups:\t"}, *expected[] = {uid, gid, groups};
        for (int i = 0; i < 3; ++i) {
            const char *line = strstr(text, names[i]); assert(line);
            line += strlen(names[i]);
            assert(strncmp(line, expected[i], strlen(expected[i])) == 0 && line[strlen(expected[i])] == '\n');
        }
        ++seen;
    }
    closedir(tasks);
    return seen;
}
static unsigned threads(void) {
    char uid[128], gid[128], groups[1024]; pthread_t thread[THREADS]; unsigned looks = 0, most = 0;
    status_line("/proc/self/status", "Uid:", uid, sizeof uid); status_line("/proc/self/status", "Gid:", gid, sizeof gid);
    status_line("/proc/self/status", "Groups:", groups, sizeof groups);
    int dumpable = prctl(PR_GET_DUMPABLE); assert(dumpable == 1);
    running = THREADS;
    for (uintptr_t i = 0; i < THREADS; ++i) assert(pthread_create(&thread[i], NULL, spawner, (void*)i) == 0);
    while (running > 0) { unsigned seen = look_at_every_thread(uid, gid, groups); if (seen > most) most = seen; ++looks; }
    for (int i = 0; i < THREADS; ++i) assert(pthread_join(thread[i], NULL) == 0);
    /* Looks were taken while several threads ran. A thread that was joined may still be listed for a moment. */
    assert(most > 1 && looks > 0);
    assert(look_at_every_thread(uid, gid, groups) >= 1);
    assert(prctl(PR_GET_DUMPABLE) == dumpable && getuid() == 0 && geteuid() == 0);
    no_children();
    return looks;
}

static void validation(const dotnet_pal_identity *identity) {
    dotnet_pal_spawned out; dotnet_pal_spawn_as_stats before, after;
    const uint8_t *arguments[] = {(const uint8_t*)"sh", (const uint8_t*)"-c", (const uint8_t*)"exit 0"}, *holed[] = {(const uint8_t*)"sh", NULL, (const uint8_t*)"exit 0"};
    const uint8_t *environment[] = {(const uint8_t*)"A=1", NULL};
    static uint8_t lengthy[4097];
    memset(lengthy, 'n', sizeof lengthy); lengthy[0] = '/';
    int opened = descriptors();
    assert(s->read_stats(&before, sizeof before) == 0);
#define SPAWN(...) do { memset(&out, 0x55, sizeof out); assert(s->spawn_as(__VA_ARGS__) == INVALID); } while (0)
    /* The request is the one of spawn, and is refused as spawn refuses it. */
    SPAWN(NULL, 7, arguments, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT(""), arguments, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN((const uint8_t*)"/bin/sh\0x", 9, arguments, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out)); /* a terminator inside the path */
    SPAWN(lengthy, sizeof lengthy - 1, arguments, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 0, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out)); /* no argument 0 */
    SPAWN(TEXT("/bin/sh"), NULL, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), holed, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, DOTNET_PAL_MAX_ARGUMENTS + 1, NULL, 0, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 1, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, environment, 2, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    const uint8_t *nameless[] = {(const uint8_t*)"A=1", (const uint8_t*)"=1"}, *valueless[] = {(const uint8_t*)"A"}, *blank[] = {(const uint8_t*)""};
    SPAWN(TEXT("/bin/sh"), arguments, 3, nameless, 2, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, valueless, 1, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, blank, 1, NULL, 0, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 4, 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, TEXT(""), 0, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 8, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, IN | OUT | ERR | 8, identity, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, identity, NULL, sizeof out);
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, identity, (dotnet_pal_spawned*)((char*)&out + 1), sizeof out);
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, identity, &out, sizeof out - 1); assert(out.id == UINT64_C(0x5555555555555555)); /* too small to write to */
    /* The identity: there is one, where an identity can be, and its list is as long as it says and no longer than a list may be. */
    static uint32_t many[DOTNET_PAL_MAX_GROUPS + 2];
    union { uint64_t aligned; char bytes[sizeof(dotnet_pal_identity) + 8]; } odd;
    memcpy(odd.bytes + 1, identity, sizeof *identity);
    const dotnet_pal_identity counted = {identity->user_id, identity->group_id, NULL, 1}, lengthy_list = {identity->user_id, identity->group_id, many, DOTNET_PAL_MAX_GROUPS + 1},
        odd_list = {identity->user_id, identity->group_id, (const uint32_t*)((const char*)many + 1), 1};
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, NULL, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, (const dotnet_pal_identity*)(const void*)(odd.bytes + 1), &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, &counted, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, &lengthy_list, &out, sizeof out); assert(empty(&out));
    SPAWN(TEXT("/bin/sh"), arguments, 3, NULL, 0, NULL, 0, 0, &odd_list, &out, sizeof out); assert(empty(&out));
#undef SPAWN
    assert(s->read_stats(NULL, sizeof after) == INVALID && s->read_stats(&after, sizeof after - 1) == INVALID);
    assert(s->read_stats((dotnet_pal_spawn_as_stats*)((char*)&after + 1), sizeof after) == INVALID);
    assert(s->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + 25 && after.spawn_ok == before.spawn_ok);
    refused += 25;
    assert(descriptors() == opened);
    no_children();
}

int main(int argc, char **argv) {
    alarm(20);
    signal(SIGPIPE, SIG_DFL);
#ifdef PAL_HOST_TEST
    pal_spawn_as_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_spawn_as_fault == 1) { assert(!api); puts("SPAWN_AS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_SPAWN_AS_API_SIZE);
    assert((api->header.capabilities & DOTNET_PAL_CAP_SPAWN_AS) && (api->header.capabilities & DOTNET_PAL_CAP_PROCESSES));
    p = &api->processes; s = &api->spawn_as;
    assert(s->spawn_as && s->read_stats && p->spawn && p->wait && p->terminate && p->release && p->pipe_read && p->pipe_write && p->pipe_close);
    dotnet_pal_spawn_as_stats stats; dotnet_pal_processes_stats handled;
#ifdef PAL_HOST_TEST
    if (pal_spawn_as_fault == 2) {
        dotnet_pal_spawned out; const uint8_t *arguments[] = {(const uint8_t*)"sh"};
        /* No handle, no identifier, a pipe nobody asked for, no pipe where one was asked for, a status spawn_as does not have. */
        for (int i = 0; i < 5; ++i) {
            memset(&out, 0x55, sizeof out);
            assert(s->spawn_as(TEXT("/bin/sh"), arguments, 1, NULL, 0, NULL, 0, IN | OUT, &OTHER, &out, sizeof out) == DOTNET_PAL_OS_ERROR && empty(&out));
        }
        /* A refusal keeps its name, and what the provider wrote beside it goes nowhere. */
        memset(&out, 0x55, sizeof out);
        assert(s->spawn_as(TEXT("/bin/sh"), arguments, 1, NULL, 0, NULL, 0, IN | OUT, &OTHER, &out, sizeof out) == DENIED && empty(&out));
        assert(s->read_stats(&stats, sizeof stats) == 0 && stats.rejected_or_failed == 6 && stats.spawn_ok == 0);
        puts("SPAWN_AS host errors sanitized"); return 0;
    }
#endif
    /* The standard streams are this test's to pass on, all three of them; whatever else it was started with is closed. */
    for (int fd = 0; fd < 3; ++fd) if (fcntl(fd, F_GETFD) < 0) assert(open("/dev/null", O_RDWR) == fd);
    for (int fd = 3; fd < 256; ++fd) close(fd);
    int opened = descriptors(); unsigned looks = 0;
    const int privileged = geteuid() == 0;
    gid_t held[64]; int count = getgroups(64, held); assert(count >= 0);
    const dotnet_pal_identity own = {(uint32_t)getuid(), (uint32_t)getgid(), (const uint32_t*)held, (size_t)count};
    /* Each part gets its own watchdog: a child that never ends or a pipe that never closes fails the run instead of hanging it. */
    if (privileged) {
        alarm(20); identity_of_the_child();
        alarm(20); no_way_back();
        alarm(20); children_of_the_processes_group();
        alarm(20); signals();
        alarm(20); closed_streams();
        alarm(20); failures();
        alarm(20); descriptors_and_zombies();
        alarm(20); from_an_unprivileged_process();
        alarm(60); looks = threads();
    } else {
        alarm(20); without_privilege((const uint32_t*)held, (size_t)count);
    }
    alarm(20); validation(&own);
    assert(descriptors() == opened);
    no_children();
    assert(s->read_stats(&stats, sizeof stats) == 0 && p->read_stats(&handled, sizeof handled) == 0);
    /* Every start is counted once, in its own group; every child of either group was given back and every pipe closed through the processes group. */
    assert(stats.spawn_ok == children && stats.rejected_or_failed == refused && handled.spawn_ok == plain);
    assert(handled.release_ok == children + plain && handled.pipe_close_ok == pipes_made);
    assert(privileged ? children >= 120 && refused >= 35 : children >= 2 && refused >= 31);
    alarm(0);
    if (!privileged) puts("SPAWN_AS note: not root, so only what a process without the privilege may and may not do was checked");
    printf("SPAWN_AS PASS privileged=%d children=%u refused=%u plain=%u pipes=%u looks_at_threads=%u\n", privileged, (unsigned)children, (unsigned)refused,
        (unsigned)plain, (unsigned)pipes_made, looks);
    return 0;
}
