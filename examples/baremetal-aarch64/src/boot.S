// Boot code for QEMU -M virt. QEMU's -kernel loader enters here with the
// MMU off, caches off and interrupts masked, at EL2 when the machine has it
// and at EL1 otherwise. x0 holds the device tree address; this port does not
// read it, because every address it needs is fixed by the machine model.
//
// The sequence is: drop to EL1 if needed, set the boot stack, zero .bss, allow
// FP/SIMD, build the identity map, enable the MMU and caches, install the
// exception vectors, then hand over to Rust.

.section .text.boot,"ax",%progbits
.globl _start
.type _start, %function
_start:
    mrs     x1, CurrentEL
    cmp     x1, #(2 << 2)
    b.ne    .Lat_el1

    // EL2: give EL1 an AArch64 execution state and the counters, then eret.
    mrs     x1, hcr_el2
    orr     x1, x1, #(1 << 31)              // RW: EL1 is AArch64
    msr     hcr_el2, x1
    mrs     x1, cnthctl_el2
    orr     x1, x1, #3                      // EL1 may read the physical counter/timer
    msr     cnthctl_el2, x1
    msr     cntvoff_el2, xzr                // the virtual counter equals the physical one
    movz    x1, #0x0800
    movk    x1, #0x30d0, lsl #16            // SCTLR_EL1 reset value: MMU/caches off, RES1 bits set
    msr     sctlr_el1, x1
    mov     x1, #0x3c5                      // EL1h, DAIF masked
    msr     spsr_el2, x1
    adr     x1, .Lat_el1
    msr     elr_el2, x1
    eret

.Lat_el1:
    movz    x1, #0x0800
    movk    x1, #0x30d0, lsl #16
    msr     sctlr_el1, x1
    isb
    adrp    x1, __boot_stack_top
    add     x1, x1, :lo12:__boot_stack_top
    mov     sp, x1

    // Zero .bss. Rust statics and the page table live there.
    adrp    x1, __bss_start
    add     x1, x1, :lo12:__bss_start
    adrp    x2, __bss_end
    add     x2, x2, :lo12:__bss_end
.Lzero:
    cmp     x1, x2
    b.hs    .Lzeroed
    str     xzr, [x1], #8
    b       .Lzero
.Lzeroed:

    // FP/SIMD at EL1. Rust and C code use the d registers, and the context
    // switch saves d8-d15, so the traps must be off before either runs.
    mov     x1, #(3 << 20)                  // CPACR_EL1.FPEN = full access
    msr     cpacr_el1, x1
    isb

    bl      pal_mmu_enable

    adrp    x1, pal_vectors
    add     x1, x1, :lo12:pal_vectors
    msr     vbar_el1, x1
    isb

    bl      pal_rust_start                  // does not return
.Lhalt:
    wfe
    b       .Lhalt
.size _start, . - _start

// Identity map. The level-1 table covers 1 GiB per entry:
//   [0x0000_0000, 0x4000_0000)  a level-2 table of 2 MiB blocks:
//       [0x0000_0000, 0x0020_0000)  invalid: a null dereference faults
//       [0x0020_0000, 0x4000_0000)  Device-nGnRnE, execute never  (PL011 UART, GIC)
//   [0x4000_0000, 0x8000_0000)  a level-2 table and 512 level-3 tables:
//       Normal write-back cacheable 4 KiB pages (RAM, -m 1024).
// Everything else stays invalid, so a stray access faults instead of reading
// an unmapped device.
.globl pal_mmu_enable
.type pal_mmu_enable, %function
pal_mmu_enable:
    adrp    x3, pal_level2_table
    add     x3, x3, :lo12:pal_level2_table
    movz    x1, #0x0401                     // block, AF=1, AttrIndx=0, AP=RW at EL1
    movk    x1, #0x0060, lsl #48            // PXN | UXN
    mov     x2, #1                          // entry 0 stays zero: the null page never maps
.Ldevice:
    add     x4, x1, x2, lsl #21             // output address = index * 2 MiB
    str     x4, [x3, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, #512
    b.lo    .Ldevice

    adrp    x0, pal_level1_table
    add     x0, x0, :lo12:pal_level1_table
    orr     x1, x3, #3                      // table descriptor
    str     x1, [x0]
    // RAM uses page descriptors, not one 1 GiB block. The memory provider can
    // revoke or protect one 4 KiB page without changing its neighbours.
    adrp    x3, pal_ram_level2_table
    add     x3, x3, :lo12:pal_ram_level2_table
    orr     x1, x3, #3
    str     x1, [x0, #8]
    adrp    x4, pal_ram_pages
    add     x4, x4, :lo12:pal_ram_pages
    mov     x2, #0
.Lram_tables:
    add     x1, x4, x2, lsl #12
    orr     x1, x1, #3
    str     x1, [x3, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, #512
    b.lo    .Lram_tables

    mov     x2, #0
    mov     x5, #0x40000000
    movz    x6, #0x0707                     // page, AF=1, inner-shareable, AttrIndx=1
    orr     x6, x6, #(1 << 54)              // UXN; EL1 may execute the boot image
    mov     x7, #0x40000                    // 262144 pages cover 1 GiB
.Lram_pages:
    add     x1, x5, x2, lsl #12
    orr     x1, x1, x6
    str     x1, [x4, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, x7
    b.lo    .Lram_pages

    mov     x1, #0xff00                     // MAIR: attr0 Device-nGnRnE, attr1 Normal WB RW-alloc
    msr     mair_el1, x1
    movz    x1, #0x3519                     // T0SZ=25 (39-bit VA), 4 KiB granule, WB walks, inner shareable
    movk    x1, #0x0080, lsl #16            // EPD1: no TTBR1 translation
    movk    x1, #0x0002, lsl #32            // IPS = 40-bit physical addresses
    msr     tcr_el1, x1
    msr     ttbr0_el1, x0
    dsb     ish
    tlbi    vmalle1
    dsb     ish
    isb
    mrs     x1, sctlr_el1
    orr     x1, x1, #(1 << 0)               // M: MMU on
    orr     x1, x1, #(1 << 2)               // C: data cache on
    orr     x1, x1, #(1 << 12)              // I: instruction cache on
    msr     sctlr_el1, x1
    isb
    ret
.size pal_mmu_enable, . - pal_mmu_enable

// A synchronous exception taken at EL1 on the thread's own stack goes to
// pal_fault_entry, which can resume the interrupted code from an edited frame.
// Every other exception ends the run: the handler reports the syndrome on the
// UART and halts.
.macro PAL_VECTOR index
    .balign 0x80
    mov     x0, #\index
    b       pal_exception_entry
.endm

.section .text.vectors,"ax",%progbits
.balign 0x800
.globl pal_vectors
pal_vectors:
    PAL_VECTOR 0                            // current EL, SP_EL0: synchronous
    PAL_VECTOR 1                            // ... IRQ
    PAL_VECTOR 2                            // ... FIQ
    PAL_VECTOR 3                            // ... SError
    .balign 0x80                            // current EL, SP_ELx: synchronous
    b       pal_fault_entry
    PAL_VECTOR 5
    PAL_VECTOR 6
    PAL_VECTOR 7
    PAL_VECTOR 8                            // lower EL, AArch64
    PAL_VECTOR 9
    PAL_VECTOR 10
    PAL_VECTOR 11
    PAL_VECTOR 12                           // lower EL, AArch32
    PAL_VECTOR 13
    PAL_VECTOR 14
    PAL_VECTOR 15

.section .text.boot,"ax",%progbits
.type pal_exception_entry, %function
pal_exception_entry:
    mrs     x1, esr_el1
    mrs     x2, elr_el1
    mrs     x3, far_el1
    mrs     x4, spsr_el1
    bl      pal_exception                   // does not return
    brk     #0
.size pal_exception_entry, . - pal_exception_entry

// The interrupted registers in the boundary's AArch64 fault frame (x0..x30, sp,
// pc, pstate: 34 words), built on the faulting thread's stack. pal_fault returns
// only to resume: the frame, possibly edited, becomes the register state again.
// The FP/SIMD state is kept below the frame across the call, because a handler
// that resumes at the faulting instruction's neighbor must not have changed it.
// TPIDR_EL1 is this port's scratch register for the last step, where every
// general register already holds its final value.
.type pal_fault_entry, %function
pal_fault_entry:
    sub     sp, sp, #272
    stp     x0, x1, [sp, #0]
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    add     x0, sp, #272                    // the interrupted stack pointer
    stp     x30, x0, [sp, #240]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x0, x1, [sp, #256]
    sub     sp, sp, #528                    // q0..q31, fpcr, fpsr
    stp     q0, q1, [sp, #0]
    stp     q2, q3, [sp, #32]
    stp     q4, q5, [sp, #64]
    stp     q6, q7, [sp, #96]
    stp     q8, q9, [sp, #128]
    stp     q10, q11, [sp, #160]
    stp     q12, q13, [sp, #192]
    stp     q14, q15, [sp, #224]
    stp     q16, q17, [sp, #256]
    stp     q18, q19, [sp, #288]
    stp     q20, q21, [sp, #320]
    stp     q22, q23, [sp, #352]
    stp     q24, q25, [sp, #384]
    stp     q26, q27, [sp, #416]
    stp     q28, q29, [sp, #448]
    stp     q30, q31, [sp, #480]
    mrs     x0, fpcr
    mrs     x1, fpsr
    add     x2, sp, #512                    // past the reach of a pair offset
    stp     x0, x1, [x2]
    add     x0, sp, #528
    bl      pal_fault
    add     x2, sp, #512
    ldp     x0, x1, [x2]
    msr     fpcr, x0
    msr     fpsr, x1
    ldp     q0, q1, [sp, #0]
    ldp     q2, q3, [sp, #32]
    ldp     q4, q5, [sp, #64]
    ldp     q6, q7, [sp, #96]
    ldp     q8, q9, [sp, #128]
    ldp     q10, q11, [sp, #160]
    ldp     q12, q13, [sp, #192]
    ldp     q14, q15, [sp, #224]
    ldp     q16, q17, [sp, #256]
    ldp     q18, q19, [sp, #288]
    ldp     q20, q21, [sp, #320]
    ldp     q22, q23, [sp, #352]
    ldp     q24, q25, [sp, #384]
    ldp     q26, q27, [sp, #416]
    ldp     q28, q29, [sp, #448]
    ldp     q30, q31, [sp, #480]
    add     sp, sp, #528
    ldp     x0, x1, [sp, #256]
    msr     elr_el1, x0
    msr     spsr_el1, x1
    ldp     x2, x3, [sp, #16]
    ldp     x4, x5, [sp, #32]
    ldp     x6, x7, [sp, #48]
    ldp     x8, x9, [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30, [sp, #240]
    ldr     x0, [sp, #0]
    msr     tpidr_el1, x0
    ldr     x1, [sp, #8]
    ldr     x0, [sp, #248]                  // the stack pointer to resume on; the frame is dead after this
    mov     sp, x0
    mrs     x0, tpidr_el1
    eret
.size pal_fault_entry, . - pal_fault_entry

.section .bss.pagetable,"aw",%nobits
.balign 4096
pal_level1_table:
    .space 4096
pal_level2_table:
    .space 4096
pal_ram_level2_table:
    .space 4096
.globl pal_ram_pages
pal_ram_pages:
    .space 512 * 4096                      // 512 level-3 tables, one per 2 MiB

// The linker script needs a .tdata and a .tbss to exist to compute the TLS
// layout, and the boot code needs them even when no thread-local variable is
// linked in. Both are zero and unused.
.section .tdata,"awT",%progbits
.balign 16
pal_tls_guard_data:
    .quad 0
.section .tbss,"awT",%nobits
.balign 16
pal_tls_guard_bss:
    .space 8
