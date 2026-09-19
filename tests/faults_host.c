/* Independent POSIX reference provider for the host-faults conformance suite:
 * the CPU faults Linux delivers as signals, reported in the boundary's frame.
 * The handler runs on the faulting stack, never an alternate one: the consumer
 * resumes on that stack. A stack overflow therefore ends the process before it
 * can be reported, and DOTNET_PAL_FAULT_STACK_OVERFLOW is never produced.
 * Fault 1 withholds a callback; fault 2 describes a frame eight bytes too long. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <signal.h>
#include <stdatomic.h>
#include <ucontext.h>
int pal_faults_fault;
static const int signals[5] = {SIGSEGV, SIGBUS, SIGFPE, SIGILL, SIGTRAP};
static _Atomic uintptr_t deliver_slot;
#if defined(__aarch64__)
typedef dotnet_pal_fault_frame_arm64 frame_t;
#define FRAME_TAG DOTNET_PAL_FRAME_ARM64
static void capture(frame_t *frame, const ucontext_t *uc) {
    for (int i = 0; i < 31; ++i) frame->x[i] = uc->uc_mcontext.regs[i];
    frame->sp = uc->uc_mcontext.sp; frame->pc = uc->uc_mcontext.pc; frame->pstate = uc->uc_mcontext.pstate;
}
static void apply(ucontext_t *uc, const frame_t *frame) {
    for (int i = 0; i < 31; ++i) uc->uc_mcontext.regs[i] = frame->x[i];
    uc->uc_mcontext.sp = frame->sp; uc->uc_mcontext.pc = frame->pc; uc->uc_mcontext.pstate = frame->pstate;
}
#elif defined(__x86_64__)
typedef dotnet_pal_fault_frame_x64 frame_t;
#define FRAME_TAG DOTNET_PAL_FRAME_X64
/* The frame is in instruction-encoding order; gregs is not. */
static const int order[16] = {REG_RAX, REG_RCX, REG_RDX, REG_RBX, REG_RSP, REG_RBP, REG_RSI, REG_RDI,
    REG_R8, REG_R9, REG_R10, REG_R11, REG_R12, REG_R13, REG_R14, REG_R15};
static void capture(frame_t *frame, const ucontext_t *uc) {
    for (int i = 0; i < 16; ++i) frame->registers[i] = (uint64_t)uc->uc_mcontext.gregs[order[i]];
    frame->rip = (uint64_t)uc->uc_mcontext.gregs[REG_RIP]; frame->rflags = (uint64_t)uc->uc_mcontext.gregs[REG_EFL];
}
static void apply(ucontext_t *uc, const frame_t *frame) {
    for (int i = 0; i < 16; ++i) uc->uc_mcontext.gregs[order[i]] = (greg_t)frame->registers[i];
    uc->uc_mcontext.gregs[REG_RIP] = (greg_t)frame->rip; uc->uc_mcontext.gregs[REG_EFL] = (greg_t)frame->rflags;
}
#else
#error "the boundary defines a fault frame for aarch64 and x86-64 only"
#endif
static uint32_t classify(int code, const siginfo_t *info) {
    switch (code) {
    case SIGSEGV: return DOTNET_PAL_FAULT_ACCESS;
    case SIGBUS: return info->si_code == BUS_ADRALN ? DOTNET_PAL_FAULT_ALIGNMENT : DOTNET_PAL_FAULT_ACCESS;
    case SIGFPE: return info->si_code == FPE_INTDIV ? DOTNET_PAL_FAULT_INTEGER_DIVIDE
        : info->si_code == FPE_INTOVF ? DOTNET_PAL_FAULT_INTEGER_OVERFLOW : DOTNET_PAL_FAULT_FLOATING_POINT;
    case SIGILL: return DOTNET_PAL_FAULT_ILLEGAL_INSTRUCTION;
    default: return DOTNET_PAL_FAULT_BREAKPOINT;
    }
}
static void on_signal(int code, siginfo_t *info, void *context) {
    int saved = errno;
    dotnet_pal_fault_handler deliver = (dotnet_pal_fault_handler)atomic_load_explicit(&deliver_slot, memory_order_acquire);
    /* si_code <= 0 is a signal somebody sent, not a fault: it has no faulting instruction to resume from. */
    int sent = info->si_code <= 0;
    if (deliver && !sent) {
        frame_t frame;
        capture(&frame, context);
        uint32_t kind = classify(code, info);
        int memory = kind == DOTNET_PAL_FAULT_ACCESS || kind == DOTNET_PAL_FAULT_ALIGNMENT;
        if (deliver(kind, memory ? (uintptr_t)info->si_addr : 0, &frame, sizeof frame, NULL) == DOTNET_PAL_FAULT_RESUME) {
            /* The kernel resumes from the context it is handed back, so every register goes back. */
            apply(context, &frame);
            errno = saved;
            return;
        }
    }
    /* Unhandled: with the default action back, returning re-executes the faulting instruction and the process ends the normal way. */
    struct sigaction standard = {0};
    standard.sa_handler = SIG_DFL;
    sigemptyset(&standard.sa_mask);
    sigaction(code, &standard, NULL);
    if (sent) raise(code);
    errno = saved;
}
static uint64_t frame_tag(void) { return FRAME_TAG; }
static size_t frame_size(void) { return sizeof(frame_t); }
static size_t frame_size_long(void) { return sizeof(frame_t) + 8; }
static uint32_t enable(dotnet_pal_fault_handler deliver) {
    if (!deliver) return DOTNET_PAL_INVALID_ARGUMENT;
    atomic_store_explicit(&deliver_slot, (uintptr_t)deliver, memory_order_release);
    struct sigaction action = {0}, previous[5];
    action.sa_sigaction = on_signal;
    action.sa_flags = SA_SIGINFO; /* no SA_ONSTACK: the consumer resumes on the faulting stack */
    /* All five stay blocked in the handler, so the kernel ends the process on a fault taken inside it. */
    sigemptyset(&action.sa_mask);
    for (int i = 0; i < 5; ++i) sigaddset(&action.sa_mask, signals[i]);
    for (int i = 0; i < 5; ++i) if (sigaction(signals[i], &action, &previous[i]) != 0) {
        while (i-- > 0) sigaction(signals[i], &previous[i], NULL);
        return DOTNET_PAL_OS_ERROR;
    }
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_faults table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_faults), DOTNET_PAL_CAP_FAULTS},
    {frame_tag, frame_size, enable},
};
static const dotnet_pal_host_faults malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_faults), DOTNET_PAL_CAP_FAULTS}, {frame_tag, frame_size, NULL}};
static const dotnet_pal_host_faults mismatched = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_faults), DOTNET_PAL_CAP_FAULTS}, {frame_tag, frame_size_long, enable}};
const dotnet_pal_host_faults *dotnet_pal_host_faults_v2(void) {
    return pal_faults_fault == 1 ? &malformed : pal_faults_fault == 2 ? &mismatched : &table;
}
