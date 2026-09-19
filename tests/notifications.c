/* Conformance test of the notifications group on Linux with signals the kernel
 * really delivers: reports come from a thread of the port's own in ordinary
 * thread context, a kind that is not enabled keeps the action it had, and the
 * fatal and stopping default actions are observed from outside, in children. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_notifications_fault;
#endif
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
enum { INTERRUPT = DOTNET_PAL_NOTIFY_INTERRUPT, QUIT = DOTNET_PAL_NOTIFY_QUIT, TERMINATE = DOTNET_PAL_NOTIFY_TERMINATE, HANGUP = DOTNET_PAL_NOTIFY_HANGUP,
    CONTINUE = DOTNET_PAL_NOTIFY_CONTINUE, WINDOW_CHANGE = DOTNET_PAL_NOTIFY_WINDOW_CHANGE, STOP = DOTNET_PAL_NOTIFY_STOP };
/* What the handler does with a kind it is told about. */
enum { NOTHING, DISABLE, DEFAULT_ACTION };
enum { CHILD_DISABLED, CHILD_DEFAULT, CHILD_DEFAULT_IN_HANDLER, CHILD_LATE_REPORT, CHILD_STOP, CHILDREN };
static const int nine[9] = {SIGINT, SIGQUIT, SIGTERM, SIGHUP, SIGCONT, SIGWINCH, SIGTTIN, SIGTTOU, SIGTSTP};
static const dotnet_pal_api *api;
static const dotnet_pal_notifications_ops *n;
static unsigned cookie = 0x1234;
static pthread_t main_thread;
static pthread_mutex_t gate;
/* calls counts finished handler runs, so whatever the handler did is visible once it has moved. */
static _Atomic unsigned calls[10], invalid, old_hits;
static _Atomic int reaction[10];
static _Atomic uint32_t reaction_status[10];
static void old_handler(int code) { (void)code; atomic_fetch_add(&old_hits, 1); }
static void handler(uint32_t kind, void *data) {
    int bad = kind < 1 || kind > 9 || data != &cookie || cookie != 0x1234 || pthread_equal(pthread_self(), main_thread);
    /* Everything a signal context forbids: a mutex the interrupted thread may hold (it reports
     * EDEADLK to a handler run on top of its owner), the heap, and the boundary itself. */
    bad |= pthread_mutex_lock(&gate) != 0;
    volatile char *block = malloc(4096);
    if (block) block[0] = 1; else bad = 1;
    free((void*)block);
    void *mutex = NULL; uint64_t now = 0;
    bad |= api->kernel.mutex_create(0, &mutex) != 0 || api->kernel.mutex_lock(mutex) != 0 || api->kernel.mutex_unlock(mutex) != 0 || api->kernel.mutex_destroy(mutex) != 0;
    bad |= api->services.monotonic_ns(&now) != 0 || now == 0;
    /* Both reference providers keep the nine signals away from the reporting thread, also after a default action raised one on it. */
    sigset_t mask;
    bad |= pthread_sigmask(SIG_SETMASK, NULL, &mask) != 0;
    for (int i = 0; i < 9; ++i) bad |= sigismember(&mask, nine[i]) != 1;
    bad |= pthread_mutex_unlock(&gate) != 0;
    if (bad) { atomic_fetch_add(&invalid, 1); if (kind < 1 || kind > 9) return; }
    int what = atomic_load(&reaction[kind]);
    if (what == DISABLE) atomic_store(&reaction_status[kind], n->disable(kind));
    if (what == DEFAULT_ACTION) atomic_store(&reaction_status[kind], n->default_action(kind));
    atomic_fetch_add(&calls[kind], 1);
}
/* Bounded by iterations, not by time: a stopped process does not use its budget up. */
static int reached(_Atomic unsigned *counter, unsigned value) {
    for (int i = 0; i < 5000; ++i) {
        if (atomic_load(counter) >= value) return 1;
        nanosleep(&(struct timespec){0, 1000000}, NULL);
    }
    return 0;
}
static void send(int number) { assert(kill(getpid(), number) == 0); }
/* Reports are made in order, so after this one everything sent before it has been reported. */
static void drain(void) {
    unsigned before = atomic_load(&calls[WINDOW_CHANGE]);
    send(SIGWINCH);
    assert(reached(&calls[WINDOW_CHANGE], before + 1));
}
static sighandler_t current(int number) {
    struct sigaction action;
    assert(sigaction(number, NULL, &action) == 0);
    return action.sa_handler;
}
static void setup(void) {
    assert(n->install(handler, &cookie) == 0);
    assert(n->enable(INTERRUPT) == 0 && n->enable(TERMINATE) == 0 && n->enable(WINDOW_CHANGE) == 0);
}
static _Noreturn void child(int scenario) {
    alarm(20);
    setup();
    switch (scenario) {
    case CHILD_DISABLED: /* disable gives the signal its default action back */
        assert(n->disable(INTERRUPT) == 0 && current(SIGINT) == SIG_DFL);
        send(SIGINT);
        break;
    case CHILD_DEFAULT: /* on a thread that has the signal unblocked */
        send(SIGTERM);
        assert(reached(&calls[TERMINATE], 1));
        n->default_action(TERMINATE);
        break;
    case CHILD_DEFAULT_IN_HANDLER: /* on the reporting thread, which has it blocked */
        atomic_store(&reaction[TERMINATE], DEFAULT_ACTION);
        send(SIGTERM);
        break;
    case CHILD_LATE_REPORT:
        /* Two interrupts are caught before the handler gets to disable the kind on the first report: the
         * second report finds nobody who wants it, and the port owes the signal its default action. */
        atomic_store(&reaction[INTERRUPT], DISABLE);
        assert(pthread_mutex_lock(&gate) == 0);
        send(SIGINT); send(SIGINT);
        assert(pthread_mutex_unlock(&gate) == 0);
        break;
    case CHILD_STOP:
        /* A group of its own with the parent outside it: the kernel discards a stop aimed at an orphaned group. */
        assert(setpgid(0, 0) == 0 && n->enable(STOP) == 0);
        atomic_store(&reaction[STOP], DEFAULT_ACTION);
        send(SIGTSTP); /* caught, so nothing stops until the handler asks for it */
        assert(reached(&calls[STOP], 1) && atomic_load(&reaction_status[STOP]) == 0);
        /* Continued by the parent: the kind is still enabled and is reported again. */
        atomic_store(&reaction[STOP], NOTHING);
        send(SIGTSTP);
        assert(reached(&calls[STOP], 2) && atomic_load(&invalid) == 0);
        _exit(0);
    }
    for (;;) pause();
}
static int outcome(int scenario) {
    pid_t pid = fork();
    assert(pid >= 0);
    if (pid == 0) child(scenario);
    int status = 0;
    assert(waitpid(pid, &status, WUNTRACED) == pid);
    if (WIFSTOPPED(status)) {
        assert(scenario == CHILD_STOP && WSTOPSIG(status) == SIGTSTP);
        assert(kill(pid, SIGCONT) == 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status));
        status |= 0x10000;
    }
    return status;
}
int main(int argc, char **argv) {
    alarm(50);
    /* However this was started (a background job ignores SIGINT and SIGQUIT), the nine start from their defaults. */
    for (int i = 0; i < 9; ++i) signal(nine[i], SIG_DFL);
#ifdef PAL_HOST_TEST
    pal_notifications_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_notifications_fault == 1) { assert(!api); puts("NOTIFICATIONS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_NOTIFICATIONS_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_NOTIFICATIONS);
    n = &api->notifications;
    main_thread = pthread_self();
    pthread_mutexattr_t attributes;
    assert(pthread_mutexattr_init(&attributes) == 0 && pthread_mutexattr_settype(&attributes, PTHREAD_MUTEX_ERRORCHECK) == 0);
    assert(pthread_mutex_init(&gate, &attributes) == 0);
    dotnet_pal_notifications_stats stats;
#ifdef PAL_HOST_TEST
    if (pal_notifications_fault == 2) {
        /* A host that reports kind 0, kind 10 and a kind nobody enabled: all dropped, the handler never runs. */
        assert(n->install(handler, &cookie) == 0);
        for (int i = 0; i < 5000; ++i) {
            assert(n->read_stats(&stats, sizeof stats) == 0);
            if (stats.dropped >= 3) break;
            nanosleep(&(struct timespec){0, 1000000}, NULL);
        }
        assert(stats.dropped == 3 && stats.delivered == 0);
        for (int kind = 0; kind < 10; ++kind) assert(atomic_load(&calls[kind]) == 0);
        /* A kind somebody wants still gets through. */
        assert(n->enable(INTERRUPT) == 0);
        send(SIGINT);
        assert(reached(&calls[INTERRUPT], 1) && atomic_load(&invalid) == 0);
        assert(n->read_stats(&stats, sizeof stats) == 0 && stats.dropped == 3 && stats.delivered == 1);
        puts("NOTIFICATIONS unwanted host reports dropped"); return 0;
    }
#endif
    /* The children first: a process forked later would have the pipe but no reporting thread. */
    int status[CHILDREN];
    for (int scenario = 0; scenario < CHILDREN; ++scenario) status[scenario] = outcome(scenario);
    assert(WIFSIGNALED(status[CHILD_DISABLED]) && WTERMSIG(status[CHILD_DISABLED]) == SIGINT);
    assert(WIFSIGNALED(status[CHILD_DEFAULT]) && WTERMSIG(status[CHILD_DEFAULT]) == SIGTERM);
    assert(WIFSIGNALED(status[CHILD_DEFAULT_IN_HANDLER]) && WTERMSIG(status[CHILD_DEFAULT_IN_HANDLER]) == SIGTERM);
    assert(WIFSIGNALED(status[CHILD_LATE_REPORT]) && WTERMSIG(status[CHILD_LATE_REPORT]) == SIGINT);
    assert(status[CHILD_STOP] == 0x10000); /* stopped by SIGTSTP, continued, exited 0 */

    /* One handler, once; nothing can be enabled before it. */
    assert(n->install(NULL, &cookie) == INVALID);
    assert(n->enable(INTERRUPT) == INVALID && n->disable(INTERRUPT) == INVALID);
    /* Actions other than the default in place before enable: a handler for SIGHUP, ignore for SIGQUIT. */
    struct sigaction before = {0};
    before.sa_handler = old_handler;
    sigemptyset(&before.sa_mask);
    assert(sigaction(SIGHUP, &before, NULL) == 0 && signal(SIGQUIT, SIG_IGN) != SIG_ERR);
    setup();
    assert(n->install(handler, &cookie) == DOTNET_PAL_BUSY);
    assert(n->enable(0) == INVALID && n->enable(10) == INVALID && n->disable(0) == INVALID);
    assert(n->default_action(0) == INVALID && n->default_action(10) == INVALID);
    assert(n->enable(INTERRUPT) == 0); /* again: the action before the first enable stays the remembered one */
    struct sigaction observed;
    assert(sigaction(SIGINT, NULL, &observed) == 0 && observed.sa_handler != SIG_DFL && observed.sa_handler != SIG_IGN && (observed.sa_flags & SA_RESTART));

    /* The signal is caught on this thread while it holds the gate. The report waits for the gate on
     * another thread; a handler called from the signal context would have failed on it instead. */
    assert(pthread_mutex_lock(&gate) == 0);
    errno = E2BIG;
    send(SIGINT);
    assert(errno == E2BIG);
    nanosleep(&(struct timespec){0, 20000000}, NULL);
    assert(atomic_load(&calls[INTERRUPT]) == 0);
    assert(pthread_mutex_unlock(&gate) == 0);
    assert(reached(&calls[INTERRUPT], 1));
    send(SIGTERM);
    assert(reached(&calls[TERMINATE], 1));
    drain();
    assert(atomic_load(&calls[WINDOW_CHANGE]) == 1 && atomic_load(&invalid) == 0);

    /* Pending signals of one number merge: at least one report, never more than were sent. */
    for (int i = 0; i < 100; ++i) send(SIGINT);
    drain();
    unsigned burst = atomic_load(&calls[INTERRUPT]) - 1;
    assert(burst >= 1 && burst <= 100);
    /* The reporting thread is held up while more signals arrive than the pipe holds: the thread that
     * catches them never waits (it holds the gate, so waiting would be a deadlock), the excess is dropped. */
    int probe[2];
    assert(pipe(probe) == 0);
    long flood = fcntl(probe[1], F_GETPIPE_SZ) + 1000;
    close(probe[0]); close(probe[1]);
    unsigned floor = atomic_load(&calls[INTERRUPT]), seen = floor;
    assert(flood > 1000 && pthread_mutex_lock(&gate) == 0);
    for (long i = 0; i < flood; ++i) send(SIGINT);
    assert(pthread_mutex_unlock(&gate) == 0);
    /* Until the pipe has been emptied: a full one would drop the signal drain waits for. */
    for (int quiet = 0; quiet < 50; ++quiet) {
        nanosleep(&(struct timespec){0, 1000000}, NULL);
        if (atomic_load(&calls[INTERRUPT]) != seen) { seen = atomic_load(&calls[INTERRUPT]); quiet = 0; }
    }
    drain();
    unsigned flooded = atomic_load(&calls[INTERRUPT]) - floor;
    assert(flooded >= 1 && flooded < flood);

    /* Nothing to do for the kinds the target ignores by default. */
    assert(n->default_action(WINDOW_CHANGE) == 0 && n->default_action(CONTINUE) == 0);
    /* disable from inside the handler. */
    atomic_store(&reaction[TERMINATE], DISABLE);
    atomic_store(&reaction_status[TERMINATE], 99);
    send(SIGTERM);
    assert(reached(&calls[TERMINATE], 2) && atomic_load(&reaction_status[TERMINATE]) == 0 && current(SIGTERM) == SIG_DFL);

    /* The default action of a kind is what the action before enable does: the old SIGHUP handler runs, and the kind stays enabled. */
    assert(n->enable(HANGUP) == 0);
    send(SIGHUP);
    assert(reached(&calls[HANGUP], 1) && atomic_load(&old_hits) == 0);
    assert(n->default_action(HANGUP) == 0 && atomic_load(&old_hits) == 1);
    send(SIGHUP);
    assert(reached(&calls[HANGUP], 2) && atomic_load(&old_hits) == 1);
    assert(n->disable(HANGUP) == 0 && current(SIGHUP) == old_handler);
    send(SIGHUP);
    drain();
    assert(atomic_load(&old_hits) == 2 && atomic_load(&calls[HANGUP]) == 2);
    /* An ignored SIGQUIT (a background job, nohup for SIGHUP) is reported once enabled, and its default action stays "ignore". */
    assert(n->enable(QUIT) == 0);
    send(SIGQUIT);
    assert(reached(&calls[QUIT], 1));
    assert(n->default_action(QUIT) == 0 && n->disable(QUIT) == 0 && current(SIGQUIT) == SIG_IGN);

    /* A forked child shares the pipe: a signal it catches is not a request to this process. */
    unsigned interrupts = atomic_load(&calls[INTERRUPT]);
    pid_t pid = fork();
    assert(pid >= 0);
    if (pid == 0) { kill(getpid(), SIGINT); _exit(0); }
    int forked = 0;
    assert(waitpid(pid, &forked, 0) == pid && WIFEXITED(forked) && WEXITSTATUS(forked) == 0);
    drain();
    assert(atomic_load(&calls[INTERRUPT]) == interrupts);

    assert(n->disable(INTERRUPT) == 0 && current(SIGINT) == SIG_DFL);
    assert(n->disable(WINDOW_CHANGE) == 0 && n->disable(WINDOW_CHANGE) == 0 && current(SIGWINCH) == SIG_DFL);
    unsigned total = 0;
    for (int kind = 1; kind < 10; ++kind) total += atomic_load(&calls[kind]);
    assert(n->read_stats(NULL, sizeof stats) == INVALID && n->read_stats(&stats, sizeof stats - 1) == INVALID);
    assert(n->read_stats(&stats, sizeof stats) == 0);
    assert(stats.installs == 1 && stats.enabled == 6 && stats.disabled == 6 && stats.delivered == total && stats.dropped == 0 && stats.rejected == 9);
    assert(atomic_load(&invalid) == 0);
    printf("NOTIFICATIONS PASS reports=%u burst=%u/100 flood=%u/%ld from a thread outside signal context, children=%d ended or stopped by the default action\n",
        total, burst, flooded, flood, CHILDREN);
    return 0;
}
