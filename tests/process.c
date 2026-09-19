/* Conformance test of the process group on Linux: debugger detection, the
 * crash-dump utility launch (with a shell standing in for createdump) and exit. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_process_fault;
#endif
static const dotnet_pal_process_ops *p;
static uint32_t launch(const char *script, char *error, size_t capacity) {
    const char *argv[] = {"/bin/sh", "-c", script};
    return p->crash_dump((const uint8_t *const *)argv, 3, (uint8_t*)error, capacity);
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_process_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_process_fault == 1) { assert(!api); puts("PROCESS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_PROCESS_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_PROCESS);
    p = &api->process;
    assert(p->exit && p->debugger_present && p->crash_dump);
    uint32_t present = 7;
#ifdef PAL_HOST_TEST
    if (pal_process_fault == 2) {
        assert(p->debugger_present(&present) == DOTNET_PAL_OS_ERROR && present == 0);
        char error[32] = "stale";
        assert(launch("exit 0", error, sizeof error) == DOTNET_PAL_OS_ERROR && error[sizeof error - 1] == 0);
        puts("PROCESS host errors sanitized"); return 0;
    }
#endif
    assert(p->debugger_present(&present) == 0 && present == 0); /* not traced when run by the scripts */
    assert(p->debugger_present(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    char error[128];
    memset(error, 'x', sizeof error);
    assert(launch("exit 0", error, sizeof error) == 0 && error[0] == 0);
    /* The utility's diagnostics come back through the error buffer, terminated. */
    memset(error, 'x', sizeof error);
    assert(launch("echo dump-failed >&2; exit 3", error, sizeof error) == DOTNET_PAL_OS_ERROR);
    assert(strstr(error, "dump-failed") != NULL && error[sizeof error - 1] == 0);
    /* A missing utility is a failure with a message, not a hang or a crash. */
    const char *missing[] = {"/nonexistent/createdump", "--nativeaot"};
    memset(error, 0, sizeof error);
    assert(p->crash_dump((const uint8_t *const *)missing, 2, (uint8_t*)error, sizeof error) == DOTNET_PAL_OS_ERROR);
    /* Zero capacity: the utility's output is discarded, the status still reports. */
    assert(launch("exit 0", NULL, 0) == 0);
    assert(launch("exit 1", NULL, 0) == DOTNET_PAL_OS_ERROR);
    /* Argument validation before any launch. */
    const char *empty_argument[] = {""};
    (void)empty_argument;
    assert(p->crash_dump(NULL, 1, (uint8_t*)error, sizeof error) == DOTNET_PAL_INVALID_ARGUMENT);
    const char *with_null[] = {"/bin/sh", NULL};
    assert(p->crash_dump((const uint8_t *const *)with_null, 2, (uint8_t*)error, sizeof error) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(p->crash_dump((const uint8_t *const *)with_null, 1, NULL, 8) == DOTNET_PAL_INVALID_ARGUMENT);
    /* exit terminates with the status; observe it from a child. */
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) { p->exit(7); _exit(99); }
    int status = 0;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 7);
    dotnet_pal_process_stats stats;
    assert(p->read_stats(&stats, sizeof stats) == 0 && stats.debugger_ok == 1 && stats.dump_ok == 2 && stats.rejected_or_failed >= 5);
    puts("PROCESS PASS debugger=absent dump=launched exit=observed");
    return 0;
}
