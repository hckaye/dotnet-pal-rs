/* Conformance test of the priority group on Linux: every answer is checked against
 * getpriority and against the stat files of procfs, for this process with several
 * threads, for a child by its id and for a child with several threads. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <grp.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_priority_fault;
#endif
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define DENIED DOTNET_PAL_ACCESS_DENIED
#define MISSING DOTNET_PAL_NOT_FOUND
static const dotnet_pal_priority_ops *p;
static dotnet_pal_priority_stats expected;
static atomic_int stop, own_value;
static atomic_long idle_thread, other_thread;

/* One get: the status, and the value, which a refusal leaves 0. */
static uint32_t get(uint64_t process, int32_t *value) {
    *value = 77;
    uint32_t status = p->get(process, value);
    if (status == 0) ++expected.get_ok; else { ++expected.rejected_or_failed; assert(*value == 0); }
    return status;
}
static uint32_t set(uint64_t process, int32_t value) {
    uint32_t status = p->set(process, value);
    if (status == 0) ++expected.set_ok; else ++expected.rejected_or_failed;
    return status;
}
static int nice_of(id_t thread) { errno = 0; int value = getpriority(PRIO_PROCESS, thread); assert(value != -1 || errno == 0); return value; }
/* The nice value in a stat file of procfs: the 19th field, counted behind the command. */
static int stat_nice(const char *path) {
    char line[1024];
    FILE *file = fopen(path, "r");
    assert(file && fgets(line, sizeof line, file));
    fclose(file);
    char *at = strrchr(line, ')');
    for (int field = 2; field < 19; ++field) { assert(at); at = strchr(at + 1, ' '); }
    assert(at);
    return atoi(at + 1);
}
/* Every thread of the process has the value, by getpriority and by procfs; the number of threads. Callers ask for
 * at least the threads they started: a sanitizer runtime adds one of its own at a moment of its choosing. */
static int whole_process(pid_t process, int value) {
    char path[320]; int threads = 0; struct dirent *entry;
    snprintf(path, sizeof path, "/proc/%d/task", (int)process);
    DIR *list = opendir(path);
    assert(list);
    while ((entry = readdir(list)) != NULL) {
        if (entry->d_name[0] == '.') continue;
        snprintf(path, sizeof path, "/proc/%d/task/%s/stat", (int)process, entry->d_name);
        assert(stat_nice(path) == value && nice_of((id_t)atoi(entry->d_name)) == value);
        ++threads;
    }
    closedir(list);
    return threads;
}
static void *idle(void *argument) {
    atomic_store((atomic_long *)argument, syscall(SYS_gettid));
    while (!atomic_load(&stop)) usleep(1000);
    return NULL;
}
/* A thread that gives itself another value than its process has: to the kernel "0" is this thread, to the boundary it is this process. */
static void *apart(void *argument) {
    int32_t value; long thread = syscall(SYS_gettid);
    assert(setpriority(PRIO_PROCESS, (id_t)thread, 9) == 0 && nice_of(0) == 9 && nice_of((id_t)getpid()) == 5);
    uint32_t status = p->get(0, &value);
    assert(status == 0 && value == 5);
    atomic_store(&own_value, 1);
    return idle(argument);
}
/* Without privileges: the priority goes down and not up again, and another user's process is out of reach. */
static int unprivileged(pid_t parent) {
    int32_t value; dotnet_pal_priority_stats before, after, mine = expected;
    assert(p->read_stats(&before, sizeof before) == 0);
    if (setgroups(0, NULL) != 0 || setgid(54321) != 0 || setuid(54321) != 0) return 77;
    assert(set(0, 0) == DENIED && get(0, &value) == 0 && value == 10 && nice_of(0) == 10);
    assert(set(0, 9) == DENIED && set(0, 12) == 0 && get(0, &value) == 0 && value == 12 && nice_of(0) == 12);
    assert(set((uint64_t)parent, 15) == DENIED && get((uint64_t)parent, &value) == 0 && value == 10 && nice_of((id_t)parent) == 10);
    assert(p->read_stats(&after, sizeof after) == 0);
    assert(after.get_ok - before.get_ok == expected.get_ok - mine.get_ok && after.set_ok - before.set_ok == expected.set_ok - mine.set_ok);
    assert(after.rejected_or_failed - before.rejected_or_failed == expected.rejected_or_failed - mine.rejected_or_failed && after.rejected_or_failed - before.rejected_or_failed == 3);
    return 0;
}
/* A child with two more threads that tells through the pipe when they run, and then waits to be ended. */
static pid_t threaded_child(void) {
    int ready[2]; char byte;
    assert(pipe(ready) == 0);
    pid_t child = fork(); assert(child >= 0);
    if (child == 0) {
        pthread_t threads[2]; atomic_long ids[2] = {0, 0};
        for (int i = 0; i < 2; ++i) if (pthread_create(&threads[i], NULL, idle, &ids[i]) != 0) _exit(1);
        while (!atomic_load(&ids[0]) || !atomic_load(&ids[1])) usleep(1000);
        if (write(ready[1], "r", 1) != 1) _exit(1);
        for (;;) pause();
    }
    close(ready[1]);
    assert(read(ready[0], &byte, 1) == 1);
    close(ready[0]);
    return child;
}
/* Whether this process may make itself more favoured again: CAP_SYS_NICE, which the root of a default container has not got, or a nice limit that allows it. */
static int may_lower(void) {
    pid_t child = fork(); assert(child >= 0);
    if (child == 0) _exit(setpriority(PRIO_PROCESS, 0, 19) == 0 && setpriority(PRIO_PROCESS, 0, -20) == 0 ? 0 : 1);
    int result = 0;
    assert(waitpid(child, &result, 0) == child && WIFEXITED(result));
    return WEXITSTATUS(result) == 0;
}
static void reap(pid_t child) { int result; assert(kill(child, SIGKILL) == 0 && waitpid(child, &result, 0) == child); }

int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_priority_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_priority_fault == 1 || pal_priority_fault == 3 || pal_priority_fault == 4) { assert(!api); puts("PRIORITY malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_PRIORITY_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_PRIORITY);
    p = &api->priority;
    assert(p->get && p->set && p->read_stats);
    int32_t value; dotnet_pal_priority_stats stats;
#ifdef PAL_HOST_TEST
    if (pal_priority_fault == 2) {
        /* A value outside the range, an unknown status, a status get does not have: refused, and the value stays 0. */
        for (int i = 0; i < 7; ++i) assert(get(0, &value) == DOTNET_PAL_OS_ERROR);
        assert(get(0, &value) == MISSING); /* no such process is get's to report, without the output of the call that reported it */
        assert(get(0, &value) == 0 && value == 19 && get(0, &value) == 0 && value == -20);
        assert(set(0, 1) == DOTNET_PAL_OS_ERROR && set(0, 1) == DOTNET_PAL_OS_ERROR);
        /* A value that is none does not reach the provider: its next answer is still the third one. */
        assert(set(0, 20) == INVALID && set(0, 1) == DENIED && set(0, 1) == MISSING && set(0, 1) == 0);
        assert(p->read_stats(&stats, sizeof stats) == 0 && stats.get_ok == 2 && stats.set_ok == 1 && stats.rejected_or_failed == 8 + 5);
        assert(expected.get_ok == 2 && expected.set_ok == 1 && expected.rejected_or_failed == 13);
        puts("PRIORITY host errors sanitized"); return 0;
    }
#endif
    /* This process, which has a second thread from the start: its value is what getpriority says, and a new value is the value of every thread. */
    pthread_t threads[3];
    assert(pthread_create(&threads[0], NULL, idle, &idle_thread) == 0);
    while (!atomic_load(&idle_thread)) usleep(1000);
    int start = nice_of(0), root = geteuid() == 0;
    assert(get(0, &value) == 0 && value == start && (start <= 5 || root));
    assert(set(0, 5) == 0 && nice_of(0) == 5 && nice_of((id_t)getpid()) == 5 && get(0, &value) == 0 && value == 5 && get((uint64_t)getpid(), &value) == 0 && value == 5);
    assert(whole_process(getpid(), 5) >= 2);
    /* A thread with a value of its own does not change what the process answers, and is brought back by the next change. */
    assert(pthread_create(&threads[1], NULL, apart, &other_thread) == 0);
    while (!atomic_load(&other_thread)) usleep(1000);
    assert(atomic_load(&own_value) && nice_of((id_t)atomic_load(&other_thread)) == 9 && nice_of(0) == 5);
    expected.get_ok += 1; /* the one that thread asked */
    assert(get(0, &value) == 0 && value == 5);
    assert(set(0, 10) == 0 && whole_process(getpid(), 10) >= 3 && get(0, &value) == 0 && value == 10);
    /* A thread started now inherits the value. */
    atomic_long late = 0;
    assert(pthread_create(&threads[2], NULL, idle, &late) == 0);
    while (!atomic_load(&late)) usleep(1000);
    assert(whole_process(getpid(), 10) >= 4);

    /* Going back up is a privilege. A child that has given its privileges up has not got it; this process has it or not, and a refusal changes nothing. */
    int unprivileged_child = -1, lowering = may_lower();
    if (root) {
        pid_t child = fork(); assert(child >= 0);
        if (child == 0) _exit(unprivileged(getppid()));
        int result = 0;
        assert(waitpid(child, &result, 0) == child && WIFEXITED(result) && (WEXITSTATUS(result) == 0 || WEXITSTATUS(result) == 77));
        unprivileged_child = WEXITSTATUS(result) == 0;
    }
    if (unprivileged_child != 1) puts("PRIORITY note: not root, or the ids could not be changed: a refused change was not checked in a child without privileges");
    if (lowering) {
        assert(set(0, 0) == 0 && whole_process(getpid(), 0) >= 4 && get(0, &value) == 0 && value == 0);
        assert(set(0, -20) == 0 && whole_process(getpid(), -20) >= 4 && get(0, &value) == 0 && value == -20);
        /* -1 is a value, not a failure. */
        assert(set(0, -1) == 0 && whole_process(getpid(), -1) >= 4 && get(0, &value) == 0 && value == -1);
        assert(set(0, 19) == 0 && whole_process(getpid(), 19) >= 4 && get(0, &value) == 0 && value == 19);
        assert(set(0, 0) == 0 && whole_process(getpid(), 0) >= 4);
    } else {
        assert(set(0, 0) == DENIED && set(0, -1) == DENIED && whole_process(getpid(), 10) >= 4 && get(0, &value) == 0 && value == 10);
        puts("PRIORITY note: this process may not make itself more favoured (a container without CAP_SYS_NICE): the values below the one it has, -1 among them, were not checked");
    }
    /* A child by its id: getpriority and procfs see the value the boundary set, and the boundary reads what they see.
     * The values stand above the one this process has now, so the test needs no privilege for them: 7, 8 and 3 as root. */
    char path[64]; int base = nice_of(0);
    assert(base <= 10);
    pid_t child = fork(); assert(child >= 0);
    if (child == 0) { execlp("sleep", "sleep", "60", (char *)NULL); _exit(127); }
    assert(set((uint64_t)child, base + 7) == 0 && nice_of((id_t)child) == base + 7 && get((uint64_t)child, &value) == 0 && value == base + 7);
    snprintf(path, sizeof path, "/proc/%d/stat", (int)child);
    assert(stat_nice(path) == base + 7);
    assert(setpriority(PRIO_PROCESS, (id_t)child, base + 8) == 0 && get((uint64_t)child, &value) == 0 && value == base + 8);
    /* A child with threads: all of them, where setpriority alone changes the first. */
    pid_t crowd = threaded_child();
    assert(setpriority(PRIO_PROCESS, (id_t)crowd, base + 3) == 0 && nice_of((id_t)crowd) == base + 3);
    snprintf(path, sizeof path, "/proc/%d/task", (int)crowd);
    int first_only = 0; struct dirent *entry; DIR *list = opendir(path);
    assert(list);
    while ((entry = readdir(list)) != NULL) if (entry->d_name[0] != '.' && atoi(entry->d_name) != crowd) { assert(nice_of((id_t)atoi(entry->d_name)) == base); ++first_only; }
    closedir(list);
    assert(first_only == 2);
    assert(set((uint64_t)crowd, base + 8) == 0 && whole_process(crowd, base + 8) >= 3 && get((uint64_t)crowd, &value) == 0 && value == base + 8);
    reap(crowd);
    /* A process that has ended and been waited for is no process. */
    reap(child);
    assert(get((uint64_t)child, &value) == MISSING && set((uint64_t)child, 7) == MISSING);
    assert(get((uint64_t)crowd, &value) == MISSING && set((uint64_t)crowd, 7) == MISSING);

    /* An id no process can have is no process, and never this one or another by the low half of the number. */
    uint64_t none[] = {(uint64_t)INT32_MAX + 1, UINT32_MAX, (UINT64_C(1) << 32), (UINT64_C(1) << 32) + (uint64_t)getpid(), (UINT64_C(1) << 63) + (uint64_t)getpid(), UINT64_MAX};
    for (size_t i = 0; i < sizeof none / sizeof *none; ++i) assert(get(none[i], &value) == MISSING && set(none[i], 15) == MISSING);
    assert(nice_of(0) == base && get(0, &value) == 0 && value == base);
    /* Values that are none, and an answer that has no place. */
    int32_t wrong[] = {-21, 20, INT32_MIN, INT32_MAX};
    for (size_t i = 0; i < sizeof wrong / sizeof *wrong; ++i) assert(set(0, wrong[i]) == INVALID && set((uint64_t)getpid(), wrong[i]) == INVALID);
    assert(p->get(0, NULL) == INVALID && p->get(0, (int32_t *)((uint8_t *)&value + 1)) == INVALID);
    expected.rejected_or_failed += 2;
    assert(p->read_stats(NULL, sizeof stats) == INVALID && p->read_stats(&stats, sizeof stats - 1) == INVALID);

    atomic_store(&stop, 1);
    for (int i = 0; i < 3; ++i) assert(pthread_join(threads[i], NULL) == 0);
    /* Counters: one for the values read, one for the values set, one for everything refused. */
    assert(p->read_stats(&stats, sizeof stats) == 0);
    assert(stats.get_ok == expected.get_ok && stats.set_ok == expected.set_ok && stats.rejected_or_failed == expected.rejected_or_failed);
    printf("PRIORITY PASS start=%d root=%d unprivileged_child=%d lowering=%d get_ok=%llu set_ok=%llu refused=%llu\n", start, root, unprivileged_child, lowering,
        (unsigned long long)stats.get_ok, (unsigned long long)stats.set_ok, (unsigned long long)stats.rejected_or_failed);
    return 0;
}
