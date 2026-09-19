//! Facts about the process and the machine (`CAP_SYSTEM`): the environment as an
//! enumeration, the executable's path, operating system texts, the user, CPU time
//! and uptime. Every question is optional per call; the capability stays whole.
use crate::io;
use crate::port::{Port, SystemInfo};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 1073741824;
pub const TEXT_EXECUTABLE_PATH: u32 = 1;
pub const TEXT_OS_NAME: u32 = 2;
pub const TEXT_OS_RELEASE: u32 = 3;
pub const TEXT_OS_VERSION: u32 = 4;
pub const TEXT_USER_NAME: u32 = 5;
pub const TEXT_HOME_DIRECTORY: u32 = 6;
/// Longest environment entry with its NUL. A variable is not a path: the bound is the
/// largest single string Linux passes to a new program, not the boundary's name limit.
pub const MAX_ENVIRONMENT_ENTRY: usize = 131072;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub environment_entry: Option<unsafe extern "C" fn(usize, *mut u8, usize, *mut usize) -> u32>,
    pub text: Option<unsafe extern "C" fn(u32, *mut u8, usize, *mut usize) -> u32>,
    pub process_times: Option<unsafe extern "C" fn(*mut u64, *mut u64) -> u32>,
    pub uptime_ns: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub user_ids: Option<unsafe extern "C" fn(*mut u32, *mut u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub environment_ok: u64, pub text_ok: u64, pub times_ok: u64, pub identity_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 4;
static COUNTERS: [Counter; 5] = [const { Counter::new() }; 5];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY => status,
        NOT_FOUND if index == 0 => status,
        BUFFER_TOO_SMALL if index <= 1 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
fn valid_buffer(p: *const u8, size: usize) -> bool { size <= isize::MAX as usize && (size == 0 || (!p.is_null() && (p as usize).checked_add(size).is_some())) }
unsafe extern "C" fn environment_entry<S: SystemInfo>(index: usize, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    if !aligned_output(needed) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, 0); }
    unsafe { needed.write(0) };
    let status = unsafe { io::text(out, capacity, needed, MAX_ENVIRONMENT_ENTRY, || S::environment_entry(index, out, capacity)) };
    // An entry is NAME=value with a non-empty name: anything else would reach managed code as a broken variable.
    if status == OK {
        let text = unsafe { core::slice::from_raw_parts(out, needed.read() - 1) };
        if !matches!(text.iter().position(|b| *b == b'='), Some(at) if at > 0) {
            unsafe { core::ptr::write_bytes(out, 0, capacity); needed.write(0); }
            return record(OS_ERROR, 0);
        }
    }
    record(status, 0)
}
unsafe extern "C" fn text<S: SystemInfo>(what: u32, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    if !aligned_output(needed) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, 1); }
    unsafe { needed.write(0) };
    if !(TEXT_EXECUTABLE_PATH..=TEXT_HOME_DIRECTORY).contains(&what) { return record(INVALID_ARGUMENT, 1); }
    record(unsafe { io::text(out, capacity, needed, crate::runtime::MAX_NAME + 1, || S::text(what, out, capacity)) }, 1)
}
unsafe extern "C" fn process_times<S: SystemInfo>(user_ns: *mut u64, kernel_ns: *mut u64) -> u32 {
    if !aligned_output(user_ns) || !aligned_output(kernel_ns) { return record(INVALID_ARGUMENT, 2); }
    unsafe { user_ns.write(0); kernel_ns.write(0); }
    match S::process_times() {
        Ok((user, kernel)) => { unsafe { user_ns.write(user); kernel_ns.write(kernel); } record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe extern "C" fn uptime_ns<S: SystemInfo>(out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 2); }
    unsafe { out.write(0) };
    match S::uptime_ns() {
        Ok(value) => { unsafe { out.write(value) }; record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe extern "C" fn user_ids<S: SystemInfo>(user: *mut u32, group: *mut u32) -> u32 {
    if !aligned_output(user) || !aligned_output(group) { return record(INVALID_ARGUMENT, 3); }
    unsafe { user.write(0); group.write(0); }
    match S::user_ids() {
        Ok((u, g)) => { unsafe { user.write(u); group.write(g); } record(OK, 3) }
        Err(e) => record(e.status(), 3),
    }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { environment_ok: COUNTERS[0].load(), text_ok: COUNTERS[1].load(), times_ok: COUNTERS[2].load(),
        identity_ok: COUNTERS[3].load(), rejected_or_failed: COUNTERS[FAILED].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { environment_entry: None, text: None, process_times: None, uptime_ns: None, user_ids: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's system information provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::SystemInfo;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { environment_entry: Some(environment_entry::<T<P>>), text: Some(text::<T<P>>), process_times: Some(process_times::<T<P>>),
        uptime_ns: Some(uptime_ns::<T<P>>), user_ids: Some(user_ids::<T<P>>), read_stats: Some(read_stats) })
}
