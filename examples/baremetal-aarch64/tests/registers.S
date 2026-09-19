// Helper for the register half of the context-switch contract.
//
// A C compiler is free to spill a callee-saved register around a call, so a
// check written in C proves nothing about what the switch itself preserved.
// This does it by hand: stamp a distinct value into every register the AAPCS64
// standard says a callee keeps (x19-x28, d8-d15), call the yield function, and
// compare. Each thread passes its own base value, so a switch that dropped a
// register would come back holding another thread's marker.
//
// uint64_t pal_test_registers(uint64_t base, uint32_t (*yield)(void));
// Returns zero when every register came back, nonzero otherwise.
.section .text,"ax",%progbits

.macro STAMP_X reg, offset
    add     \reg, x3, #\offset
.endm
.macro STAMP_D reg, offset
    add     x2, x3, #\offset
    fmov    \reg, x2
.endm
.macro CHECK_X reg, offset
    add     x1, x3, #\offset
    sub     x2, \reg, x1
    orr     x0, x0, x2
.endm
.macro CHECK_D reg, offset
    add     x1, x3, #\offset
    fmov    x2, \reg
    sub     x2, x2, x1
    orr     x0, x0, x2
.endm

.globl pal_test_registers
.type pal_test_registers, %function
pal_test_registers:
    stp     x29, x30, [sp, #-176]!
    mov     x29, sp
    stp     x19, x20, [sp, #16]             // the caller's own registers come back
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]
    stp     d8,  d9,  [sp, #96]
    stp     d10, d11, [sp, #112]
    stp     d12, d13, [sp, #128]
    stp     d14, d15, [sp, #144]
    str     x1, [sp, #160]                  // the yield function
    str     x0, [sp, #168]                  // the base value
    mov     x3, x0

    STAMP_D d8,  10
    STAMP_D d9,  11
    STAMP_D d10, 12
    STAMP_D d11, 13
    STAMP_D d12, 14
    STAMP_D d13, 15
    STAMP_D d14, 16
    STAMP_D d15, 17
    STAMP_X x19, 0
    STAMP_X x20, 1
    STAMP_X x21, 2
    STAMP_X x22, 3
    STAMP_X x23, 4
    STAMP_X x24, 5
    STAMP_X x25, 6
    STAMP_X x26, 7
    STAMP_X x27, 8
    STAMP_X x28, 9

    ldr     x2, [sp, #160]
    blr     x2

    ldr     x3, [sp, #168]
    mov     x0, xzr                         // any difference below makes this nonzero
    CHECK_X x19, 0
    CHECK_X x20, 1
    CHECK_X x21, 2
    CHECK_X x22, 3
    CHECK_X x23, 4
    CHECK_X x24, 5
    CHECK_X x25, 6
    CHECK_X x26, 7
    CHECK_X x27, 8
    CHECK_X x28, 9
    CHECK_D d8,  10
    CHECK_D d9,  11
    CHECK_D d10, 12
    CHECK_D d11, 13
    CHECK_D d12, 14
    CHECK_D d13, 15
    CHECK_D d14, 16
    CHECK_D d15, 17

    ldp     x19, x20, [sp, #16]
    ldp     x21, x22, [sp, #32]
    ldp     x23, x24, [sp, #48]
    ldp     x25, x26, [sp, #64]
    ldp     x27, x28, [sp, #80]
    ldp     d8,  d9,  [sp, #96]
    ldp     d10, d11, [sp, #112]
    ldp     d12, d13, [sp, #128]
    ldp     d14, d15, [sp, #144]
    ldp     x29, x30, [sp], #176
    ret
.size pal_test_registers, . - pal_test_registers

// Probes for the fault path. Each one faults on a known instruction, so the
// test's handler can recognize it by address and step over it.
//
// uint64_t pal_fault_load(const uint64_t *address);
// Loads through x0 into x0. The handler supplies the value the load could not.
.globl pal_fault_load
.type pal_fault_load, %function
pal_fault_load:
    ldr     x0, [x0]
    ret
.size pal_fault_load, . - pal_fault_load

// uint64_t pal_fault_keeps(uint64_t *address);
// Stamps registers a handler has no reason to preserve for us (x9-x15, d0-d7,
// d16-d31 are caller-saved), faults on a store, and reports whether every one of
// them came back. Zero means they all did.
.globl pal_fault_keeps
.type pal_fault_keeps, %function
pal_fault_keeps:
    mov     x9, #0x1109
    mov     x10, #0x110a
    mov     x15, #0x110f
    mov     x2, #0x2200
    fmov    d0, x2
    add     x2, x2, #7
    fmov    d7, x2
    add     x2, x2, #24
    fmov    d31, x2
.globl pal_fault_keeps_store
pal_fault_keeps_store:
    str     x9, [x0]
    mov     x0, #0
    mov     x1, #0x1109
    sub     x1, x9, x1
    orr     x0, x0, x1
    mov     x1, #0x110a
    sub     x1, x10, x1
    orr     x0, x0, x1
    mov     x1, #0x110f
    sub     x1, x15, x1
    orr     x0, x0, x1
    mov     x2, #0x2200
    fmov    x1, d0
    sub     x1, x1, x2
    orr     x0, x0, x1
    add     x2, x2, #7
    fmov    x1, d7
    sub     x1, x1, x2
    orr     x0, x0, x1
    add     x2, x2, #24
    fmov    x1, d31
    sub     x1, x1, x2
    orr     x0, x0, x1
    ret
.size pal_fault_keeps, . - pal_fault_keeps
