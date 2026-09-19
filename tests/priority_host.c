/* Independent reference provider for the host-priority conformance suite. It reads
 * a nice value from the stat file of procfs instead of asking getpriority, and
 * like the Linux provider it changes every thread of a process, because the
 * kernel keeps one value a thread. Faults 1, 3 and 4 offer a table the front end
 * must reject: without set, without get, without the capability bit. Fault 2
 * breaks the output contracts so the front end's sanitizing is observable. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
int pal_priority_fault;
/* The nice value in a stat file of procfs: the 19th field, counted behind the command, which may hold spaces and parentheses. */
static uint32_t stat_nice(const char *path, int32_t *value) {
    char line[1024];
    FILE *file = fopen(path, "r");
    if (!file) return errno == ENOENT || errno == ESRCH ? DOTNET_PAL_NOT_FOUND : DOTNET_PAL_OS_ERROR;
    char *got = fgets(line, sizeof line, file);
    fclose(file);
    /* A process that ended while its file was open reads as nothing. */
    if (!got) return DOTNET_PAL_NOT_FOUND;
    char *at = strrchr(line, ')');
    if (!at) return DOTNET_PAL_OS_ERROR;
    for (int field = 2; field < 19; ++field) { at = strchr(at + 1, ' '); if (!at) return DOTNET_PAL_OS_ERROR; }
    *value = (int32_t)strtol(at + 1, NULL, 10);
    return DOTNET_PAL_OK;
}
static uint32_t get(uint64_t process, int32_t *value) {
    if (pal_priority_fault == 2) {
        static int step;
        switch (step++) {
        case 0: *value = 25; return DOTNET_PAL_OK;                  /* less favoured than any value */
        case 1: *value = -30; return DOTNET_PAL_OK;                 /* more favoured than any value */
        case 2: *value = 20; return DOTNET_PAL_OK;                  /* one past the end */
        case 3: *value = -21; return DOTNET_PAL_OK;                 /* one before the start */
        case 4: *value = 5; return 99u;                             /* no such status */
        case 5: *value = 5; return DOTNET_PAL_ACCESS_DENIED;        /* a status only set has */
        case 6: *value = 5; return DOTNET_PAL_TIMEOUT;              /* a status of the kernel group */
        case 7: *value = 5; return DOTNET_PAL_NOT_FOUND;            /* an output written by a failing call */
        case 8: *value = 19; return DOTNET_PAL_OK;                  /* the two ends are values */
        default: *value = -20; return DOTNET_PAL_OK;
        }
    }
    char path[64];
    if (process > INT_MAX) return DOTNET_PAL_NOT_FOUND;
    if (process) snprintf(path, sizeof path, "/proc/%llu/stat", (unsigned long long)process); else snprintf(path, sizeof path, "/proc/self/stat");
    return stat_nice(path, value);
}
static uint32_t refusal(void) { return errno == ESRCH ? DOTNET_PAL_NOT_FOUND : errno == EACCES || errno == EPERM ? DOTNET_PAL_ACCESS_DENIED : DOTNET_PAL_OS_ERROR; }
static uint32_t set(uint64_t process, int32_t value) {
    if (pal_priority_fault == 2) {
        static int step;
        switch (step++) {
        case 0: return 99u;                                         /* no such status */
        case 1: return DOTNET_PAL_BUFFER_TOO_SMALL;                 /* a status of the runtime group */
        case 2: return DOTNET_PAL_ACCESS_DENIED;
        case 3: return DOTNET_PAL_NOT_FOUND;
        default: return DOTNET_PAL_OK;
        }
    }
    if (process > INT_MAX) return DOTNET_PAL_NOT_FOUND;
    pid_t leader = process ? (pid_t)process : getpid();
    /* The thread that carries the process id answers for the process, before anything has changed. */
    if (setpriority(PRIO_PROCESS, (id_t)leader, value) != 0) return refusal();
    char path[96]; uint32_t status = DOTNET_PAL_OK;
    for (int pass = 0; pass < 16 && status == DOTNET_PAL_OK; ++pass) {
        snprintf(path, sizeof path, "/proc/%d/task", (int)leader);
        DIR *threads = opendir(path);
        if (!threads) return DOTNET_PAL_OK; /* the process ended meanwhile, or there is no procfs */
        int changed = 0; struct dirent *entry;
        while ((entry = readdir(threads)) != NULL) {
            char *end; long thread = strtol(entry->d_name, &end, 10); int32_t current;
            if (end == entry->d_name || *end || thread == leader) continue;
            snprintf(path, sizeof path, "/proc/%d/task/%ld/stat", (int)leader, thread);
            if (stat_nice(path, &current) != DOTNET_PAL_OK || current == value) continue;
            if (setpriority(PRIO_PROCESS, (id_t)thread, value) == 0) changed = 1;
            else if (errno != ESRCH && status == DOTNET_PAL_OK) status = refusal();
        }
        closedir(threads);
        if (!changed) return status;
    }
    return status == DOTNET_PAL_OK ? DOTNET_PAL_OS_ERROR : status;
}
static const dotnet_pal_host_priority table = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_priority), DOTNET_PAL_CAP_PRIORITY}, {get, set, NULL}};
/* A priority that can be read and not changed, or the other way round, is no priority group. */
static const dotnet_pal_host_priority no_set = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_priority), DOTNET_PAL_CAP_PRIORITY}, {get, NULL, NULL}};
static const dotnet_pal_host_priority no_get = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_priority), DOTNET_PAL_CAP_PRIORITY}, {NULL, set, NULL}};
static const dotnet_pal_host_priority no_bit = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_priority), 0}, {get, set, NULL}};
const dotnet_pal_host_priority *dotnet_pal_host_priority_v2(void) {
    return pal_priority_fault == 1 ? &no_set : pal_priority_fault == 3 ? &no_get : pal_priority_fault == 4 ? &no_bit : &table;
}
