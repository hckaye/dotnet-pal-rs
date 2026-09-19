/* Independent POSIX reference provider for the host-notifications conformance
 * suite: the nine signals behind the kinds, a self-pipe the signal handler
 * writes the kind into, and a dispatcher thread that reports from ordinary
 * thread context. The default action of a kind is what the action in place
 * before enable does with the signal.
 * Fault 1 withholds a callback; fault 2 reports kinds nobody can want. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <unistd.h>
int pal_notifications_fault;
static const int signals[10] = {0, SIGINT, SIGQUIT, SIGTERM, SIGHUP, SIGCONT, SIGWINCH, SIGTTIN, SIGTTOU, SIGTSTP};
/* Serializes every change of a signal action; the signal handler never takes it. */
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static struct sigaction previous[10];
static unsigned installed; /* bit per kind, under lock */
static _Atomic int write_end = -1;
static int read_end;
static _Atomic pid_t owner;
static uint32_t (*report)(uint32_t kind);
static void on_signal(int code) {
    int saved = errno;
    /* A child made by fork shares the pipe but has no dispatcher. */
    if (getpid() == atomic_load(&owner)) for (uint8_t kind = 1; kind < 10; ++kind) if (signals[kind] == code) {
        /* Non-blocking: a full pipe drops the report. */
        while (write(atomic_load(&write_end), &kind, 1) < 0 && errno == EINTR) {}
        break;
    }
    errno = saved;
}
static void handle(int number) {
    struct sigaction action = {0};
    action.sa_handler = on_signal;
    action.sa_flags = SA_RESTART;
    sigemptyset(&action.sa_mask);
    sigaction(number, &action, NULL);
}
static uint32_t enable(uint32_t kind) {
    if (kind < 1 || kind > 9 || atomic_load(&write_end) < 0) return DOTNET_PAL_INVALID_ARGUMENT;
    uint32_t status = DOTNET_PAL_OK;
    pthread_mutex_lock(&lock);
    if (!(installed >> kind & 1)) {
        /* Remembered once: enabling an enabled kind must not record on_signal as the previous action. */
        if (sigaction(signals[kind], NULL, &previous[kind]) != 0) status = DOTNET_PAL_OS_ERROR;
        else { handle(signals[kind]); installed |= 1u << kind; }
    }
    pthread_mutex_unlock(&lock);
    return status;
}
static uint32_t disable(uint32_t kind) {
    if (kind < 1 || kind > 9) return DOTNET_PAL_INVALID_ARGUMENT;
    uint32_t status = DOTNET_PAL_OK;
    pthread_mutex_lock(&lock);
    if (installed >> kind & 1) {
        if (sigaction(signals[kind], &previous[kind], NULL) != 0) status = DOTNET_PAL_OS_ERROR;
        else installed &= ~(1u << kind);
    }
    pthread_mutex_unlock(&lock);
    return status;
}
static uint32_t default_action(uint32_t kind) {
    if (kind < 1 || kind > 9) return DOTNET_PAL_INVALID_ARGUMENT;
    if (kind == DOTNET_PAL_NOTIFY_CONTINUE || kind == DOTNET_PAL_NOTIFY_WINDOW_CHANGE) return DOTNET_PAL_OK;
    pthread_mutex_lock(&lock);
    int ours = installed >> kind & 1, raised = 0;
    if (!ours || previous[kind].sa_handler != SIG_IGN) {
        if (ours) sigaction(signals[kind], &previous[kind], NULL);
        /* The dispatcher blocks the signal: unblocked on the calling thread only, for the raise only. */
        sigset_t set, old;
        sigemptyset(&set); sigaddset(&set, signals[kind]);
        pthread_sigmask(SIG_UNBLOCK, &set, &old);
        raised = raise(signals[kind]);
        pthread_sigmask(SIG_SETMASK, &old, NULL);
        /* Still here: the previous action handled it, or a stop has been continued. */
        if (ours) handle(signals[kind]);
    }
    pthread_mutex_unlock(&lock);
    return raised == 0 ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static void *dispatcher(void *unused) {
    (void)unused;
    sigset_t set;
    sigemptyset(&set);
    for (int kind = 1; kind < 10; ++kind) sigaddset(&set, signals[kind]);
    pthread_sigmask(SIG_BLOCK, &set, NULL);
    if (pal_notifications_fault == 2) { (void)report(0); (void)report(10); (void)report(DOTNET_PAL_NOTIFY_HANGUP); }
    uint8_t kinds[64];
    for (;;) {
        ssize_t count = read(read_end, kinds, sizeof kinds);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) return NULL;
        for (ssize_t i = 0; i < count; ++i) {
            pthread_mutex_lock(&lock);
            int wanted = installed >> kinds[i] & 1;
            pthread_mutex_unlock(&lock);
            /* A signal caught before disable and read after it gets the action it would have got had it arrived
             * after the disable; the callback's answer covers a disable that came in after the check above. */
            if (!wanted || !report(kinds[i])) default_action(kinds[i]);
        }
    }
}
static uint32_t start(uint32_t (*deliver)(uint32_t kind)) {
    if (!deliver) return DOTNET_PAL_INVALID_ARGUMENT;
    int ends[2];
    pthread_t thread;
    if (pipe2(ends, O_CLOEXEC) != 0) return DOTNET_PAL_OS_ERROR;
    /* Only the write end is non-blocking: the signal handler never waits, the dispatcher always does. */
    int flags = fcntl(ends[1], F_GETFL);
    report = deliver; read_end = ends[0];
    atomic_store(&owner, getpid());
    atomic_store(&write_end, ends[1]);
    if (flags < 0 || fcntl(ends[1], F_SETFL, flags | O_NONBLOCK) != 0 || pthread_create(&thread, NULL, dispatcher, NULL) != 0) {
        atomic_store(&write_end, -1);
        close(ends[0]); close(ends[1]);
        return DOTNET_PAL_OS_ERROR;
    }
    pthread_detach(thread);
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_notifications table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_notifications), DOTNET_PAL_CAP_NOTIFICATIONS},
    {start, enable, disable, default_action},
};
static const dotnet_pal_host_notifications malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_notifications), DOTNET_PAL_CAP_NOTIFICATIONS}, {start, enable, disable, NULL}};
const dotnet_pal_host_notifications *dotnet_pal_host_notifications_v2(void) { return pal_notifications_fault == 1 ? &malformed : &table; }
