// Cooperative context switch. Only the callee-saved state of the AAPCS64
// procedure call standard has to survive, because a switch always happens at a
// call boundary inside the port: x19-x28, the frame pointer, the link register,
// d8-d15 and the thread pointer. Everything else belongs to the caller of
// pal_context_switch and is already dead there.
//
// Frame written on the outgoing stack (176 bytes, 16-byte aligned):
//   0  x19 x20 | 16 x21 x22 | 32 x23 x24 | 48 x25 x26 | 64 x27 x28 | 80 x29 x30
//   96 tpidr_el0 (+8 padding)
//   112 d8 d9 | 128 d10 d11 | 144 d12 d13 | 160 d14 d15
// src/thread.rs builds the same frame by hand to start a new thread.

.section .text,"ax",%progbits
.globl pal_context_switch
.type pal_context_switch, %function
// pal_context_switch(x0: *mut usize saved_sp_of_current, x1: usize sp_of_next)
pal_context_switch:
    sub     sp, sp, #176
    stp     x19, x20, [sp, #0]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    mrs     x2, tpidr_el0
    str     x2, [sp, #96]
    stp     d8,  d9,  [sp, #112]
    stp     d10, d11, [sp, #128]
    stp     d12, d13, [sp, #144]
    stp     d14, d15, [sp, #160]
    mov     x2, sp
    str     x2, [x0]                        // publish the outgoing stack pointer
    mov     sp, x1                          // and continue on the incoming one
    ldp     d8,  d9,  [sp, #112]
    ldp     d10, d11, [sp, #128]
    ldp     d12, d13, [sp, #144]
    ldp     d14, d15, [sp, #160]
    ldr     x2, [sp, #96]
    msr     tpidr_el0, x2
    ldp     x19, x20, [sp, #0]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    add     sp, sp, #176
    ret
.size pal_context_switch, . - pal_context_switch

// First entry of a new thread. The prepared frame leaves the thread index in
// x19 and this address in x30, so the switch "returns" here.
.globl pal_thread_trampoline
.type pal_thread_trampoline, %function
pal_thread_trampoline:
    mov     x29, xzr                        // end the frame chain at the thread root
    mov     x30, xzr
    mov     x0, x19
    bl      pal_thread_main                 // runs the entry, then switches away for good
    brk     #0
.size pal_thread_trampoline, . - pal_thread_trampoline
