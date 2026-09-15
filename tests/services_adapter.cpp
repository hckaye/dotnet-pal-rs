#include "gc_services_adapter.h"
#include <cassert>
#include <csignal>
#include <sys/wait.h>
#include <unistd.h>

static dotnet_pal_api table{};
static uint64_t timestamp = 1234567890;
static uint64_t slept;
static bool fail;
static uint32_t clock_ns(uint64_t *out) { *out = timestamp; return fail ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_OK; }
static uint32_t sleep_ns(uint64_t ns) { slept = ns; return DOTNET_PAL_OK; }
static uint32_t yield_thread() { return DOTNET_PAL_OK; }
extern "C" const dotnet_pal_api *dotnet_pal_get_api(uint32_t) { return &table; }
static void aborts(void (*operation)()) {
    pid_t pid = fork();
    assert(pid >= 0);
    if (pid == 0) { operation(); _exit(0); }
    int status = 0;
    assert(waitpid(pid, &status, 0) == pid);
    assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
}
int main() {
    using namespace dotnet_pal_gc_services;
    table.header = {2, sizeof table, DOTNET_PAL_CAP_CLOCK | DOTNET_PAL_CAP_SCHEDULER};
    table.services = {clock_ns, sleep_ns, yield_thread, nullptr};
    assert(api());
    assert(counter() == 1234567890 && frequency() == 1000000000 && lowres_ms() == 1234);
    sleep_ms(UINT32_MAX);
    assert(slept == UINT64_C(4294967295000000));
    table.header.struct_size = DOTNET_PAL_LINEAR_API_SIZE;
    assert(!api()); // the legacy ABI 2 prefix is too short for services
    aborts([] { (void)counter(); });
    table.header.struct_size = sizeof table;
    table.header.capabilities = DOTNET_PAL_CAP_CLOCK;
    assert(!api());
    table.header.capabilities |= DOTNET_PAL_CAP_SCHEDULER;
    fail = true;
    aborts([] { (void)counter(); });
    fail = false;
    timestamp = UINT64_MAX;
    aborts([] { (void)counter(); }); // signed .NET counter must not wrap negative
}
