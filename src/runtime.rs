//! Runtime OS services, separately negotiated from VM and synchronization.
//! All pointers are trusted FFI borrows, not sandboxed addresses.
use crate::port::{self, Entropy, Environment, Identity, Lookup, Modules, NativeMapping, Port, Realtime};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP_ENVIRONMENT: u64 = 1024;
pub const CAP_IDENTITY: u64 = 2048;
pub const CAP_REALTIME: u64 = 4096;
pub const CAP_ENTROPY: u64 = 8192;
pub const CAP_NATIVE_MEMORY: u64 = 16384;
pub const CAP_MODULES: u64 = 32768;
pub const BUFFER_TOO_SMALL: u32 = 7;
pub const NOT_FOUND: u32 = 8;
pub const READ: u32 = 1;
pub const WRITE: u32 = 2;
pub const EXECUTE: u32 = 4;
pub const ALL: u64 = CAP_ENVIRONMENT | CAP_IDENTITY | CAP_REALTIME | CAP_ENTROPY | CAP_NATIVE_MEMORY | CAP_MODULES;
pub const MAX_NAME: usize = 4095;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleInfo {
    pub base: *mut c_void,
    pub name: *const u8,
    pub name_length: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub environment_get: Option<unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> u32>,
    pub process_id: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub thread_id: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub realtime_ns: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub random_bytes: Option<unsafe extern "C" fn(*mut u8, usize) -> u32>,
    pub mapping_allocate: Option<unsafe extern "C" fn(usize, u32, *mut *mut c_void) -> u32>,
    pub mapping_release: Option<unsafe extern "C" fn(*mut c_void, usize) -> u32>,
    pub mapping_protect: Option<unsafe extern "C" fn(*mut c_void, usize, u32) -> u32>,
    pub module_open: Option<unsafe extern "C" fn(*const u8, usize, *mut *mut c_void) -> u32>,
    pub module_symbol: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut *mut c_void) -> u32>,
    pub module_close: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub module_info: Option<unsafe extern "C" fn(*mut c_void, *mut ModuleInfo) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub environment_ok: u64, pub identity_ok: u64, pub realtime_ok: u64, pub entropy_ok: u64,
    pub mapping_allocate_ok: u64, pub mapping_release_ok: u64, pub mapping_protect_ok: u64,
    pub module_open_ok: u64, pub module_symbol_ok: u64, pub module_close_ok: u64,
    pub module_info_ok: u64, pub rejected_or_failed: u64,
}
static COUNTERS: [Counter; 12] = [const { Counter::new() }; 12];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY => status,
        BUFFER_TOO_SMALL | NOT_FOUND if index == 0 => status,
        NOT_FOUND if index == 7 || index == 8 || index == 10 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { 11 }].increment();
    status
}
unsafe fn valid_name<'a>(name: *const u8, length: usize, environment: bool) -> Option<&'a [u8]> {
    if name.is_null() || length == 0 || length > MAX_NAME || (name as usize).checked_add(length).is_none() { return None; }
    // SAFETY: caller guarantees a readable borrow for length bytes.
    let bytes = unsafe { core::slice::from_raw_parts(name, length) };
    (!bytes.contains(&0) && (!environment || !bytes.contains(&b'='))).then_some(bytes)
}
fn valid_buffer<T>(p: *mut T, count: usize) -> bool {
    if count > isize::MAX as usize / mem::size_of::<T>().max(1) { return false; }
    count == 0 || (aligned_output(p) && (p as usize).checked_add(count * mem::size_of::<T>()).is_some())
}
unsafe extern "C" fn environment_get<E: Environment>(name: *const u8, len: usize, out: *mut u8, capacity: usize, required: *mut usize) -> u32 {
    if !aligned_output(required) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, 0); }
    unsafe { required.write(0); if capacity != 0 { out.write(0); } }
    let Some(name) = (unsafe { valid_name(name, len, true) }) else { return record(INVALID_ARGUMENT, 0); };
    let status = match unsafe { E::get(name, out, capacity) } {
        Ok(Lookup::Copied(needed)) => {
            // A provider reporting more than the capacity, or no terminator, broke the contract.
            if needed == 0 || needed > capacity || needed > isize::MAX as usize || unsafe { out.add(needed - 1).read() } != 0 { OS_ERROR }
            else { unsafe { required.write(needed) }; OK }
        }
        Ok(Lookup::TooSmall(needed)) => {
            if needed == 0 || needed <= capacity || needed > isize::MAX as usize { OS_ERROR }
            else { unsafe { required.write(needed) }; BUFFER_TOO_SMALL }
        }
        Err(e) => e.status(),
    };
    if status != OK && capacity != 0 { unsafe { out.write(0) }; }
    record(status, 0)
}
unsafe fn scalar(out: *mut u64, index: usize, nonzero: bool, value: port::Result<u64>) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, index); }
    unsafe { out.write(0) };
    match value {
        Ok(0) if nonzero => record(OS_ERROR, index),
        Ok(value) => { unsafe { out.write(value) }; record(OK, index) }
        Err(e) => record(e.status(), index),
    }
}
unsafe extern "C" fn process_id<I: Identity>(out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 1); }
    unsafe { scalar(out, 1, true, I::process_id()) }
}
unsafe extern "C" fn thread_id<I: Identity>(out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 1); }
    unsafe { scalar(out, 1, true, I::thread_id()) }
}
unsafe extern "C" fn realtime_ns<R: Realtime>(out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 2); }
    unsafe { scalar(out, 2, false, R::realtime_ns()) }
}
unsafe extern "C" fn random_bytes<E: Entropy>(out: *mut u8, size: usize) -> u32 {
    if !valid_buffer(out, size) { return record(INVALID_ARGUMENT, 3); }
    let status = if size == 0 { OK } else { port::status(unsafe { E::fill(out, size) }) };
    // Never return partially generated bytes on error. The caller must check status.
    if status != OK && size != 0 { unsafe { ptr::write_bytes(out, 0, size) }; }
    record(status, 3)
}
unsafe extern "C" fn mapping_allocate<M: NativeMapping>(size: usize, protection: u32, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 4); }
    unsafe { out.write(ptr::null_mut()) };
    if size == 0 || size > isize::MAX as usize || protection & !7 != 0 { return record(INVALID_ARGUMENT, 4); }
    let status = match unsafe { M::allocate(size, protection) } {
        Ok(value) => {
            let page = M::page_size();
            if value.is_null() || !page.is_power_of_two() || value as usize % page != 0 || (value as usize).checked_add(size).is_none() { OS_ERROR }
            else { unsafe { out.write(value) }; OK }
        }
        Err(e) => e.status(),
    };
    record(status, 4)
}
unsafe extern "C" fn mapping_release<M: NativeMapping>(address: *mut c_void, size: usize) -> u32 {
    if address.is_null() || size == 0 || !valid_buffer(address.cast::<u8>(), size) { return record(INVALID_ARGUMENT, 5); }
    record(port::status(unsafe { M::release(address, size) }), 5)
}
unsafe extern "C" fn mapping_protect<M: NativeMapping>(address: *mut c_void, size: usize, protection: u32) -> u32 {
    if address.is_null() || size == 0 || !valid_buffer(address.cast::<u8>(), size) || protection & !7 != 0 { return record(INVALID_ARGUMENT, 6); }
    record(port::status(unsafe { M::protect(address, size, protection) }), 6)
}
unsafe extern "C" fn module_open<M: Modules>(name: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 7); }
    unsafe { out.write(ptr::null_mut()) };
    // NULL with length zero explicitly denotes the current process.
    let name = if name.is_null() && len == 0 { None } else {
        match unsafe { valid_name(name, len, false) } { Some(n) => Some(n), None => return record(INVALID_ARGUMENT, 7) }
    };
    let status = match unsafe { M::open(name) } {
        Ok(handle) if handle.is_null() => OS_ERROR,
        Ok(handle) => { unsafe { out.write(handle) }; OK }
        Err(e) => e.status(),
    };
    record(status, 7)
}
unsafe extern "C" fn module_symbol<M: Modules>(handle: *mut c_void, name: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 8); }
    unsafe { out.write(ptr::null_mut()) };
    if handle.is_null() { return record(INVALID_ARGUMENT, 8); }
    let Some(name) = (unsafe { valid_name(name, len, false) }) else { return record(INVALID_ARGUMENT, 8); };
    // A symbol may legitimately have address zero; the status disambiguates it.
    let status = match unsafe { M::symbol(handle, name) } {
        Ok(value) => { unsafe { out.write(value) }; OK }
        Err(e) => e.status(),
    };
    record(status, 8)
}
unsafe extern "C" fn module_close<M: Modules>(handle: *mut c_void) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 9); }
    record(port::status(unsafe { M::close(handle) }), 9)
}
unsafe extern "C" fn module_info<M: Modules>(address: *mut c_void, out: *mut ModuleInfo) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 10); }
    let empty = ModuleInfo { base: ptr::null_mut(), name: ptr::null(), name_length: 0 };
    unsafe { out.write(empty) };
    if address.is_null() { return record(INVALID_ARGUMENT, 10); }
    let status = match unsafe { M::info(address) } {
        Ok(value) => {
            if value.base.is_null() || value.name.is_null() || value.name_length > isize::MAX as usize
                || (value.name as usize).checked_add(value.name_length).is_none() { OS_ERROR }
            else { unsafe { out.write(value) }; OK }
        }
        Err(e) => e.status(),
    };
    record(status, 10)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats {
        environment_ok: COUNTERS[0].load(), identity_ok: COUNTERS[1].load(), realtime_ok: COUNTERS[2].load(),
        entropy_ok: COUNTERS[3].load(), mapping_allocate_ok: COUNTERS[4].load(), mapping_release_ok: COUNTERS[5].load(),
        mapping_protect_ok: COUNTERS[6].load(), module_open_ok: COUNTERS[7].load(), module_symbol_ok: COUNTERS[8].load(),
        module_close_ok: COUNTERS[9].load(), module_info_ok: COUNTERS[10].load(), rejected_or_failed: COUNTERS[11].load(),
    }) };
    OK
}
pub const EMPTY: Ops = Ops {
    environment_get: None, process_id: None, thread_id: None, realtime_ns: None, random_bytes: None,
    mapping_allocate: None, mapping_release: None, mapping_protect: None, module_open: None,
    module_symbol: None, module_close: None, module_info: None, read_stats: Some(read_stats),
};
/// Capability bits and callbacks for the port's runtime service providers.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    let mut caps = 0;
    let mut ops = EMPTY;
    if P::Environment::PROVIDED { caps |= CAP_ENVIRONMENT; ops.environment_get = Some(environment_get::<P::Environment>); }
    if P::Identity::PROVIDED {
        caps |= CAP_IDENTITY;
        ops.process_id = Some(process_id::<P::Identity>); ops.thread_id = Some(thread_id::<P::Identity>);
    }
    if P::Realtime::PROVIDED { caps |= CAP_REALTIME; ops.realtime_ns = Some(realtime_ns::<P::Realtime>); }
    if P::Entropy::PROVIDED { caps |= CAP_ENTROPY; ops.random_bytes = Some(random_bytes::<P::Entropy>); }
    if P::NativeMapping::PROVIDED {
        caps |= CAP_NATIVE_MEMORY;
        ops.mapping_allocate = Some(mapping_allocate::<P::NativeMapping>);
        ops.mapping_release = Some(mapping_release::<P::NativeMapping>);
        ops.mapping_protect = Some(mapping_protect::<P::NativeMapping>);
    }
    if P::Modules::PROVIDED {
        caps |= CAP_MODULES;
        ops.module_open = Some(module_open::<P::Modules>); ops.module_symbol = Some(module_symbol::<P::Modules>);
        ops.module_close = Some(module_close::<P::Modules>); ops.module_info = Some(module_info::<P::Modules>);
    }
    (caps, ops)
}
