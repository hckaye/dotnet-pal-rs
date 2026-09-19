/* Conformance test of the faults group with faults the CPU really takes. Built
 * only with -DPAL_HOST_TEST, against the reference host provider: Linux proper
 * reports faults as signals through the context group and has no Faults provider. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
#ifndef PAL_HOST_TEST
#error "the faults group has no Linux provider: build with -DPAL_HOST_TEST and tests/faults_host.c"
#endif
extern int pal_faults_fault;
#if defined(__aarch64__)
typedef dotnet_pal_fault_frame_arm64 frame_t;
#define FRAME_TAG DOTNET_PAL_FRAME_ARM64
#define ARCHITECTURE "arm64"
#elif defined(__x86_64__)
typedef dotnet_pal_fault_frame_x64 frame_t;
#define FRAME_TAG DOTNET_PAL_FRAME_X64
#define ARCHITECTURE "x64"
#else
#error "the boundary defines a fault frame for aarch64 and x86-64 only"
#endif
#if defined(__clang__)
#define OPAQUE __attribute__((noinline, optnone))
#else
#define OPAQUE __attribute__((noinline, noclone))
#endif
/* What recover must find in its argument registers after the first, and what the faulting store has
 * in its registers instead: a register the port failed to write back cannot be right by accident. */
#define MAGIC UINT64_C(0xfeedfacecafe0001)
#define STORED UINT64_C(0x0123456789abcdef)
/* A null reference plus a field offset. */
#define WILD 16u
enum { STORE, DIVIDE };
enum { RECOVER, DECLINE, NEST, SENT };
struct attempt {
    jmp_buf env;
    int armed, calls, recovered, aligned;
    uint32_t kind;
    uintptr_t address, pc, sp, return_address;
    size_t size;
    void *data;
    pthread_t thread;
};
/* Thread-local and static: the handler finds the record of the thread it runs on,
 * and nothing setjmp leaves indeterminate is read after the longjmp. */
static _Thread_local struct attempt attempt;
static unsigned cookie = 0x1234;
static volatile int answer = RECOVER;
static int report = -1;
static _Atomic uint64_t recovered;
/* Each faulting function is alone in a named section, so the linker's bounds are the function's. */
extern const char __start_pal_fault_store[], __stop_pal_fault_store[];
/* The address comes from a volatile object: a store the compiler can prove null
 * may be compiled to a trap instruction, which is a different fault. */
static volatile uintptr_t wild = WILD;
__attribute__((section("pal_fault_store"))) OPAQUE static void store(uint64_t value) { *(volatile uint64_t *)wild = value; }
#if defined(__x86_64__)
extern const char __start_pal_fault_divide[], __stop_pal_fault_divide[];
static volatile int32_t divisor, quotient;
__attribute__((section("pal_fault_divide"))) OPAQUE static int32_t divide(int32_t value) { return value / divisor; }
#endif
/* Entered by the edited frame, as if the faulting instruction had called it, and
 * checks that it was. It leaves by longjmp and never returns: the faulting
 * function made no call, so its caller-saved registers (and on arm64 its own
 * x30) are not what a return would need. */
OPAQUE static void recover(struct attempt *a, uint64_t second, uint64_t third, uint64_t fourth) {
    if (a != &attempt || second != MAGIC || third != MAGIC + 1 || fourth != MAGIC + 2) abort();
    a->return_address = (uintptr_t)__builtin_return_address(0);
    a->aligned = ((uintptr_t)__builtin_frame_address(0) & 15) == 0;
    a->recovered++;
    longjmp(a->env, 1);
}
static uint32_t on_fault(uint32_t kind, uintptr_t address, void *frame, size_t size, void *data) {
    struct attempt *a = &attempt;
    a->calls++; a->kind = kind; a->address = address; a->size = size; a->data = data; a->thread = pthread_self();
    if (report >= 0) {
        uint64_t record[2] = {kind, address};
        if (write(report, record, sizeof record) != (ssize_t)sizeof record) _exit(3);
    }
    if (answer == NEST) store(STORED);
    if (answer != RECOVER || !a->armed || size != sizeof(frame_t)) return DOTNET_PAL_FAULT_UNHANDLED;
    a->armed = 0;
    frame_t *f = frame;
#if defined(__aarch64__)
    a->pc = (uintptr_t)f->pc; a->sp = (uintptr_t)f->sp;
    f->x[0] = (uint64_t)(uintptr_t)a; f->x[1] = MAGIC; f->x[2] = MAGIC + 1; f->x[3] = MAGIC + 2;
    f->x[30] = f->pc; f->pc = (uint64_t)(uintptr_t)recover;
#else
    a->pc = (uintptr_t)f->rip; a->sp = (uintptr_t)f->registers[4];
    /* The call is emulated: the return address goes on the stack and recover is entered with rsp + 8
     * on a 16-byte boundary. The slot stays inside the 128-byte red zone under the interrupted rsp,
     * because the provider's handler runs on this stack and its signal frame begins below that zone. */
    uint64_t rsp = ((f->registers[4] - 8) & ~UINT64_C(15)) - 8;
    *(uint64_t *)(uintptr_t)rsp = f->rip;
    /* rdi, rsi, rdx, rcx: the last two are where encoding order differs from alphabetical order. */
    f->registers[7] = (uint64_t)(uintptr_t)a; f->registers[6] = MAGIC; f->registers[2] = MAGIC + 1; f->registers[1] = MAGIC + 2;
    f->registers[4] = rsp; f->rip = (uint64_t)(uintptr_t)recover;
#endif
    return DOTNET_PAL_FAULT_RESUME;
}
static int on_own_stack(uintptr_t sp) {
    pthread_attr_t attributes; void *low = NULL; size_t size = 0;
    assert(pthread_getattr_np(pthread_self(), &attributes) == 0 && pthread_attr_getstack(&attributes, &low, &size) == 0);
    pthread_attr_destroy(&attributes);
    return sp >= (uintptr_t)low && sp < (uintptr_t)low + size;
}
/* Takes one fault on the calling thread and checks what the handler saw and what recover found. */
static void provoke(int what) {
    volatile char marker = 0;
    memset(&attempt, 0, sizeof attempt);
    attempt.armed = 1;
    if (setjmp(attempt.env) == 0) {
        if (what == STORE) store(STORED);
#if defined(__x86_64__)
        else quotient = divide(7);
#endif
        abort(); /* the faulting function is never resumed */
    }
    const char *start = __start_pal_fault_store, *stop = __stop_pal_fault_store;
    uint32_t kind = DOTNET_PAL_FAULT_ACCESS;
#if defined(__x86_64__)
    if (what == DIVIDE) { start = __start_pal_fault_divide; stop = __stop_pal_fault_divide; kind = DOTNET_PAL_FAULT_INTEGER_DIVIDE; }
#endif
    assert(attempt.calls == 1 && attempt.recovered == 1 && pthread_equal(attempt.thread, pthread_self()));
    assert(attempt.kind == kind && attempt.size == sizeof(frame_t) && attempt.data == &cookie && cookie == 0x1234);
    assert(what != STORE || attempt.address == WILD);
    assert(attempt.pc >= (uintptr_t)start && attempt.pc < (uintptr_t)stop);
    assert(attempt.sp < (uintptr_t)&marker && (uintptr_t)&marker - attempt.sp < 4096 && on_own_stack(attempt.sp));
    assert(attempt.return_address == attempt.pc && attempt.aligned);
    /* The handler returned through the port: the fault's signal is deliverable again. */
    sigset_t mask;
    assert(pthread_sigmask(SIG_SETMASK, NULL, &mask) == 0 && !sigismember(&mask, SIGSEGV) && !sigismember(&mask, SIGFPE));
    atomic_fetch_add(&recovered, 1);
}
static void *on_thread(void *unused) { (void)unused; provoke(STORE); return &cookie; }
/* A process the port ends cannot say why, so the child's handler reports each call through a pipe first. */
static void dies(int how, int by, size_t calls) {
    int fds[2];
    assert(pipe(fds) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        struct rlimit none = {0, 0};
        setrlimit(RLIMIT_CORE, &none);
        alarm(10);
        close(fds[0]);
        answer = how; report = fds[1]; attempt.armed = 1;
        if (how == SENT) kill(getpid(), by); else store(STORED);
        _exit(0);
    }
    close(fds[1]);
    uint64_t records[4][2]; size_t got = 0; ssize_t n;
    while (got < sizeof records && (n = read(fds[0], (char *)records + got, sizeof records - got)) > 0) got += (size_t)n;
    close(fds[0]);
    int status = 0;
    assert(waitpid(child, &status, 0) == child && WIFSIGNALED(status) && WTERMSIG(status) == by);
    assert(got == calls * sizeof records[0]);
    assert(calls == 0 || (records[0][0] == DOTNET_PAL_FAULT_ACCESS && records[0][1] == WILD));
}
int main(int argc, char **argv) {
    pal_faults_fault = argc > 1 ? atoi(argv[1]) : 0;
    alarm(30); /* a port that resumes at the wrong place can spin instead of failing */
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
    if (pal_faults_fault == 1) { assert(!api); puts("FAULTS PASS malformed host table rejected"); return 0; }
    if (pal_faults_fault == 2) { assert(!api); puts("FAULTS PASS host frame size mismatch rejected"); return 0; }
    assert(api && api->header.struct_size >= DOTNET_PAL_FAULTS_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_FAULTS);
    const dotnet_pal_faults_ops *f = &api->faults;
    assert(f->frame_tag() == FRAME_TAG && f->frame_size() == sizeof(frame_t));
    assert(f->install(NULL, &cookie) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(f->install(on_fault, &cookie) == DOTNET_PAL_OK);
    assert(f->install(on_fault, &cookie) == DOTNET_PAL_BUSY);
    dotnet_pal_faults_stats stats;
    assert(f->read_stats(&stats, sizeof stats) == 0 && stats.installs == 1 && stats.delivered == 0 && stats.rejected == 2);
    /* Twice on this thread: a resumed fault leaves the port ready for the next one. */
    provoke(STORE);
    provoke(STORE);
    /* The handler runs on the thread that faulted: another thread's record is filled, this one's is not touched. */
    pthread_t thread; void *other = NULL;
    assert(pthread_create(&thread, NULL, on_thread, NULL) == 0 && pthread_join(thread, &other) == 0);
    assert(other == &cookie && attempt.calls == 1);
#if defined(__x86_64__)
    /* arm64 does not trap an integer division by zero. */
    provoke(DIVIDE);
#endif
    /* UNHANDLED ends the process by the fault's own signal; so does a fault inside the handler, which
     * is not delivered a second time; a signal somebody sent is not a fault and reaches no handler.
     * That one is SIGBUS: Rosetta, translating x86-64 on an arm64 Linux kernel, was seen to present a
     * sent SIGSEGV as a fault (si_code 1) and to drop it under the default action; the kernel does neither. */
    dies(DECLINE, SIGSEGV, 1);
    dies(NEST, SIGSEGV, 1);
    dies(SENT, SIGBUS, 0);
    /* The children counted their own faults; this process resumed every one it took. */
    uint64_t expected = atomic_load(&recovered);
    assert(f->read_stats(NULL, sizeof stats) == DOTNET_PAL_INVALID_ARGUMENT && f->read_stats(&stats, sizeof stats - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(f->read_stats(&stats, sizeof stats) == 0 && stats.installs == 1 && stats.delivered == expected && stats.resumed == expected
        && stats.unhandled == 0 && stats.rejected == 2);
    printf("FAULTS PASS %s frame=%zu delivered=%llu resumed=%llu unhandled=%llu rejected=%llu children=3\n", ARCHITECTURE, sizeof(frame_t),
        (unsigned long long)stats.delivered, (unsigned long long)stats.resumed, (unsigned long long)stats.unhandled, (unsigned long long)stats.rejected);
    return 0;
}
