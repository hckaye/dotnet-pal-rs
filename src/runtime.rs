//! Runtime OS services, separately negotiated from VM and synchronization.
//! All pointers are trusted FFI borrows, not sandboxed addresses. Host tables are
//! immutable for the process lifetime and must be available before managed startup.
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
pub const CAPABILITIES: u64 = if cfg!(any(feature="linux", feature="host-runtime")) { ALL }
    else if cfg!(feature="wasi-runtime") { CAP_ENVIRONMENT | CAP_REALTIME | CAP_ENTROPY } else { 0 };
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
#[cfg(feature="linux")]
#[path="runtime_linux.rs"]
mod platform;
#[cfg(feature="host-runtime")]
#[path="runtime_host.rs"]
mod platform;
#[cfg(feature="wasi-runtime")]
#[path="runtime_wasi.rs"]
mod platform;
#[cfg(not(any(feature="linux", feature="host-runtime", feature="wasi-runtime")))]
mod platform { use super::*; pub fn ops() -> Option<&'static Ops> { None } }

pub fn available() -> bool {
    CAPABILITIES == 0 || platform::ops().is_some()
}
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
unsafe fn valid_name(name: *const u8, length: usize, environment: bool) -> bool {
    if name.is_null() || length == 0 || length > MAX_NAME || (name as usize).checked_add(length).is_none() { return false; }
    // SAFETY: caller guarantees a readable borrow for length bytes.
    let bytes = unsafe { core::slice::from_raw_parts(name, length) };
    !bytes.contains(&0) && (!environment || !bytes.contains(&b'='))
}
fn valid_buffer<T>(p: *mut T, count: usize) -> bool {
    if count > isize::MAX as usize / mem::size_of::<T>().max(1) { return false; }
    count == 0 || (aligned_output(p) && (p as usize).checked_add(count * mem::size_of::<T>()).is_some())
}
unsafe extern "C" fn environment_get(name: *const u8, len: usize, out: *mut u8, capacity: usize, required: *mut usize) -> u32 {
    if !aligned_output(required) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, 0); }
    unsafe { required.write(0); if capacity != 0 { out.write(0); } }
    if !unsafe { valid_name(name, len, true) } { return record(INVALID_ARGUMENT, 0); }
    let Some(call) = platform::ops().and_then(|o| o.environment_get) else { return record(UNSUPPORTED, 0); };
    let mut needed = 0;
    let mut status = unsafe { call(name, len, out, capacity, &mut needed) };
    if status == OK {
        if needed == 0 || needed > capacity || needed > isize::MAX as usize { status = OS_ERROR; }
        else if unsafe { out.add(needed - 1).read() } != 0 { status = OS_ERROR; }
    } else if status == BUFFER_TOO_SMALL {
        if needed == 0 || needed <= capacity || needed > isize::MAX as usize { status = OS_ERROR; }
    }
    if status == OK || status == BUFFER_TOO_SMALL { unsafe { required.write(needed) }; }
    if status != OK && capacity != 0 { unsafe { out.write(0) }; }
    record(status, 0)
}
macro_rules! scalar {
    ($name:ident, $index:expr, $nonzero:expr) => {
        unsafe extern "C" fn $name(out: *mut u64) -> u32 {
            if !aligned_output(out) { return record(INVALID_ARGUMENT, $index); }
            unsafe { out.write(0) };
            let Some(call) = platform::ops().and_then(|o| o.$name) else { return record(UNSUPPORTED, $index); };
            let mut value = 0;
            let mut status = unsafe { call(&mut value) };
            if status == OK && $nonzero && value == 0 { status = OS_ERROR; }
            if status == OK { unsafe { out.write(value) }; }
            record(status, $index)
        }
    };
}
scalar!(process_id, 1, true);
scalar!(thread_id, 1, true);
scalar!(realtime_ns, 2, false);
unsafe extern "C" fn random_bytes(out: *mut u8, size: usize) -> u32 {
    if !valid_buffer(out, size) { return record(INVALID_ARGUMENT, 3); }
    let Some(call) = platform::ops().and_then(|o| o.random_bytes) else { return record(UNSUPPORTED, 3); };
    let status = if size == 0 { OK } else { unsafe { call(out, size) } };
    // Never return partially generated bytes on error. The caller must check status.
    if status != OK && size != 0 { unsafe { ptr::write_bytes(out, 0, size) }; }
    record(status, 3)
}
fn page_size() -> usize {
    #[cfg(any(feature="linux", feature="host"))]
    return crate::backend::page_size();
    #[cfg(not(any(feature="linux", feature="host")))]
    0
}
unsafe extern "C" fn mapping_allocate(size: usize, protection: u32, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 4); }
    unsafe { out.write(ptr::null_mut()) };
    if size == 0 || size > isize::MAX as usize || protection & !7 != 0 { return record(INVALID_ARGUMENT, 4); }
    let Some(call) = platform::ops().and_then(|o| o.mapping_allocate) else { return record(UNSUPPORTED, 4); };
    let mut value = ptr::null_mut();
    let mut status = unsafe { call(size, protection, &mut value) };
    if status == OK {
        if value.is_null() || !page_size().is_power_of_two() || value as usize % page_size() != 0 || (value as usize).checked_add(size).is_none() { status = OS_ERROR; }
        else { unsafe { out.write(value) }; }
    }
    record(status, 4)
}
unsafe extern "C" fn mapping_release(address: *mut c_void, size: usize) -> u32 {
    if address.is_null() || size == 0 || !valid_buffer(address.cast::<u8>(), size) { return record(INVALID_ARGUMENT, 5); }
    let Some(call) = platform::ops().and_then(|o| o.mapping_release) else { return record(UNSUPPORTED, 5); };
    record(unsafe { call(address, size) }, 5)
}
unsafe extern "C" fn mapping_protect(address: *mut c_void, size: usize, protection: u32) -> u32 {
    if address.is_null() || size == 0 || !valid_buffer(address.cast::<u8>(), size) || protection & !7 != 0 { return record(INVALID_ARGUMENT, 6); }
    let Some(call) = platform::ops().and_then(|o| o.mapping_protect) else { return record(UNSUPPORTED, 6); };
    record(unsafe { call(address, size, protection) }, 6)
}
unsafe extern "C" fn module_open(name: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 7); }
    unsafe { out.write(ptr::null_mut()) };
    // NULL with length zero explicitly denotes the current process.
    if !(name.is_null() && len == 0) && !unsafe { valid_name(name, len, false) } { return record(INVALID_ARGUMENT, 7); }
    let Some(call) = platform::ops().and_then(|o| o.module_open) else { return record(UNSUPPORTED, 7); };
    let mut handle = ptr::null_mut();
    let mut status = unsafe { call(name, len, &mut handle) };
    if status == OK {
        if handle.is_null() { status = OS_ERROR; } else { unsafe { out.write(handle) }; }
    }
    record(status, 7)
}
unsafe extern "C" fn module_symbol(handle: *mut c_void, name: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 8); }
    unsafe { out.write(ptr::null_mut()) };
    if handle.is_null() || !unsafe { valid_name(name, len, false) } { return record(INVALID_ARGUMENT, 8); }
    let Some(call) = platform::ops().and_then(|o| o.module_symbol) else { return record(UNSUPPORTED, 8); };
    let mut value = ptr::null_mut();
    let status = unsafe { call(handle, name, len, &mut value) };
    // A symbol may legitimately have address zero; the status disambiguates it.
    if status == OK { unsafe { out.write(value) }; }
    record(status, 8)
}
unsafe extern "C" fn module_close(handle: *mut c_void) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 9); }
    let Some(call) = platform::ops().and_then(|o| o.module_close) else { return record(UNSUPPORTED, 9); };
    record(unsafe { call(handle) }, 9)
}
unsafe extern "C" fn module_info(address: *mut c_void, out: *mut ModuleInfo) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 10); }
    let empty = ModuleInfo { base: ptr::null_mut(), name: ptr::null(), name_length: 0 };
    unsafe { out.write(empty) };
    if address.is_null() { return record(INVALID_ARGUMENT, 10); }
    let Some(call) = platform::ops().and_then(|o| o.module_info) else { return record(UNSUPPORTED, 10); };
    let mut value = empty;
    let mut status = unsafe { call(address, &mut value) };
    if status == OK {
        if value.base.is_null() || value.name.is_null() || value.name_length > isize::MAX as usize
            || (value.name as usize).checked_add(value.name_length).is_none() { status = OS_ERROR; }
        else { unsafe { out.write(value) }; }
    }
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
pub const OPS: Ops = Ops {
    environment_get: if CAPABILITIES & CAP_ENVIRONMENT != 0 { Some(environment_get) } else { None },
    process_id: if CAPABILITIES & CAP_IDENTITY != 0 { Some(process_id) } else { None },
    thread_id: if CAPABILITIES & CAP_IDENTITY != 0 { Some(thread_id) } else { None },
    realtime_ns: if CAPABILITIES & CAP_REALTIME != 0 { Some(realtime_ns) } else { None },
    random_bytes: if CAPABILITIES & CAP_ENTROPY != 0 { Some(random_bytes) } else { None },
    mapping_allocate: if CAPABILITIES & CAP_NATIVE_MEMORY != 0 { Some(mapping_allocate) } else { None },
    mapping_release: if CAPABILITIES & CAP_NATIVE_MEMORY != 0 { Some(mapping_release) } else { None },
    mapping_protect: if CAPABILITIES & CAP_NATIVE_MEMORY != 0 { Some(mapping_protect) } else { None },
    module_open: if CAPABILITIES & CAP_MODULES != 0 { Some(module_open) } else { None },
    module_symbol: if CAPABILITIES & CAP_MODULES != 0 { Some(module_symbol) } else { None },
    module_close: if CAPABILITIES & CAP_MODULES != 0 { Some(module_close) } else { None },
    module_info: if CAPABILITIES & CAP_MODULES != 0 { Some(module_info) } else { None },
    read_stats: Some(read_stats),
};
