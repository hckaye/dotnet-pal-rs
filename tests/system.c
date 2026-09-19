/* Conformance test of the system group on Linux: every answer is checked against
 * what the C library and the kernel say about the same process. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
extern char **environ;
#ifdef PAL_HOST_TEST
extern int pal_system_fault;
/* The host table reads the CPU time in clock ticks and the uptime in hundredths of a second. */
#define SLACK UINT64_C(10000000)
#else
#define SLACK UINT64_C(0)
#endif
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define SMALL DOTNET_PAL_BUFFER_TOO_SMALL
#define CAPACITY DOTNET_PAL_MAX_ENVIRONMENT_ENTRY
#define LONGEST (DOTNET_PAL_MAX_ENVIRONMENT_ENTRY - 1)
#define MS UINT64_C(1000000)
static const dotnet_pal_system_ops *s;
static uint8_t out[CAPACITY + 1];
static int zero(const uint8_t *bytes, size_t size) { while (size--) if (*bytes++) return 0; return 1; }
static uint64_t ns(struct timespec time) { return (uint64_t)time.tv_sec * UINT64_C(1000000000) + (uint64_t)time.tv_nsec; }
static uint64_t us(struct timeval time) { return (uint64_t)time.tv_sec * UINT64_C(1000000000) + (uint64_t)time.tv_usec * 1000; }
/* Entry `which` of the environment, or the text `which` selects. */
static uint32_t ask(int environment, size_t which, uint8_t *buffer, size_t capacity, size_t *needed) {
    return environment ? s->environment_entry(which, buffer, capacity, needed) : s->text((uint32_t)which, buffer, capacity, needed);
}
/* The answer is `expected`: whole with its NUL where it fits, cleared behind it and never past the capacity; the length alone, and a cleared buffer, where it does not. */
static void delivers(int environment, size_t which, const char *expected) {
    size_t length = strlen(expected) + 1, needed = 7;
    assert(length >= 2 && length <= CAPACITY);
    memset(out, 0xAA, sizeof out);
    assert(ask(environment, which, out, CAPACITY, &needed) == 0 && needed == length && memcmp(out, expected, length) == 0);
    assert(zero(out + length, CAPACITY - length) && out[CAPACITY] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(ask(environment, which, out, length, &needed) == 0 && needed == length && memcmp(out, expected, length) == 0 && out[length] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(ask(environment, which, out, length - 1, &needed) == SMALL && needed == length && zero(out, length - 1) && out[length - 1] == 0xAA);
    needed = 7;
    assert(ask(environment, which, NULL, 0, &needed) == SMALL && needed == length);
}
static void refuses(int environment, size_t which, uint32_t status) {
    size_t needed = 7;
    memset(out, 0xAA, sizeof out);
    assert(ask(environment, which, out, CAPACITY, &needed) == status && needed == 0 && zero(out, CAPACITY) && out[CAPACITY] == 0xAA);
}
/* Spends CPU time in user code and in the kernel until the kernel has accounted `user` and `kernel` more of each, or 20 seconds have passed. */
static void burn(uint64_t user, uint64_t kernel) {
    static uint8_t page[1 << 20];
    struct rusage start, now; struct timespec began, time;
    int source = open("/dev/zero", O_RDONLY | O_CLOEXEC);
    assert(source >= 0 && getrusage(RUSAGE_SELF, &start) == 0 && clock_gettime(CLOCK_MONOTONIC, &began) == 0);
    do {
        volatile uint64_t sum = 0;
        for (uint64_t i = 0; i < 2000000; ++i) sum += i * i;
        for (int i = 0; i < 16; ++i) assert(read(source, page, sizeof page) == (ssize_t)sizeof page);
        assert(getrusage(RUSAGE_SELF, &now) == 0 && clock_gettime(CLOCK_MONOTONIC, &time) == 0);
    } while ((us(now.ru_utime) < us(start.ru_utime) + user || us(now.ru_stime) < us(start.ru_stime) + kernel) && ns(time) < ns(began) + 20000 * MS);
    close(source);
}
/* A user the passwd database does not know: no name and no home, and the ids are still the process's own. */
static int nameless(void) {
    uint32_t user = 7, group = 7;
    if (setgroups(0, NULL) != 0 || setgid(54321) != 0 || setuid(54321) != 0 || getpwuid(54321)) return 77;
    refuses(0, DOTNET_PAL_TEXT_USER_NAME, DOTNET_PAL_UNSUPPORTED);
    refuses(0, DOTNET_PAL_TEXT_HOME_DIRECTORY, DOTNET_PAL_UNSUPPORTED);
    assert(s->user_ids(&user, &group) == 0 && user == 54321 && group == 54321);
    return 0;
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_system_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    /* The environment as the process finds it, plus: an empty value, a value with '=', a long one, the longest text the boundary
     * carries and one byte more, and two strings that are no variables, which only a parent or the program itself can put there. */
    static char longest[3001], limit[LONGEST - 16], oversized[LONGEST - 19];
    memset(longest, 'v', sizeof longest - 1); memset(limit, 'l', sizeof limit - 1); memset(oversized, 'o', sizeof oversized - 1);
    assert(setenv("PAL_SYSTEM_PLAIN", "value", 1) == 0 && setenv("PAL_SYSTEM_EMPTY", "", 1) == 0 && setenv("PAL_SYSTEM_EQUALS", "a=b=", 1) == 0);
    assert(setenv("PAL_SYSTEM_LONG", longest, 1) == 0 && setenv("PAL_SYSTEM_LIMIT", limit, 1) == 0 && setenv("PAL_SYSTEM_OVERSIZED", oversized, 1) == 0);
    size_t present = 0;
    while (environ[present]) ++present;
    char **block = calloc(present + 3, sizeof *block);
    assert(block);
    block[0] = "PAL_SYSTEM_NOT_A_VARIABLE";
    memcpy(block + 1, environ, (present / 2) * sizeof *block);
    block[1 + present / 2] = "=PAL_SYSTEM_NAMELESS";
    memcpy(block + 2 + present / 2, environ + present / 2, (present - present / 2) * sizeof *block);
    environ = block;

    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_system_fault == 1) { assert(!api); puts("SYSTEM malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_SYSTEM_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_SYSTEM);
    s = &api->system;
    assert(s->environment_entry && s->text && s->process_times && s->uptime_ns && s->user_ids && s->read_stats);
    size_t needed = 7; uint64_t user = 7, kernel = 7, uptime = 7; uint32_t uid = 7, gid = 7;
    dotnet_pal_system_stats before, after;
#ifdef PAL_HOST_TEST
    if (pal_system_fault == 2) {
        /* A text that is no variable, malformed, of an impossible length or behind an unknown status is refused and cleared. */
        for (int i = 0; i < 9; ++i) refuses(1, 0, DOTNET_PAL_OS_ERROR);
        refuses(1, 0, DOTNET_PAL_NOT_FOUND); /* the end is the enumeration's to report, without the outputs of the call that reported it */
        for (int i = 0; i < 7; ++i) refuses(0, DOTNET_PAL_TEXT_OS_NAME, DOTNET_PAL_OS_ERROR);
        for (int i = 0; i < 2; ++i) { user = kernel = 7; assert(s->process_times(&user, &kernel) == DOTNET_PAL_OS_ERROR && user == 0 && kernel == 0); }
        assert(s->uptime_ns(&uptime) == DOTNET_PAL_OS_ERROR && uptime == 0);
        assert(s->user_ids(&uid, &gid) == DOTNET_PAL_OS_ERROR && uid == 0 && gid == 0);
        assert(s->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == 21);
        assert(after.environment_ok + after.text_ok + after.times_ok + after.identity_ok == 0);
        puts("SYSTEM host errors sanitized"); return 0;
    }
    if (pal_system_fault == 3) {
        /* A host answers the questions it can: each of the others is UNSUPPORTED, and the group is still there. */
        refuses(1, 0, DOTNET_PAL_UNSUPPORTED);
        for (uint32_t what = DOTNET_PAL_TEXT_EXECUTABLE_PATH; what <= DOTNET_PAL_TEXT_HOME_DIRECTORY; ++what) refuses(0, what, DOTNET_PAL_UNSUPPORTED);
        assert(s->process_times(&user, &kernel) == DOTNET_PAL_UNSUPPORTED && user == 0 && kernel == 0);
        assert(s->uptime_ns(&uptime) == DOTNET_PAL_UNSUPPORTED && uptime == 0);
        assert(s->user_ids(&uid, &gid) == DOTNET_PAL_UNSUPPORTED && uid == 0 && gid == 0);
        assert(s->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == 10);
        assert(after.environment_ok + after.text_ok + after.times_ok + after.identity_ok == 0);
        puts("SYSTEM absent host callbacks unsupported"); return 0;
    }
#endif
    /* Enumeration: the variables of environ in its order, the two strings that are none left out, NOT_FOUND from the count on.
     * A text longer than the boundary's longest entry keeps its index and is OS_ERROR. */
    size_t count = 0, own = 0, skipped = 0, too_long = 0;
    for (char **entry = environ; *entry; ++entry) {
        const char *equals = strchr(*entry, '=');
        if (!equals || equals == *entry) { ++skipped; continue; }
        if (strncmp(*entry, "PAL_SYSTEM_", 11) == 0) ++own;
        if (strlen(*entry) > LONGEST) { refuses(1, count++, DOTNET_PAL_OS_ERROR); ++too_long; continue; }
        delivers(1, count++, *entry);
    }
    assert(count + skipped == present + 2 && skipped == 2 && own == 6 && too_long >= 1);
    assert(strlen("PAL_SYSTEM_LIMIT=") + strlen(limit) == LONGEST && strlen("PAL_SYSTEM_OVERSIZED=") + strlen(oversized) == LONGEST + 1);
    refuses(1, count, DOTNET_PAL_NOT_FOUND); refuses(1, count + 1, DOTNET_PAL_NOT_FOUND); refuses(1, SIZE_MAX, DOTNET_PAL_NOT_FOUND);

    /* Texts: the executable behind /proc/self/exe, uname, and the passwd entry of the effective user. */
    char executable[CAPACITY] = {0}, name[256] = {0}, home[CAPACITY] = {0};
    struct utsname names;
    assert(readlink("/proc/self/exe", executable, sizeof executable - 1) > 0 && uname(&names) == 0);
    delivers(0, DOTNET_PAL_TEXT_EXECUTABLE_PATH, executable);
    delivers(0, DOTNET_PAL_TEXT_OS_NAME, names.sysname);
    delivers(0, DOTNET_PAL_TEXT_OS_RELEASE, names.release);
    delivers(0, DOTNET_PAL_TEXT_OS_VERSION, names.version);
    struct passwd *entry = getpwuid(geteuid());
    if (entry) {
        snprintf(name, sizeof name, "%s", entry->pw_name); snprintf(home, sizeof home, "%s", entry->pw_dir);
        delivers(0, DOTNET_PAL_TEXT_USER_NAME, name);
        delivers(0, DOTNET_PAL_TEXT_HOME_DIRECTORY, home);
    } else {
        refuses(0, DOTNET_PAL_TEXT_USER_NAME, DOTNET_PAL_UNSUPPORTED);
        refuses(0, DOTNET_PAL_TEXT_HOME_DIRECTORY, DOTNET_PAL_UNSUPPORTED);
    }
    /* Root can ask again as a user without an entry, in a child that gives its privileges up. */
    int unknown_user = entry ? -1 : 1;
    if (geteuid() == 0) {
        pid_t child = fork(); assert(child >= 0);
        if (child == 0) _exit(nameless());
        int result = 0;
        assert(waitpid(child, &result, 0) == child && WIFEXITED(result) && (WEXITSTATUS(result) == 0 || WEXITSTATUS(result) == 77));
        if (WEXITSTATUS(result) == 0) unknown_user = 1;
    }

    /* CPU time: what getrusage says around the call, and more of both kinds after work of both kinds. */
    struct rusage earlier, later;
    assert(getrusage(RUSAGE_SELF, &earlier) == 0 && s->process_times(&user, &kernel) == 0 && getrusage(RUSAGE_SELF, &later) == 0);
    assert(user + SLACK >= us(earlier.ru_utime) && user <= us(later.ru_utime) && kernel + SLACK >= us(earlier.ru_stime) && kernel <= us(later.ru_stime));
    burn(100 * MS, 50 * MS);
    uint64_t user_later = 7, kernel_later = 7;
    assert(getrusage(RUSAGE_SELF, &earlier) == 0 && s->process_times(&user_later, &kernel_later) == 0 && getrusage(RUSAGE_SELF, &later) == 0);
    assert(user_later + SLACK >= us(earlier.ru_utime) && user_later <= us(later.ru_utime) && kernel_later + SLACK >= us(earlier.ru_stime) && kernel_later <= us(later.ru_stime));
    assert(user_later + SLACK >= user + 100 * MS && kernel_later + SLACK >= kernel + 50 * MS);

    /* Uptime is the clock that keeps counting while the machine is suspended. */
    struct timespec earliest, latest;
    assert(clock_gettime(CLOCK_BOOTTIME, &earliest) == 0 && s->uptime_ns(&uptime) == 0 && clock_gettime(CLOCK_BOOTTIME, &latest) == 0);
    assert(uptime + SLACK >= ns(earliest) && uptime <= ns(latest));
    assert(s->user_ids(&uid, &gid) == 0 && uid == geteuid() && gid == getegid());

    /* Argument validation: refused before a provider runs. */
    assert(s->environment_entry(0, out, CAPACITY, NULL) == INVALID && s->environment_entry(0, NULL, 8, &needed) == INVALID);
    assert(s->environment_entry(0, out, SIZE_MAX, &needed) == INVALID);
    assert(s->text(DOTNET_PAL_TEXT_OS_NAME, out, CAPACITY, NULL) == INVALID && s->text(DOTNET_PAL_TEXT_OS_NAME, NULL, 8, &needed) == INVALID);
    needed = 7;
    assert(s->text(0, out, CAPACITY, &needed) == INVALID && needed == 0);
    needed = 7;
    assert(s->text(DOTNET_PAL_TEXT_HOME_DIRECTORY + 1, out, CAPACITY, &needed) == INVALID && needed == 0 && s->text(UINT32_MAX, out, CAPACITY, &needed) == INVALID);
    assert(s->process_times(NULL, &kernel) == INVALID && s->process_times(&user, NULL) == INVALID && s->uptime_ns(NULL) == INVALID);
    assert(s->user_ids(NULL, &gid) == INVALID && s->user_ids(&uid, NULL) == INVALID);
    assert(s->read_stats(NULL, sizeof after) == INVALID && s->read_stats(&after, sizeof after - 1) == INVALID);

    /* Counters: one per class of answered question, one for everything refused. */
    assert(s->read_stats(&before, sizeof before) == 0);
    assert(s->environment_entry(0, out, CAPACITY, &needed) == 0 && s->text(DOTNET_PAL_TEXT_OS_NAME, out, CAPACITY, &needed) == 0);
    assert(s->process_times(&user, &kernel) == 0 && s->uptime_ns(&uptime) == 0 && s->user_ids(&uid, &gid) == 0);
    assert(s->environment_entry(count, out, CAPACITY, &needed) == DOTNET_PAL_NOT_FOUND && s->text(DOTNET_PAL_TEXT_OS_NAME, out, 1, &needed) == SMALL);
    assert(s->text(0, out, CAPACITY, &needed) == INVALID && s->uptime_ns(NULL) == INVALID);
    assert(s->read_stats(&after, sizeof after) == 0);
    assert(after.environment_ok == before.environment_ok + 1 && after.text_ok == before.text_ok + 1 && after.times_ok == before.times_ok + 2);
    assert(after.identity_ok == before.identity_ok + 1 && after.rejected_or_failed == before.rejected_or_failed + 4);
    assert(before.environment_ok == 2 * (count - too_long) && before.identity_ok == 1 && before.times_ok == 3);
    /* clearenv leaves the C library without a block at all: an environment without a first entry. */
    assert(clearenv() == 0 && environ == NULL);
    refuses(1, 0, DOTNET_PAL_NOT_FOUND);
    printf("SYSTEM PASS variables=%zu too_long=%zu os=%s %s user=%s uid=%u gid=%u unknown_user=%d cpu_ms=%llu+%llu uptime_s=%llu\n", count, too_long,
        names.sysname, names.release, name, uid, gid, unknown_user, (unsigned long long)(user / MS), (unsigned long long)(kernel / MS), (unsigned long long)(uptime / (1000 * MS)));
    return 0;
}
