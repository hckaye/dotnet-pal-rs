/* Test-only POSIX host implementation, not an implicit fallback in the library. */
#define _POSIX_C_SOURCE 200809L
#include "dotnet_pal.h"
#include <errno.h>
#include <sched.h>
#include <time.h>
#ifdef PAL_SERVICES_FAULT_HOST
extern int pal_services_fault;
#else
#define pal_services_fault 0
#endif
static uint32_t clock_ns(uint64_t *out) {
    if (pal_services_fault == 1 || pal_services_fault == 7) {
        *out = UINT64_MAX;
        return pal_services_fault == 7 ? 999u : DOTNET_PAL_OS_ERROR;
    }
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) return DOTNET_PAL_OS_ERROR;
    *out = (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
    return DOTNET_PAL_OK;
}
static uint32_t sleep_ns(uint64_t ns) {
    if (pal_services_fault == 1 || pal_services_fault == 7) return DOTNET_PAL_OS_ERROR;
    struct timespec request = {(time_t)(ns / UINT64_C(1000000000)), (long)(ns % UINT64_C(1000000000))};
    while (nanosleep(&request, &request) != 0) if (errno != EINTR) return DOTNET_PAL_OS_ERROR;
    return DOTNET_PAL_OK;
}
static uint32_t yield_thread(void) {
    if (pal_services_fault == 1 || pal_services_fault == 7) return DOTNET_PAL_OS_ERROR;
    return sched_yield() == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
#define HEADER {2, sizeof(dotnet_pal_host_services), DOTNET_PAL_CAP_CLOCK | DOTNET_PAL_CAP_SCHEDULER}
static const dotnet_pal_host_services valid = {HEADER, clock_ns, sleep_ns, yield_thread};
#ifdef PAL_SERVICES_FAULT_HOST
static const dotnet_pal_host_services wrong_version = {{99, sizeof valid, 12}, clock_ns, sleep_ns, yield_thread};
static const dotnet_pal_host_services short_table = {{2, sizeof(dotnet_pal_header), 12}, clock_ns, sleep_ns, yield_thread};
static const dotnet_pal_host_services no_cap = {{2, sizeof valid, DOTNET_PAL_CAP_CLOCK}, clock_ns, sleep_ns, yield_thread};
static const dotnet_pal_host_services no_function = {HEADER, clock_ns, NULL, yield_thread};
#endif
const dotnet_pal_host_services *dotnet_pal_host_services_v2(void) {
#ifdef PAL_SERVICES_FAULT_HOST
    switch (pal_services_fault) {
        case 2: return NULL;
        case 3: return &wrong_version;
        case 4: return &short_table;
        case 5: return &no_cap;
        case 6: return &no_function;
    }
#endif
    return &valid;
}
