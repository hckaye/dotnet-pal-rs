//! Native helper resources. Not the GC heap, and not a sandbox for C pointers.
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};
pub const CAP_HEAP: u64 = 524288;
pub const CAP_RWLOCK: u64 = 1048576;
pub const CAP_NAME: u64 = 2097152;
pub const CAP_DIAGNOSTICS: u64 = 4194304;
pub const ALL: u64 = CAP_HEAP | CAP_RWLOCK | CAP_NAME | CAP_DIAGNOSTICS;
pub const CAPABILITIES: u64 = if cfg!(any(feature="linux", feature="host-support")) { ALL } else { 0 };
type HandleOp = unsafe extern "C" fn(*mut c_void) -> u32;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub allocate: Option<unsafe extern "C" fn(usize, u32, *mut *mut c_void) -> u32>,
    pub resize: Option<unsafe extern "C" fn(*mut c_void, usize, *mut *mut c_void) -> u32>,
    pub release: Option<HandleOp>,
    pub rw_create: Option<unsafe extern "C" fn(*mut *mut c_void) -> u32>,
    pub rw_read: Option<HandleOp>, pub rw_write: Option<HandleOp>,
    pub rw_unlock: Option<HandleOp>, pub rw_destroy: Option<HandleOp>,
    pub write_stderr: Option<unsafe extern "C" fn(*const u8, usize, *mut usize) -> u32>,
    pub thread_name: Option<unsafe extern "C" fn(*const u8, usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
pub struct Stats {
    pub allocate_ok: u64, pub resize_ok: u64, pub release_ok: u64,
    pub rw_create_ok: u64, pub rw_read_ok: u64, pub rw_write_ok: u64,
    pub rw_unlock_ok: u64, pub rw_destroy_ok: u64, pub write_ok: u64,
    pub name_ok: u64, pub rejected: u64,
}
static COUNTERS: [Counter; 11] = [const { Counter::new() }; 11];
#[cfg(feature="linux")]
#[path="support_linux.rs"]
mod platform;
#[cfg(feature="host-support")]
mod platform {
    use super::*;
    static CACHED: core::sync::atomic::AtomicPtr<Ops> = core::sync::atomic::AtomicPtr::new(ptr::null_mut());
    extern "C" { fn dotnet_pal_host_support_v2() -> *const Host; }
    pub fn ops() -> Option<&'static Ops> {
        let cached = CACHED.load(core::sync::atomic::Ordering::Acquire);
        if !cached.is_null() { return Some(unsafe { &*cached }); }
        let p = unsafe { dotnet_pal_host_support_v2() };
        if p.is_null() || (p as usize) % mem::align_of::<Host>() != 0 { return None; }
        // Host guarantees at least a readable header, then the advertised extent.
        let h = unsafe { ptr::read(p.cast::<Header>()) };
        if h.abi_version != crate::ABI_VERSION || (h.struct_size as usize) < mem::size_of::<Host>() || h.capabilities & ALL != ALL { return None; }
        let o = unsafe { &(*p).ops };
        if o.allocate.is_none() || o.resize.is_none() || o.release.is_none() || o.rw_create.is_none()
            || o.rw_read.is_none() || o.rw_write.is_none() || o.rw_unlock.is_none() || o.rw_destroy.is_none()
            || o.write_stderr.is_none() || o.thread_name.is_none() { return None; }
        CACHED.store(o as *const Ops as *mut Ops, core::sync::atomic::Ordering::Release);
        Some(o)
    }
}
#[cfg(not(any(feature="linux", feature="host-support")))]
mod platform { use super::*; pub fn ops() -> Option<&'static Ops> { None } }
pub fn available() -> bool { CAPABILITIES == 0 || platform::ops().is_some() }
fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | UNSUPPORTED => status,
        crate::kernel::BUSY if index == 7 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { 10 }].increment();
    status
}
fn size_ok(size: usize) -> bool { size != 0 && size <= isize::MAX as usize }
unsafe extern "C" fn allocate(size: usize, zero: u32, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(ptr::null_mut()); }
    if !size_ok(size) || zero > 1 { return record(INVALID_ARGUMENT, 0); }
    let Some(f) = platform::ops().and_then(|o| o.allocate) else { return record(UNSUPPORTED, 0); };
    let mut result = ptr::null_mut();
    let mut status = unsafe { f(size, zero, &mut result) };
    if status == OK {
        if result.is_null() || result as usize % mem::align_of::<usize>() != 0 || (result as usize).checked_add(size).is_none() { status = OS_ERROR; }
        else { unsafe { out.write(result) }; }
    }
    record(status, 0)
}
unsafe extern "C" fn resize(address: *mut c_void, size: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 1); }
    unsafe { out.write(ptr::null_mut()); }
    if !size_ok(size) { return record(INVALID_ARGUMENT, 1); }
    let Some(f) = platform::ops().and_then(|o| o.resize) else { return record(UNSUPPORTED, 1); };
    let mut result = ptr::null_mut();
    let mut status = unsafe { f(address, size, &mut result) };
    if status == OK {
        if result.is_null() || result as usize % mem::align_of::<usize>() != 0 || (result as usize).checked_add(size).is_none() { status = OS_ERROR; }
        else { unsafe { out.write(result) }; }
    }
    record(status, 1)
}
unsafe extern "C" fn rw_create(out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 3); }
    unsafe { out.write(ptr::null_mut()); }
    let Some(f) = platform::ops().and_then(|o| o.rw_create) else { return record(UNSUPPORTED, 3); };
    let mut result = ptr::null_mut();
    let mut status = unsafe { f(&mut result) };
    if status == OK { if result.is_null() { status = OS_ERROR; } else { unsafe { out.write(result); } } }
    record(status, 3)
}
macro_rules! handle_op {
    ($name:ident, $index:expr, $null_ok:expr) => {
        unsafe extern "C" fn $name(h: *mut c_void) -> u32 {
            if h.is_null() {
                return record(if $null_ok { OK } else { INVALID_ARGUMENT }, $index);
            }
            let Some(f) = platform::ops().and_then(|o| o.$name) else { return record(UNSUPPORTED, $index); };
            record(unsafe { f(h) }, $index)
        }
    };
}
handle_op!(release, 2, true);
handle_op!(rw_read, 4, false);
handle_op!(rw_write, 5, false);
handle_op!(rw_unlock, 6, false);
handle_op!(rw_destroy, 7, false);
unsafe extern "C" fn write_stderr(data: *const u8, size: usize, written: *mut usize) -> u32 {
    if !aligned_output(written) { return record(INVALID_ARGUMENT, 8); }
    unsafe { written.write(0); }
    if size > isize::MAX as usize || (size != 0 && (data.is_null() || (data as usize).checked_add(size).is_none())) { return record(INVALID_ARGUMENT, 8); }
    let Some(f) = platform::ops().and_then(|o| o.write_stderr) else { return record(UNSUPPORTED, 8); };
    let mut done = 0;
    let mut status = if size == 0 { OK } else { unsafe { f(data, size, &mut done) } };
    if done > size || (status == OK && done != size) { done = 0; status = OS_ERROR; }
    unsafe { written.write(done); }
    record(status, 8)
}
unsafe extern "C" fn thread_name(name: *const u8, len: usize) -> u32 {
    if name.is_null() || len > 255 || (name as usize).checked_add(len).is_none() { return record(INVALID_ARGUMENT, 9); }
    if unsafe { core::slice::from_raw_parts(name,len) }.contains(&0) { return record(INVALID_ARGUMENT, 9); }
    let Some(f) = platform::ops().and_then(|o| o.thread_name) else { return record(UNSUPPORTED, 9); };
    record(unsafe { f(name, len) }, 9)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { allocate_ok: COUNTERS[0].load(), resize_ok: COUNTERS[1].load(), release_ok: COUNTERS[2].load(),
        rw_create_ok: COUNTERS[3].load(), rw_read_ok: COUNTERS[4].load(), rw_write_ok: COUNTERS[5].load(),
        rw_unlock_ok: COUNTERS[6].load(), rw_destroy_ok: COUNTERS[7].load(), write_ok: COUNTERS[8].load(),
        name_ok: COUNTERS[9].load(), rejected: COUNTERS[10].load() }); }
    OK
}
pub const EMPTY: Ops = Ops { allocate: None, resize: None, release: None, rw_create: None,
    rw_read: None, rw_write: None, rw_unlock: None, rw_destroy: None, write_stderr: None,
    thread_name: None, read_stats: Some(read_stats) };
pub const OPS: Ops = if CAPABILITIES == 0 { EMPTY } else { Ops {
    allocate: Some(allocate), resize: Some(resize), release: Some(release), rw_create: Some(rw_create),
    rw_read: Some(rw_read), rw_write: Some(rw_write), rw_unlock: Some(rw_unlock), rw_destroy: Some(rw_destroy),
    write_stderr: Some(write_stderr), thread_name: Some(thread_name), read_stats: Some(read_stats),
} };
