/* Independent POSIX reference provider for the host-process conformance suite.
 * Fault 1 withholds the table; fault 2 makes every callback fail. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
int pal_process_fault;
static void process_exit(int32_t code) { exit(code); }
static uint32_t debugger_present(uint32_t *out) {
    if (pal_process_fault == 2) { *out = 5; return DOTNET_PAL_OS_ERROR; }
    FILE *status = fopen("/proc/self/status", "r");
    if (!status) return DOTNET_PAL_OS_ERROR;
    char line[256]; uint32_t result = DOTNET_PAL_OS_ERROR;
    while (fgets(line, sizeof line, status)) {
        if (strncmp(line, "TracerPid:", 10) == 0) { *out = strtoul(line + 10, NULL, 10) != 0; result = DOTNET_PAL_OK; break; }
    }
    fclose(status);
    return result;
}
static uint32_t crash_dump(const uint8_t *const *argv, size_t argc, uint8_t *error, size_t capacity) {
    if (pal_process_fault == 2) { if (capacity) memcpy(error, "no", capacity < 3 ? capacity : 3); return DOTNET_PAL_OS_ERROR; }
    /* A plain fork/exec/wait without the ptrace permission the Linux backend grants. */
    const char *arguments[65];
    if (argc > 64) return DOTNET_PAL_INVALID_ARGUMENT;
    for (size_t i = 0; i < argc; ++i) arguments[i] = (const char*)argv[i];
    arguments[argc] = NULL;
    int pipes[2];
    if (pipe(pipes) != 0) return DOTNET_PAL_OS_ERROR;
    pid_t child = fork();
    if (child < 0) { close(pipes[0]); close(pipes[1]); return DOTNET_PAL_OS_ERROR; }
    if (child == 0) {
        close(pipes[0]);
        if (capacity) dup2(pipes[1], STDERR_FILENO);
        execv(arguments[0], (char *const *)arguments);
        _exit(255);
    }
    close(pipes[1]);
    size_t filled = 0;
    while (capacity > 1 && filled < capacity - 1) {
        ssize_t n = read(pipes[0], error + filled, capacity - 1 - filled);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) break;
        filled += (size_t)n;
    }
    if (capacity) error[filled] = 0;
    close(pipes[0]);
    int status = 0;
    if (waitpid(child, &status, 0) != child) return DOTNET_PAL_OS_ERROR;
    return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static const dotnet_pal_host_process table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_process), DOTNET_PAL_CAP_PROCESS},
    {process_exit, debugger_present, crash_dump, NULL},
};
static const dotnet_pal_host_process malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_process), DOTNET_PAL_CAP_PROCESS}, {NULL, debugger_present, crash_dump, NULL}};
const dotnet_pal_host_process *dotnet_pal_host_process_v2(void) { return pal_process_fault == 1 ? &malformed : &table; }
