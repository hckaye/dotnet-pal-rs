//! Clock/scheduling capability. No std, allocation, managed callbacks or unwinding.
use crate::port::{Clock, Port, Scheduler};
use crate::{aligned_output, Counter, INVALID_ARGUMENT, OK, OS_ERROR};
use core::mem;

pub const CAP_CLOCK: u64 = 4;
pub const CAP_SCHEDULER: u64 = 8;

#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub clock_ok: u64,
    pub sleep_ok: u64,
    pub yield_ok: u64,
    pub rejected_or_failed: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub monotonic_ns: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub sleep_ns: Option<unsafe extern "C" fn(u64) -> u32>,
    pub yield_thread: Option<unsafe extern "C" fn() -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
static CLOCK: Counter = Counter::new();
static SLEEP: Counter = Counter::new();
static YIELD: Counter = Counter::new();
static FAILED: Counter = Counter::new();
fn record(status: u32, counter: &Counter) -> u32 {
    // Unknown foreign status values must not escape as success or a new ABI value.
    crate::record(if status <= crate::OUT_OF_MEMORY { status } else { OS_ERROR }, counter, &FAILED)
}
unsafe extern "C" fn monotonic_ns<C: Clock>(out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, &CLOCK); }
    // SAFETY: writable, aligned output storage is a caller precondition.
    unsafe { out.write(0) };
    match C::monotonic_ns() {
        Ok(value) => { unsafe { out.write(value) }; record(OK, &CLOCK) }
        Err(e) => record(e.status(), &CLOCK),
    }
}
unsafe extern "C" fn sleep_ns<S: Scheduler>(ns: u64) -> u32 {
    record(crate::port::status(S::sleep_ns(ns)), &SLEEP)
}
unsafe extern "C" fn yield_thread<S: Scheduler>() -> u32 {
    record(crate::port::status(S::yield_now()), &YIELD)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    // Individual diagnostic counters; not a synchronized transaction snapshot.
    unsafe { out.write(Stats {
        clock_ok: CLOCK.load(), sleep_ok: SLEEP.load(), yield_ok: YIELD.load(),
        rejected_or_failed: FAILED.load(),
    }) };
    OK
}
pub const EMPTY: Ops = Ops { monotonic_ns: None, sleep_ns: None, yield_thread: None, read_stats: Some(read_stats) };
/// Capability bits and callbacks for the port's clock and scheduler providers.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    let mut caps = 0;
    let mut ops = EMPTY;
    if P::Clock::PROVIDED { caps |= CAP_CLOCK; ops.monotonic_ns = Some(monotonic_ns::<P::Clock>); }
    if P::Scheduler::PROVIDED {
        caps |= CAP_SCHEDULER;
        ops.sleep_ns = Some(sleep_ns::<P::Scheduler>);
        ops.yield_thread = Some(yield_thread::<P::Scheduler>);
    }
    (caps, ops)
}
