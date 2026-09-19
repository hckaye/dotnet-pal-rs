//! Exceptions: what the vectors in src/boot.S land on.
//!
//! A synchronous exception taken at EL1 arrives in [`pal_fault`] with the
//! interrupted registers in the boundary's AArch64 frame. Faults the boundary has
//! a kind for go to `dotnet_pal_rs::faults::deliver`; when the installed handler
//! answers `Resume`, returning from `pal_fault` makes the (edited) frame the
//! register state again. Everything else ends the run with the syndrome on the
//! serial port and status 132, as it does when no handler is installed.
use crate::{exit, uart, Baremetal};
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};
use dotnet_pal_rs::faults::{self, Disposition, Frame};
use dotnet_pal_rs::port;

impl port::Faults for Baremetal {
    /// The vector is installed by the boot code and reports from then on; a fault
    /// before a handler is installed is simply unhandled.
    fn enable() -> port::Result<()> { Ok(()) }
}

/// One core, synchronous exceptions only, and a handler that may not yield: a
/// second fault while this is set came from the fault path itself.
static HANDLING: AtomicBool = AtomicBool::new(false);

fn syndrome() -> (u64, u64) {
    let (esr, far): (u64, u64);
    // SAFETY: reading the syndrome registers has no side effects at EL1.
    unsafe { asm!("mrs {0}, esr_el1", "mrs {1}, far_el1", out(reg) esr, out(reg) far, options(nomem, nostack)) };
    (esr, far)
}
/// The boundary's fault kind for a syndrome, and whether FAR names the address.
fn classify(esr: u64) -> Option<(u32, bool)> {
    let status = esr & 0x3f;
    match esr >> 26 {
        // Instruction and data aborts from the current level.
        0b100001 | 0b100101 => Some(if status == 0b100001 { (faults::ALIGNMENT, true) } else { (faults::ACCESS, esr & (1 << 10) == 0) }),
        0b100010 | 0b100110 => Some((faults::ALIGNMENT, false)),
        0b101100 => Some((faults::FLOATING_POINT, false)),
        0b111100 => Some((faults::BREAKPOINT, false)),
        0b000000 => Some((faults::ILLEGAL_INSTRUCTION, false)),
        _ => None,
    }
}
/// Entered from `pal_fault_entry` with the frame on the faulting thread's stack.
/// Returns only to resume from it.
#[no_mangle]
pub extern "C" fn pal_fault(frame: &mut Frame) {
    let (esr, far) = syndrome();
    if let (Some((kind, located)), false) = (classify(esr), HANDLING.swap(true, Ordering::Relaxed)) {
        let disposition = faults::deliver(kind, if located { far as usize } else { 0 }, frame);
        HANDLING.store(false, Ordering::Relaxed);
        if disposition == Disposition::Resume {
            crate::trace(b"fault resumed at", frame.pc);
            return;
        }
    }
    report(4, esr, frame.pc, far, frame.pstate)
}
/// Every other vector: nothing on this machine resumes those.
#[no_mangle]
pub extern "C" fn pal_exception(entry: u64, esr: u64, elr: u64, far: u64, spsr: u64) -> ! {
    report(entry, esr, elr, far, spsr)
}
fn report(entry: u64, esr: u64, elr: u64, far: u64, spsr: u64) -> ! {
    uart::write(b"EXCEPTION entry=");
    uart::write_dec(entry);
    uart::write(b" class=");
    uart::write_dec(esr >> 26);
    uart::write(b" (");
    uart::write(class_of(esr));
    uart::write(b") esr=");
    uart::write_hex(esr);
    uart::write(b" elr=");
    uart::write_hex(elr);
    uart::write(b" far=");
    uart::write_hex(far);
    uart::write(b" spsr=");
    uart::write_hex(spsr);
    uart::write(b"\n");
    #[cfg(feature = "trace")]
    {
        // The thread pointer and the thread-local block around the runtime's current-thread slot.
        let tp = crate::tls::current();
        uart::write(b"[pal] tpidr_el0=");
        uart::write_hex(tp as u64);
        uart::write(b" thread=");
        uart::write_dec(crate::thread::current() as u64);
        uart::write(b"\n[pal] tls+0x50..0x80:");
        for offset in (0x50..0x80).step_by(8) {
            uart::write(b" ");
            // SAFETY: the block is this thread's mapped TLS storage.
            uart::write_hex(unsafe { ((tp + offset) as *const u64).read() });
        }
        uart::write(b"\n");
    }
    exit::exit(132)
}
fn class_of(esr: u64) -> &'static [u8] {
    match esr >> 26 {
        0b000000 => b"unknown",
        0b000001 => b"WFI or WFE",
        0b000111 => b"FP/SIMD trapped",
        0b001110 => b"illegal execution state",
        0b010101 => b"SVC",
        0b011000 => b"trapped system register",
        0b100000 | 0b100001 => b"instruction abort",
        0b100010 => b"PC alignment",
        0b100100 | 0b100101 => b"data abort",
        0b100110 => b"SP alignment",
        0b101100 => b"floating point",
        0b101111 => b"SError",
        0b111100 => b"BRK",
        _ => b"other",
    }
}
