// Hardware faults reported by the port instead of delivered as signals.
//
// Included by the patched HardwareExceptions.cpp after its EXCEPTION_* codes and
// g_hardwareExceptionHandler. A port without the signal substrate may still own a
// trap path (an exception vector, an exception port); the faults group hands the
// runtime each fault with the interrupted registers in the boundary's frame
// layout. The translation below is the one HardwareExceptionHandler performs for a
// signal: build the limited context, ask the runtime's handler, and on
// EXCEPTION_CONTINUE_EXECUTION write back the control registers and the two
// argument registers so the port resumes in RhpThrowHwEx.
//
// The frame-to-context mapping exists for ARM64 only, the architecture a port has
// executed it on. Elsewhere the capability is left unused and a fault ends the run,
// which is what a port without the capability gets as well. Everything here is
// static: it reads the including file's own g_hardwareExceptionHandler.
#include "dotnet_pal.h"
#include <cstring>
namespace dotnet_pal_faults {
#if defined(HOST_ARM64)
using Frame = dotnet_pal_fault_frame_arm64;
constexpr uint64_t FrameTag = DOTNET_PAL_FRAME_ARM64;
static void to_context(const Frame *frame, PAL_LIMITED_CONTEXT *context) {
    memset(context, 0, sizeof *context);
    context->FP = frame->x[29]; context->LR = frame->x[30];
    context->X0 = frame->x[0]; context->X1 = frame->x[1];
    context->X19 = frame->x[19]; context->X20 = frame->x[20]; context->X21 = frame->x[21]; context->X22 = frame->x[22];
    context->X23 = frame->x[23]; context->X24 = frame->x[24]; context->X25 = frame->x[25]; context->X26 = frame->x[26];
    context->X27 = frame->x[27]; context->X28 = frame->x[28];
    context->SP = frame->sp; context->IP = frame->pc;
}
static void redirect(Frame *frame, const PAL_LIMITED_CONTEXT *context, uintptr_t arg0, uintptr_t arg1) {
    frame->pc = context->IP; frame->sp = context->SP; frame->x[29] = context->FP; frame->x[30] = context->LR;
    frame->x[0] = arg0; frame->x[1] = arg1;
}
#else
#define DOTNET_PAL_FAULTS_UNMAPPED
#endif
#ifndef DOTNET_PAL_FAULTS_UNMAPPED
static uintptr_t exception_code(uint32_t kind) {
    switch (kind) {
        case DOTNET_PAL_FAULT_ACCESS: return EXCEPTION_ACCESS_VIOLATION;
        case DOTNET_PAL_FAULT_ALIGNMENT: return EXCEPTION_DATATYPE_MISALIGNMENT;
        case DOTNET_PAL_FAULT_INTEGER_DIVIDE: return EXCEPTION_INT_DIVIDE_BY_ZERO;
        case DOTNET_PAL_FAULT_INTEGER_OVERFLOW: return EXCEPTION_INT_OVERFLOW;
        case DOTNET_PAL_FAULT_FLOATING_POINT: return EXCEPTION_FLT_INVALID_OPERATION;
        case DOTNET_PAL_FAULT_ILLEGAL_INSTRUCTION: return EXCEPTION_ILLEGAL_INSTRUCTION;
        case DOTNET_PAL_FAULT_BREAKPOINT: return EXCEPTION_BREAKPOINT;
        case DOTNET_PAL_FAULT_STACK_OVERFLOW: return EXCEPTION_STACK_OVERFLOW;
        default: return 0;
    }
}
// Runs on the port's trap path in the faulting thread: no allocation, no locks, no boundary calls.
static uint32_t handle(uint32_t kind, uintptr_t address, void *raw, size_t size, void *) {
    const uintptr_t code = exception_code(kind);
    if (!g_hardwareExceptionHandler || !raw || size != sizeof(Frame) || code == 0) return DOTNET_PAL_FAULT_UNHANDLED;
    Frame *frame = static_cast<Frame*>(raw);
    PAL_LIMITED_CONTEXT context;
    to_context(frame, &context);
    uintptr_t arg0 = 0, arg1 = 0;
    if (g_hardwareExceptionHandler(code, address, &context, &arg0, &arg1) != EXCEPTION_CONTINUE_EXECUTION) return DOTNET_PAL_FAULT_UNHANDLED;
    redirect(frame, &context, arg0, arg1);
    return DOTNET_PAL_FAULT_RESUME;
}
#endif
// True when there is nothing to install or the handler is installed; false fails runtime startup,
// because a port that advertises fault reporting the runtime cannot use is a broken configuration.
static bool initialize() {
    auto *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION) return false;
    if (a->header.struct_size < DOTNET_PAL_FAULTS_API_SIZE || !(a->header.capabilities & DOTNET_PAL_CAP_FAULTS)) return true;
#ifdef DOTNET_PAL_FAULTS_UNMAPPED
    return true;
#else
    const auto *f = &a->faults;
    if (!f->frame_tag || !f->frame_size || !f->install) return false;
    if (f->frame_tag() != FrameTag || f->frame_size() != sizeof(Frame)) return false;
    return f->install(handle, nullptr) == DOTNET_PAL_OK;
#endif
}
}
