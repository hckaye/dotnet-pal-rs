//! Standard streams (`CAP_STREAMS`): the three console streams a program starts
//! with. Stream 0 is input, 1 output, 2 error output. This is not a file API:
//! there are no positions, no other descriptors and no sockets.
use crate::port::{Port, Streams};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 67108864;
pub const INPUT: u32 = 0;
pub const OUTPUT: u32 = 1;
pub const ERROR: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub write: Option<unsafe extern "C" fn(u32, *const u8, usize, *mut usize) -> u32>,
    pub read: Option<unsafe extern "C" fn(u32, *mut u8, usize, *mut usize) -> u32>,
    pub is_terminal: Option<unsafe extern "C" fn(u32, *mut u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub write_ok: u64, pub read_ok: u64, pub query_ok: u64, pub rejected_or_failed: u64 }
static COUNTERS: [Counter; 4] = [const { Counter::new() }; 4];

fn record(status: u32, index: usize) -> u32 {
    let status = match status { OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR => status, _ => OS_ERROR };
    COUNTERS[if status == OK { index } else { 3 }].increment();
    status
}
fn valid_buffer(p: *const u8, size: usize) -> bool { size <= isize::MAX as usize && (size == 0 || (!p.is_null() && (p as usize).checked_add(size).is_some())) }
unsafe extern "C" fn write<S: Streams>(stream: u32, data: *const u8, size: usize, written: *mut usize) -> u32 {
    if !aligned_output(written) { return record(INVALID_ARGUMENT, 0); }
    unsafe { written.write(0) };
    if stream == INPUT || stream > ERROR || !valid_buffer(data, size) { return record(INVALID_ARGUMENT, 0); }
    if size == 0 { return record(OK, 0); }
    let status = match unsafe { S::write(stream, data, size) } {
        Ok(0) => OS_ERROR, // no progress reported as success would loop callers forever
        Ok(done) if done > size => OS_ERROR,
        Ok(done) => { unsafe { written.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 0)
}
unsafe extern "C" fn read<S: Streams>(stream: u32, data: *mut u8, capacity: usize, got: *mut usize) -> u32 {
    if !aligned_output(got) { return record(INVALID_ARGUMENT, 1); }
    unsafe { got.write(0) };
    if stream != INPUT || !valid_buffer(data, capacity) { return record(INVALID_ARGUMENT, 1); }
    if capacity == 0 { return record(OK, 1); }
    // Zero bytes with OK is end of input.
    let status = match unsafe { S::read(stream, data, capacity) } {
        Ok(done) if done > capacity => OS_ERROR,
        Ok(done) => { unsafe { got.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 1)
}
unsafe extern "C" fn is_terminal<S: Streams>(stream: u32, out: *mut u32) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 2); }
    unsafe { out.write(0) };
    if stream > ERROR { return record(INVALID_ARGUMENT, 2); }
    match S::is_terminal(stream) {
        Ok(value) => { unsafe { out.write(value as u32) }; record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { write_ok: COUNTERS[0].load(), read_ok: COUNTERS[1].load(), query_ok: COUNTERS[2].load(), rejected_or_failed: COUNTERS[3].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { write: None, read: None, is_terminal: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's standard stream provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Streams;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { write: Some(write::<T<P>>), read: Some(read::<T<P>>), is_terminal: Some(is_terminal::<T<P>>), read_stats: Some(read_stats) })
}
