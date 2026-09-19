/* Independent POSIX reference provider for the host-system conformance suite. It
 * asks other sources than the Linux provider where the system has them: procfs
 * for the operating system texts and the uptime, realpath for the executable,
 * times() for the CPU time, the non-reentrant passwd lookup, getresuid. Fault 1
 * offers a table without the capability bit; fault 2 breaks the output contracts
 * so the front end's sanitizing is observable; fault 3 withholds every callback. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <limits.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/times.h>
#include <unistd.h>
extern char **environ;
int pal_system_fault;
static uint32_t deliver(const char *text, uint8_t *out, size_t capacity, size_t *needed) {
    if (!*text) return DOTNET_PAL_UNSUPPORTED; /* the boundary has no empty text */
    *needed = strlen(text) + 1;
    if (*needed > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, text, *needed); return DOTNET_PAL_OK;
}
/* A broken answer: the bytes as they are, the length the provider states and its status. */
static uint32_t broken(uint8_t *out, size_t capacity, size_t *needed, const char *bytes, size_t size, size_t stated, uint32_t status) {
    if (size <= capacity) memcpy(out, bytes, size);
    *needed = stated; return status;
}
static uint32_t environment_entry(size_t index, uint8_t *out, size_t capacity, size_t *needed) {
    if (pal_system_fault == 2) {
        static int step;
        switch (step++) {
        case 0: return broken(out, capacity, needed, "NOEQUALS", 9, 9, DOTNET_PAL_OK);              /* not a variable */
        case 1: return broken(out, capacity, needed, "=value", 7, 7, DOTNET_PAL_OK);                /* no name */
        case 2: return broken(out, capacity, needed, "A=bX", 4, 4, DOTNET_PAL_OK);                  /* no terminator */
        case 3: return broken(out, capacity, needed, "A=\0b", 5, 5, DOTNET_PAL_OK);                 /* terminator inside the text */
        case 4: return broken(out, capacity, needed, "A=b", 4, 0, DOTNET_PAL_OK);                   /* no length */
        case 5: return broken(out, capacity, needed, "", 1, 1, DOTNET_PAL_OK);                      /* empty text */
        case 6: return broken(out, capacity, needed, "A=b", 4, DOTNET_PAL_MAX_ENVIRONMENT_ENTRY + 1, DOTNET_PAL_BUFFER_TOO_SMALL); /* longer than any entry */
        case 7: return broken(out, capacity, needed, "A=b", 4, 4, 99u);                             /* no such status */
        case 8: return broken(out, capacity, needed, "A=b", 4, 4, DOTNET_PAL_TIMEOUT);              /* a status of the kernel group */
        default: return broken(out, capacity, needed, "A=b", 4, 4, DOTNET_PAL_NOT_FOUND);           /* outputs written by a failing call */
        }
    }
    for (char **entry = environ; entry && *entry; ++entry) {
        const char *equals = strchr(*entry, '=');
        if (!equals || equals == *entry) continue; /* not a variable: left out, so the indices stay dense */
        if (index-- == 0) return deliver(*entry, out, capacity, needed);
    }
    return DOTNET_PAL_NOT_FOUND;
}
static uint32_t first_line(const char *path, uint8_t *out, size_t capacity, size_t *needed) {
    char line[512];
    FILE *file = fopen(path, "r");
    if (!file) return errno == ENOENT ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
    char *got = fgets(line, sizeof line, file);
    fclose(file);
    if (!got) return DOTNET_PAL_OS_ERROR;
    line[strcspn(line, "\n")] = 0;
    return deliver(line, out, capacity, needed);
}
static uint32_t text(uint32_t what, uint8_t *out, size_t capacity, size_t *needed) {
    if (pal_system_fault == 2) {
        static int step;
        switch (step++) {
        case 0: return broken(out, capacity, needed, "Linux", 6, 6, DOTNET_PAL_NOT_FOUND);          /* a status only the enumeration has */
        case 1: return broken(out, capacity, needed, "Linux", 5, 5, DOTNET_PAL_OK);                 /* no terminator */
        case 2: return broken(out, capacity, needed, "", 1, 1, DOTNET_PAL_OK);                      /* empty text */
        case 3: return broken(out, capacity, needed, "Linux", 6, 0, DOTNET_PAL_OK);                 /* no length */
        case 4: return broken(out, capacity, needed, "Linux", 6, DOTNET_PAL_MAX_NAME + 2, DOTNET_PAL_BUFFER_TOO_SMALL); /* longer than any text */
        case 5: return broken(out, capacity, needed, "Linux", 6, 6, 99u);                           /* no such status */
        default: return broken(out, capacity, needed, "", 0, 6, DOTNET_PAL_BUFFER_TOO_SMALL);       /* too small for a text that fits */
        }
    }
    char path[PATH_MAX];
    struct passwd *entry;
    switch (what) {
    case DOTNET_PAL_TEXT_EXECUTABLE_PATH:
        if (!realpath("/proc/self/exe", path)) return errno == ENOENT ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
        return deliver(path, out, capacity, needed);
    case DOTNET_PAL_TEXT_OS_NAME: return first_line("/proc/sys/kernel/ostype", out, capacity, needed);
    case DOTNET_PAL_TEXT_OS_RELEASE: return first_line("/proc/sys/kernel/osrelease", out, capacity, needed);
    case DOTNET_PAL_TEXT_OS_VERSION: return first_line("/proc/sys/kernel/version", out, capacity, needed);
    case DOTNET_PAL_TEXT_USER_NAME: case DOTNET_PAL_TEXT_HOME_DIRECTORY:
        errno = 0;
        entry = getpwuid(geteuid());
        /* A user without an entry is a question this system has no answer to. */
        if (!entry) return errno == 0 || errno == ENOENT || errno == ESRCH ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
        return deliver(what == DOTNET_PAL_TEXT_USER_NAME ? entry->pw_name : entry->pw_dir, out, capacity, needed);
    default: return DOTNET_PAL_INVALID_ARGUMENT;
    }
}
static uint32_t process_times(uint64_t *user_ns, uint64_t *kernel_ns) {
    if (pal_system_fault == 2) { static int step; *user_ns = 1; *kernel_ns = 2; return step++ ? DOTNET_PAL_NOT_FOUND : DOTNET_PAL_OS_ERROR; } /* outputs written by a failing call */
    struct tms spent;
    long ticks = sysconf(_SC_CLK_TCK);
    if (ticks <= 0 || times(&spent) == (clock_t)-1) return DOTNET_PAL_OS_ERROR;
    *user_ns = (uint64_t)spent.tms_utime * (UINT64_C(1000000000) / (uint64_t)ticks);
    *kernel_ns = (uint64_t)spent.tms_stime * (UINT64_C(1000000000) / (uint64_t)ticks);
    return DOTNET_PAL_OK;
}
static uint32_t uptime_ns(uint64_t *out) {
    if (pal_system_fault == 2) { *out = 3; return DOTNET_PAL_BUFFER_TOO_SMALL; } /* a status only the texts have */
    unsigned long long seconds; unsigned hundredths;
    FILE *file = fopen("/proc/uptime", "r");
    if (!file) return errno == ENOENT ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
    int fields = fscanf(file, "%llu.%2u", &seconds, &hundredths);
    fclose(file);
    if (fields != 2) return DOTNET_PAL_OS_ERROR;
    *out = seconds * UINT64_C(1000000000) + hundredths * UINT64_C(10000000);
    return DOTNET_PAL_OK;
}
static uint32_t user_ids(uint32_t *user, uint32_t *group) {
    if (pal_system_fault == 2) { *user = 4; *group = 5; return 99u; } /* no such status */
    uid_t real_user, effective_user, saved_user; gid_t real_group, effective_group, saved_group;
    if (getresuid(&real_user, &effective_user, &saved_user) != 0 || getresgid(&real_group, &effective_group, &saved_group) != 0) return DOTNET_PAL_OS_ERROR;
    *user = effective_user; *group = effective_group;
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_system table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_system), DOTNET_PAL_CAP_SYSTEM},
    {environment_entry, text, process_times, uptime_ns, user_ids, NULL},
};
/* Every question is optional, so no missing callback rejects a table: a header that does not offer the group does. */
static const dotnet_pal_host_system malformed = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_system), 0},
    {environment_entry, text, process_times, uptime_ns, user_ids, NULL},
};
static const dotnet_pal_host_system silent = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_system), DOTNET_PAL_CAP_SYSTEM}, {NULL, NULL, NULL, NULL, NULL, NULL}};
const dotnet_pal_host_system *dotnet_pal_host_system_v2(void) { return pal_system_fault == 1 ? &malformed : pal_system_fault == 3 ? &silent : &table; }
