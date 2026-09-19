//! Per-thread ELF TLS blocks (aarch64 variant I).
//!
//! The compiler reaches a thread-local through `TPIDR_EL0`: for a local-exec
//! access the address is the thread pointer plus a 16-byte TCB plus the symbol's
//! offset in the PT_TLS image. This port builds that layout by hand for every
//! thread, including the boot thread:
//!
//! ```text
//!   tp -> [ 16-byte TCB ][ copy of .tdata ][ zeroed .tbss ]
//! ```
//!
//! The TCB is never read here; a static image has no dynamic thread vector. The
//! linker script asserts that no thread-local needs more than 16-byte alignment,
//! which is what lets the block start exactly 16 bytes above the thread pointer.
use crate::memory;
use core::{arch::asm, ptr};

extern "C" {
    static __tdata_start: u8;
    static __tdata_end: u8;
    static __tbss_end: u8;
}

const TCB: usize = 16;

/// `(image address, initialized bytes, total bytes)` of the PT_TLS image.
fn image() -> (usize, usize, usize) {
    let start = ptr::addr_of!(__tdata_start) as usize;
    let initialized = ptr::addr_of!(__tdata_end) as usize - start;
    let total = ptr::addr_of!(__tbss_end) as usize - start;
    (start, initialized, total)
}

/// A thread's TLS allocation: `base`/`size` are what to return to the region,
/// `pointer` is the value for `TPIDR_EL0`.
#[derive(Clone, Copy)]
pub struct Block {
    pub base: usize,
    pub size: usize,
    pub pointer: usize,
}

/// Builds a TLS block for one thread out of the region.
pub fn allocate() -> Option<Block> {
    let (start, initialized, total) = image();
    let size = memory::round_up(TCB + total, memory::PAGE);
    let base = memory::take(size, memory::PAGE)?;
    memory::zero(base, size);
    // SAFETY: the block is this thread's private storage and the image is the
    // linker's read-only PT_TLS data.
    unsafe { ptr::copy_nonoverlapping(start as *const u8, (base + TCB) as *mut u8, initialized) };
    Some(Block { base, size, pointer: base })
}
pub fn release(block: Block) {
    if block.size != 0 {
        memory::give(block.base, block.size);
    }
}
/// # Safety
/// `pointer` must stay a valid TLS block for as long as this thread runs.
pub unsafe fn install(pointer: usize) {
    unsafe { asm!("msr tpidr_el0, {}", in(reg) pointer, options(nomem, nostack)) };
}
pub fn current() -> usize {
    let value: usize;
    // SAFETY: TPIDR_EL0 is readable at EL1.
    unsafe { asm!("mrs {}, tpidr_el0", out(reg) value, options(nomem, nostack)) };
    value
}
