//! Leaving the machine: semihosting exit and the port's abort.
//!
//! `qemu-system-aarch64 -semihosting` turns SYS_EXIT_EXTENDED with
//! ADP_Stopped_ApplicationExit into its own process exit status, so a test script
//! reads the result from QEMU's exit code. The `EXIT code=` line is printed first
//! because a harness that only keeps the serial log still sees the exact value.
use crate::uart;
use core::arch::asm;

const SYS_EXIT_EXTENDED: u64 = 0x20;
const ADP_STOPPED_APPLICATION_EXIT: u64 = 0x20026;

pub fn exit(code: i32) -> ! {
    uart::write(b"EXIT code=");
    uart::write_dec(code as u32 as u64);
    uart::write(b"\n");
    let block = [ADP_STOPPED_APPLICATION_EXIT, code as u32 as u64];
    // SAFETY: the semihosting call does not return when the host implements it.
    unsafe {
        asm!("hlt #0xf000", in("x0") SYS_EXIT_EXTENDED, in("x1") block.as_ptr(), options(nostack));
    }
    // Reached only when semihosting is disabled, in which case the HLT trapped
    // into the exception vectors instead. Stop the core rather than run on.
    halt()
}
pub fn halt() -> ! {
    loop {
        // SAFETY: WFE with interrupts masked parks this core until reset.
        unsafe { asm!("wfe", options(nomem, nostack)) };
    }
}
/// The port's `Abort`: an internal invariant failed and the run is over.
pub struct Abort;
impl dotnet_pal_rs::port::Abort for Abort {
    fn abort() -> ! {
        uart::write(b"ABORT\n");
        exit(134)
    }
}
