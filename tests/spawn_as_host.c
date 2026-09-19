/* Independent POSIX reference provider for the host-spawn-as conformance suite.
 * The child is one of tests/processes_host.c, which this file is linked with:
 * a plain fork whose child calls setgroups, setgid and setuid before it enters
 * its directory and executes the program, so that wait, terminate, release and
 * the pipes of that provider work on it. Fault 1 withholds the callback; fault 2
 * breaks the output contracts so the front end's sanitizing is observable. */
#include "dotnet_pal.h"
int pal_spawn_as_fault;
uint32_t pal_processes_host_start(const uint8_t *program, size_t program_length, const uint8_t *const *arguments, size_t argument_count,
    const uint8_t *const *environment, size_t environment_count, const uint8_t *directory, size_t directory_length, uint32_t pipes,
    const dotnet_pal_identity *identity, dotnet_pal_spawned *out);
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
    case 4: return DOTNET_PAL_WOULD_BLOCK;              /* a status spawn_as does not have */
    default: return DOTNET_PAL_ACCESS_DENIED;           /* a refusal that left a whole child in the output */
    }
}
static uint32_t spawn_as(const uint8_t *program, size_t program_length, const uint8_t *const *arguments, size_t argument_count,
    const uint8_t *const *environment, size_t environment_count, const uint8_t *directory, size_t directory_length, uint32_t pipes,
    const dotnet_pal_identity *identity, dotnet_pal_spawned *out, size_t size) {
    (void)size;
    if (pal_spawn_as_fault == 2) return breach(pipes, out);
    return pal_processes_host_start(program, program_length, arguments, argument_count, environment, environment_count, directory, directory_length, pipes, identity, out);
}
static const dotnet_pal_host_spawn_as table = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_spawn_as), DOTNET_PAL_CAP_SPAWN_AS}, {spawn_as, NULL}};
static const dotnet_pal_host_spawn_as malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_spawn_as), DOTNET_PAL_CAP_SPAWN_AS}, {NULL, NULL}};
const dotnet_pal_host_spawn_as *dotnet_pal_host_spawn_as_v2(void) { return pal_spawn_as_fault == 1 ? &malformed : &table; }
