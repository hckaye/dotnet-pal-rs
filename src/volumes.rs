//! Mounted volumes (`CAP_VOLUMES`): the mount points as an enumeration, and the
//! capacity, free space and format of the volume that holds a path.
use crate::io::{self, ACCESS_DENIED};
use crate::port::{Port, Volumes};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 68719476736;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Status { pub total_bytes: u64, pub free_bytes: u64, pub available_bytes: u64, pub format: [u8; 32] }
impl Status {
    pub const EMPTY: Self = Self { total_bytes: 0, free_bytes: 0, available_bytes: 0, format: [0; 32] };
    /// A status whose format name is `format`, cut to the 31 bytes the field holds. The numbers are made
    /// consistent on the way: free space is cut to the capacity and usable space to the free space, because a
    /// target's own figures are taken at different moments and need not agree.
    pub fn new(total_bytes: u64, free_bytes: u64, available_bytes: u64, format: &[u8]) -> Self {
        let free_bytes = free_bytes.min(total_bytes);
        let mut status = Self { total_bytes, free_bytes, available_bytes: available_bytes.min(free_bytes), format: [0; 32] };
        let length = format.iter().position(|b| *b == 0).unwrap_or(format.len()).min(31);
        status.format[..length].copy_from_slice(&format[..length]);
        status
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub entry: Option<unsafe extern "C" fn(usize, *mut u8, usize, *mut usize) -> u32>,
    pub status: Option<unsafe extern "C" fn(*const u8, usize, *mut Status, usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub entry_ok: u64, pub status_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 2;
static COUNTERS: [Counter; 3] = [const { Counter::new() }; 3];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | NOT_FOUND => status,
        BUFFER_TOO_SMALL if index == 0 => status,
        ACCESS_DENIED if index == 1 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
unsafe extern "C" fn entry<V: Volumes>(index: usize, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    let buffer = capacity <= isize::MAX as usize && (capacity == 0 || (!out.is_null() && (out as usize).checked_add(capacity).is_some()));
    if !aligned_output(needed) || !buffer { return record(INVALID_ARGUMENT, 0); }
    unsafe { needed.write(0) };
    record(unsafe { io::text(out, capacity, needed, crate::runtime::MAX_NAME + 1, || V::entry(index, out, capacity)) }, 0)
}
unsafe extern "C" fn status<V: Volumes>(path: *const u8, path_length: usize, out: *mut Status, out_size: usize) -> u32 {
    if !aligned_output(out) || out_size < mem::size_of::<Status>() { return record(INVALID_ARGUMENT, 1); }
    unsafe { out.write(Status::EMPTY) };
    let Some(path) = (unsafe { io::path(path, path_length) }) else { return record(INVALID_ARGUMENT, 1); };
    match V::status(path) {
        // Free space above the capacity, usable space above the free space, or an unterminated name is no answer.
        Ok(s) if s.free_bytes > s.total_bytes || s.available_bytes > s.free_bytes || s.format[31] != 0 => record(OS_ERROR, 1),
        Ok(s) => { unsafe { out.write(s) }; record(OK, 1) }
        Err(e) => record(e.status(), 1),
    }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { entry_ok: COUNTERS[0].load(), status_ok: COUNTERS[1].load(), rejected_or_failed: COUNTERS[FAILED].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { entry: None, status: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's volume provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Volumes;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { entry: Some(entry::<T<P>>), status: Some(status::<T<P>>), read_stats: Some(read_stats) })
}
