#include "kernel_adapter.h"
#include <cassert>
#include <csignal>
#include <sys/wait.h>
#include <unistd.h>
static dotnet_pal_api table{};
extern "C" const dotnet_pal_api *dotnet_pal_get_api(uint32_t) { return &table; }
static void aborts() {
    pid_t child = fork(); assert(child >= 0);
    if (!child) { (void)dotnet_pal_kernel::require(); _exit(0); }
    int s; assert(waitpid(child, &s, 0) == child);
    assert(WIFSIGNALED(s) && WTERMSIG(s) == SIGABRT);
}
int main() {
    assert(!dotnet_pal_kernel::api()); aborts();
    table.header = {2, sizeof table, DOTNET_PAL_CAP_KERNEL};
    assert(!dotnet_pal_kernel::api()); // advertising a group is not enough
    table.kernel.event_create = [](uint32_t, uint32_t, void **) -> uint32_t { return 0; };
    auto op = [](void *) -> uint32_t { return 0; };
    table.kernel.event_destroy = op; table.kernel.event_set = op; table.kernel.event_reset = op;
    table.kernel.event_wait = [](void *, uint64_t ns) -> uint32_t {
        assert(ns == UINT64_MAX || ns == 0 || ns == UINT64_C(123000000)); return ns == 0 ? DOTNET_PAL_TIMEOUT : DOTNET_PAL_OK;
    };
    table.kernel.mutex_create = [](uint32_t, void **) -> uint32_t { return 0; };
    table.kernel.mutex_destroy = op; table.kernel.mutex_lock = op; table.kernel.mutex_unlock = op;
    table.kernel.thread_create = [](dotnet_pal_thread_entry, void *, size_t, void **) -> uint32_t { return 0; };
    table.kernel.thread_join = op; table.kernel.thread_detach = op;
    table.kernel.tls_create = [](dotnet_pal_tls_destructor, void **) -> uint32_t { return 0; };
    table.kernel.tls_destroy = op;
    table.kernel.tls_get = [](void *, void **) -> uint32_t { return 0; };
    table.kernel.tls_set = [](void *, void *) -> uint32_t { return 0; };
    table.kernel.stack_bounds = [](void **, void **) -> uint32_t { return 0; };
    table.kernel.process_barrier = []() -> uint32_t { return 0; };
    assert(dotnet_pal_kernel::api());
    assert(dotnet_pal_kernel::wait_ms(nullptr, 0) == 258);
    assert(dotnet_pal_kernel::wait_ms(nullptr, 123) == 0);
    assert(dotnet_pal_kernel::wait_ms(nullptr, UINT32_MAX) == 0);
    table.header.struct_size = DOTNET_PAL_KERNEL_API_SIZE - 1;
    assert(!dotnet_pal_kernel::api()); aborts();
    table.header.struct_size = sizeof table;
    table.header.capabilities &= ~DOTNET_PAL_CAP_PROCESS_BARRIER;
    assert(!dotnet_pal_kernel::api()); aborts();
}
