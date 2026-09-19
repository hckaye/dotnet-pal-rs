//! Files mapped into memory (`CAP_MAPPINGS`). The group takes the handles of the
//! files group, so the type that provides [`crate::port::Files`] provides this one.
use crate::io::ACCESS_DENIED;
use crate::port::{Mappings, Port};
use crate::runtime::{EXECUTE, READ, WRITE};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP: u64 = 34359738368;
/// Writes reach the file and every other shared mapping of it.
pub const SHARED: u32 = 1;
/// Writes stay in the mapping.
pub const PRIVATE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub map: Option<unsafe extern "C" fn(*mut c_void, u64, usize, u32, u32, *mut *mut c_void) -> u32>,
    pub unmap: Option<unsafe extern "C" fn(*mut c_void, usize) -> u32>,
    pub sync: Option<unsafe extern "C" fn(*mut c_void, usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub map_ok: u64, pub unmap_ok: u64, pub sync_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 3;
static COUNTERS: [Counter; 4] = [const { Counter::new() }; 4];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR => status,
        OUT_OF_MEMORY | ACCESS_DENIED if index == 0 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
fn valid_range(address: *mut c_void, length: usize) -> bool { !address.is_null() && length != 0 && length <= isize::MAX as usize && (address as usize).checked_add(length).is_some() }
unsafe extern "C" fn map<M: Mappings>(file: *mut c_void, offset: u64, length: usize, access: u32, mode: u32, address: *mut *mut c_void) -> u32 {
    if !aligned_output(address) { return record(INVALID_ARGUMENT, 0); }
    unsafe { address.write(ptr::null_mut()) };
    let end = offset.checked_add(length as u64);
    if file.is_null() || length == 0 || length > isize::MAX as usize || !matches!(end, Some(end) if end <= i64::MAX as u64)
        || access == 0 || access & !(READ | WRITE | EXECUTE) != 0 || !matches!(mode, SHARED | PRIVATE) { return record(INVALID_ARGUMENT, 0); }
    match unsafe { M::map(file, offset, length, access, mode == SHARED) } {
        Ok(mapped) if mapped.is_null() || (mapped as usize).checked_add(length).is_none() => record(OS_ERROR, 0),
        Ok(mapped) => { unsafe { address.write(mapped.cast()) }; record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn unmap<M: Mappings>(address: *mut c_void, length: usize) -> u32 {
    if !valid_range(address, length) { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(unsafe { M::unmap(address.cast(), length) }), 1)
}
unsafe extern "C" fn sync<M: Mappings>(address: *mut c_void, length: usize) -> u32 {
    if !valid_range(address, length) { return record(INVALID_ARGUMENT, 2); }
    record(crate::port::status(unsafe { M::sync(address.cast(), length) }), 2)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { map_ok: c(0), unmap_ok: c(1), sync_ok: c(2), rejected_or_failed: c(FAILED) }) };
    OK
}
pub const EMPTY: Ops = Ops { map: None, unmap: None, sync: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's file mapping provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Mappings;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { map: Some(map::<T<P>>), unmap: Some(unmap::<T<P>>), sync: Some(sync::<T<P>>), read_stats: Some(read_stats) })
}
