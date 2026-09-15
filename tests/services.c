#define _POSIX_C_SOURCE 200809L
#include "dotnet_pal.h"
#include <assert.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/time.h>

int pal_services_fault = 0; /* configured once before obtaining any table */
static const dotnet_pal_api *api;
static volatile sig_atomic_t interrupted;
static void alarm_handler(int sig) { (void)sig; interrupted = 1; }
static uint64_t now(void) {
    uint64_t ns = 0;
    assert(api->services.monotonic_ns(&ns) == DOTNET_PAL_OK);
    return ns;
}
static void *worker(void *unused) {
    (void)unused;
    uint64_t last = now();
    for (int i = 0; i < 400; ++i) {
        uint64_t next = now();
        assert(next >= last);
        last = next;
        assert(api->services.yield_thread() == DOTNET_PAL_OK);
    }
    return NULL;
}
int main(int argc, char **argv) {
    pal_services_fault = argc == 2 ? atoi(argv[1]) : 0;
    api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (pal_services_fault >= 2 && pal_services_fault <= 6) {
        assert(!api);
        puts("SERVICES rejected malformed host table");
        return 0;
    }
    assert(api && api->header.struct_size >= DOTNET_PAL_SERVICES_API_SIZE);
    if (!(api->header.capabilities & DOTNET_PAL_CAP_CLOCK)) {
        assert(!api->services.monotonic_ns && !api->services.sleep_ns && !api->services.yield_thread);
        puts("SERVICES unavailable: no fabricated capabilities");
        return 0;
    }
    assert(api->header.capabilities & DOTNET_PAL_CAP_SCHEDULER);
    assert(api->services.read_stats && api->services.monotonic_ns && api->services.sleep_ns && api->services.yield_thread);
    assert(api->services.monotonic_ns(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    _Alignas(uint64_t) unsigned char unaligned[sizeof(uint64_t) + 1];
    assert(api->services.monotonic_ns((uint64_t *)(unaligned + 1)) == DOTNET_PAL_INVALID_ARGUMENT);
    if (pal_services_fault == 1 || pal_services_fault == 7) {
        uint64_t out = UINT64_MAX;
        assert(api->services.monotonic_ns(&out) == DOTNET_PAL_OS_ERROR);
        assert(out == 0); /* failed foreign callback may have scribbled on its output */
        assert(api->services.sleep_ns(1) == DOTNET_PAL_OS_ERROR);
        assert(api->services.yield_thread() == DOTNET_PAL_OS_ERROR);
        puts("SERVICES failure sanitized");
        return 0;
    }
    uint64_t first = now();
    assert(first <= now());
    struct sigaction action = {0};
    action.sa_handler = alarm_handler;
    sigemptyset(&action.sa_mask);
    assert(sigaction(SIGALRM, &action, NULL) == 0);
    struct itimerval timer = {0};
    timer.it_value.tv_usec = 1000;
    uint64_t before = now();
    assert(setitimer(ITIMER_REAL, &timer, NULL) == 0);
    assert(api->services.sleep_ns(UINT64_C(20000000)) == DOTNET_PAL_OK);
    assert(now() - before >= UINT64_C(20000000));
    assert(interrupted); /* nanosleep must retry EINTR, not report early success */
    assert(api->services.sleep_ns(0) == DOTNET_PAL_OK);
    pthread_t threads[4];
    for (int i = 0; i < 4; ++i) assert(pthread_create(&threads[i], NULL, worker, NULL) == 0);
    for (int i = 0; i < 4; ++i) assert(pthread_join(threads[i], NULL) == 0);
    dotnet_pal_services_stats stats = {0};
    assert(api->services.read_stats(&stats, sizeof stats - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(api->services.read_stats(&stats, sizeof stats) == DOTNET_PAL_OK);
    assert(stats.clock_ok >= 1600 && stats.sleep_ok == 2 && stats.yield_ok == 1600);
    assert(stats.rejected_or_failed == 2);
    puts("SERVICES PASS monotonic, interrupted sleep, concurrent yield, ABI errors");
    return 0;
}
