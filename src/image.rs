//! Executable image inspection (`CAP_IMAGE`): unwind-table lookup, memory
//! readability probes and build identifiers. Addresses are trusted borrows.
use crate::port::{self, Error, Image, Port};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 33554432;
/// Bound on a build identifier; ELF notes are 20 bytes, Mach-O UUIDs 16.
pub const MAX_BUILD_ID: usize = 64;

/// Location of one image and its DWARF unwind tables, as the C ABI describes it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnwindInfo {
    pub base: usize,
    pub text_start: usize, pub text_length: usize,
    pub eh_frame_hdr: usize, pub eh_frame_hdr_length: usize,
    pub eh_frame: usize, pub eh_frame_length: usize,
}
impl UnwindInfo {
    /// A provider result that a consumer can act on: a text range containing
    /// nothing impossible and at least one unwind table.
    pub fn consistent(&self, address: usize) -> bool {
        let Some(text_end) = self.text_start.checked_add(self.text_length) else { return false; };
        if self.text_length == 0 || address < self.text_start || address >= text_end { return false; }
        let hdr = self.eh_frame_hdr != 0 && self.eh_frame_hdr_length != 0 && self.eh_frame_hdr.checked_add(self.eh_frame_hdr_length).is_some();
        let frame = self.eh_frame != 0 && self.eh_frame.checked_add(self.eh_frame_length).is_some();
        (hdr || (frame && self.eh_frame_length != 0)) && (self.eh_frame_hdr == 0 || hdr) && (self.eh_frame == 0 || frame)
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub unwind_info: Option<unsafe extern "C" fn(usize, *mut UnwindInfo, usize) -> u32>,
    pub readable: Option<unsafe extern "C" fn(usize, usize) -> u32>,
    pub build_id: Option<unsafe extern "C" fn(usize, *mut u8, usize, *mut usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub unwind_ok: u64, pub readable_ok: u64, pub build_id_ok: u64, pub rejected_or_failed: u64 }
static COUNTERS: [Counter; 4] = [const { Counter::new() }; 4];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR => status,
        NOT_FOUND if index != 3 => status,
        BUFFER_TOO_SMALL if index == 2 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { 3 }].increment();
    status
}
unsafe extern "C" fn unwind_info<I: Image>(address: usize, out: *mut UnwindInfo, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<UnwindInfo>() { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(UnwindInfo::default()) };
    if address == 0 { return record(INVALID_ARGUMENT, 0); }
    let status = match unsafe { I::unwind_info(address) } {
        Ok(info) if info.consistent(address) => { unsafe { out.write(info) }; OK }
        Ok(_) => OS_ERROR, // a table that cannot contain the address is a broken provider
        Err(e) => e.status(),
    };
    record(status, 0)
}
unsafe extern "C" fn readable<I: Image>(address: usize, size: usize) -> u32 {
    if address == 0 || size == 0 || address.checked_add(size).is_none() { return record(INVALID_ARGUMENT, 1); }
    let status = match unsafe { I::readable(address, size) } { Ok(true) => OK, Ok(false) => NOT_FOUND, Err(e) => e.status() };
    record(status, 1)
}
unsafe extern "C" fn build_id<I: Image>(base: usize, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    if !aligned_output(needed) { return record(INVALID_ARGUMENT, 2); }
    unsafe { needed.write(0) };
    if base == 0 || capacity > MAX_BUILD_ID || (capacity != 0 && out.is_null()) { return record(INVALID_ARGUMENT, 2); }
    if capacity != 0 { unsafe { core::ptr::write_bytes(out, 0, capacity) }; }
    let status = match unsafe { I::build_id(base, out, capacity) } {
        Ok(0) => OS_ERROR,
        Ok(length) if length > MAX_BUILD_ID => OS_ERROR,
        Ok(length) => { unsafe { needed.write(length) }; if length > capacity { BUFFER_TOO_SMALL } else { OK } }
        Err(e) => e.status(),
    };
    if status != OK && capacity != 0 { unsafe { core::ptr::write_bytes(out, 0, capacity) }; }
    record(status, 2)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { unwind_ok: COUNTERS[0].load(), readable_ok: COUNTERS[1].load(), build_id_ok: COUNTERS[2].load(), rejected_or_failed: COUNTERS[3].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { unwind_info: None, readable: None, build_id: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's image provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Image;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { unwind_info: Some(unwind_info::<T<P>>), readable: Some(readable::<T<P>>), build_id: Some(build_id::<T<P>>), read_stats: Some(read_stats) })
}
/// Maps a foreign `readable` status to a provider result.
pub fn readable_from_status(status: u32) -> port::Result<bool> {
    match status { OK => Ok(true), NOT_FOUND => Ok(false), other => Err(Error::from_status(other).unwrap_or(Error::Os)) }
}
