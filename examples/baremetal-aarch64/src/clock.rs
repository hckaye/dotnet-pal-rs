//! Time from the architected generic timer, and the scheduling services built
//! on it.
//!
//! `CNTVCT_EL0` counts at the rate `CNTFRQ_EL0` reports (62.5 MHz on QEMU virt)
//! and starts at zero, so it is a monotonic clock and nothing else. The wall
//! clock adds a fixed compile-time epoch to it: the machine has no battery-backed
//! clock and no network, so the date it reports is a stated constant plus the
//! time since reset, not the real time of day.
use crate::Baremetal;
use core::arch::asm;
use dotnet_pal_rs::port::{self, Error, Result};

/// 2026-01-01T00:00:00Z in nanoseconds since the Unix epoch.
pub const EPOCH_NS: u64 = 1_767_225_600 * 1_000_000_000;

pub fn counter() -> u64 {
    let value: u64;
    // SAFETY: reading the virtual counter is permitted at EL1; ISB orders it
    // against earlier instructions so two reads cannot appear out of order.
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) value, options(nomem, nostack)) };
    value
}
pub fn frequency() -> u64 {
    let value: u64;
    // SAFETY: CNTFRQ_EL0 is readable at EL1 and is set by the firmware/QEMU.
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) value, options(nomem, nostack)) };
    value
}
/// Nanoseconds since reset, or `None` when the firmware left the timer frequency
/// unusable. The split conversion keeps full resolution without 128-bit math and
/// is exact for any frequency below about 18 GHz.
pub fn monotonic_ns() -> Option<u64> {
    let frequency = frequency();
    if frequency == 0 || frequency > 18_000_000_000 {
        return None;
    }
    let ticks = counter();
    let seconds = ticks / frequency;
    let rest = ticks % frequency;
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(rest * 1_000_000_000 / frequency)
}
impl port::Clock for Baremetal {
    fn monotonic_ns() -> Result<u64> {
        monotonic_ns().ok_or(Error::Os)
    }
}
impl port::Realtime for Baremetal {
    fn realtime_ns() -> Result<u64> {
        monotonic_ns().map(|ns| EPOCH_NS.saturating_add(ns)).ok_or(Error::Os)
    }
}
impl port::Scheduler for Baremetal {
    /// Runs the other threads until the deadline passes. With nothing else
    /// runnable this is a spin on the counter: there is no timer interrupt and
    /// no low-power idle to return from.
    fn sleep_ns(nanoseconds: u64) -> Result<()> {
        let now = monotonic_ns().ok_or(Error::Os)?;
        let deadline = now.saturating_add(nanoseconds);
        match crate::thread::block_until(Some(deadline), || false) {
            Ok(()) | Err(Error::Timeout) => Ok(()),
            Err(other) => Err(other),
        }
    }
    fn yield_now() -> Result<()> {
        crate::thread::schedule();
        Ok(())
    }
}
