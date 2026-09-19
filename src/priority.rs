//! Scheduling priority of a process (`CAP_PRIORITY`), as a niceness from -20 (most
//! favoured) to 19. Process 0 is this process.
use crate::io::ACCESS_DENIED;
use crate::port::{Port, Priority};
use crate::runtime::NOT_FOUND;
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 1099511627776;
pub const MOST_FAVOURED: i32 = -20;
pub const LEAST_FAVOURED: i32 = 19;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub get: Option<unsafe extern "C" fn(u64, *mut i32) -> u32>,
    pub set: Option<unsafe extern "C" fn(u64, i32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub get_ok: u64, pub set_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 2;
static COUNTERS: [Counter; 3] = [const { Counter::new() }; 3];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | NOT_FOUND => status,
        ACCESS_DENIED if index == 1 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
unsafe extern "C" fn get<S: Priority>(process: u64, value: *mut i32) -> u32 {
    if !aligned_output(value) { return record(INVALID_ARGUMENT, 0); }
    unsafe { value.write(0) };
    match S::get(process) {
        Ok(found) if !(MOST_FAVOURED..=LEAST_FAVOURED).contains(&found) => record(OS_ERROR, 0),
        Ok(found) => { unsafe { value.write(found) }; record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn set<S: Priority>(process: u64, value: i32) -> u32 {
    if !(MOST_FAVOURED..=LEAST_FAVOURED).contains(&value) { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(S::set(process, value)), 1)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { get_ok: COUNTERS[0].load(), set_ok: COUNTERS[1].load(), rejected_or_failed: COUNTERS[FAILED].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { get: None, set: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's priority provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Priority;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { get: Some(get::<T<P>>), set: Some(set::<T<P>>), read_stats: Some(read_stats) })
}
