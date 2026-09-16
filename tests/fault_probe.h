#ifndef DOTNET_PAL_TEST_FAULT_PROBE_H
#define DOTNET_PAL_TEST_FAULT_PROBE_H
#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

/* This one access MUST reach the CPU, not a sanitizer's shadow-memory handler.
 * All PAL operations, their callers and concurrent workloads stay instrumented.
 * A sanitizer's ordinary nonzero exit is never accepted as proof of protection.
 */
#if defined(__clang__)
__attribute__((disable_sanitizer_instrumentation, noinline))
#elif defined(__GNUC__)
__attribute__((no_sanitize_address, no_sanitize_thread, noinline))
#endif
static void pal_test_raw_access(void *address, int write_access) {
    if (write_access) *(volatile uint8_t *)address = 1;
    else { volatile uint8_t byte = *(volatile uint8_t *)address; (void)byte; }
}
static void pal_test_fault_impl(void *address, int write_access,
    uint32_t (*release)(void *, size_t), size_t size) {
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        /* Restore kernel-default dispositions in this disposable child only.
         * Inherited sanitizer handlers may convert SIGSEGV to exit(66).
         */
        struct sigaction action = {0};
        action.sa_handler = SIG_DFL;
        if (sigemptyset(&action.sa_mask) != 0 ||
            sigaction(SIGSEGV, &action, NULL) != 0 ||
            sigaction(SIGBUS, &action, NULL) != 0) _exit(125);
        if (release && release(address, size) != 0) _exit(124);
        pal_test_raw_access(address, write_access);
        _exit(0);
    }
    int state = 0;
    pid_t result;
    do { result = waitpid(child, &state, 0); } while (result < 0 && errno == EINTR);
    assert(result == child);
    if (!WIFSIGNALED(state) || (WTERMSIG(state) != SIGSEGV && WTERMSIG(state) != SIGBUS)) {
        fprintf(stderr, "protection probe failed (release=%d, write=%d): wait_status=%d exited=%d code=%d signaled=%d signal=%d\n",
            release != NULL, write_access, state, WIFEXITED(state), WIFEXITED(state) ? WEXITSTATUS(state) : -1,
            WIFSIGNALED(state), WIFSIGNALED(state) ? WTERMSIG(state) : -1);
        abort();
    }
}
static inline void pal_test_must_fault(void *address, int write_access) {
    pal_test_fault_impl(address, write_access, NULL, 0);
}
static inline void pal_test_release_must_fault(void *address, size_t size,
    uint32_t (*release)(void *, size_t)) {
    /* Fork while the region is STILL owned. The fork/sanitizer machinery is
     * allowed to reuse a released address; therefore release only after fork.
     * No allocation may occur between release and the actual CPU access.
     */
    pal_test_fault_impl(address, 0, release, size);
}
#endif
