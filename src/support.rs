//! Native helper resources. Not the GC heap, and not a sandbox for C pointers.
use crate::port::{self, Diagnostics, NativeHeap, Port, RwLocks, ThreadName};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};
pub const CAP_HEAP: u64 = 524288;
pub const CAP_RWLOCK: u64 = 1048576;
pub const CAP_NAME: u64 = 2097152;
pub const CAP_DIAGNOSTICS: u64 = 4194304;
pub const ALL: u64 = CAP_HEAP | CAP_RWLOCK | CAP_NAME | CAP_DIAGNOSTICS;
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
#[derive(Default)]
pub struct Stats {
    pub allocate_ok: u64, pub resize_ok: u64, pub release_ok: u64,
    pub rw_create_ok: u64, pub rw_read_ok: u64, pub rw_write_ok: u64,
    pub rw_unlock_ok: u64, pub rw_destroy_ok: u64, pub write_ok: u64,
    pub name_ok: u64, pub rejected: u64,
}
static COUNTERS: [Counter; 11] = [const { Counter::new() }; 11];
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
fn accept(result: port::Result<*mut c_void>, size: usize, out: *mut *mut c_void) -> u32 {
    match result {
        Ok(p) => {
            if p.is_null() || p as usize % mem::align_of::<usize>() != 0 || (p as usize).checked_add(size).is_none() { OS_ERROR }
            else { unsafe { out.write(p) }; OK }
        }
        Err(e) => e.status(),
    }
}
unsafe extern "C" fn allocate<H: NativeHeap>(size: usize, zero: u32, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(ptr::null_mut()); }
    if !size_ok(size) || zero > 1 { return record(INVALID_ARGUMENT, 0); }
    record(accept(unsafe { H::allocate(size, zero != 0) }, size, out), 0)
}
unsafe extern "C" fn resize<H: NativeHeap>(address: *mut c_void, size: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 1); }
    unsafe { out.write(ptr::null_mut()); }
    if !size_ok(size) { return record(INVALID_ARGUMENT, 1); }
    record(accept(unsafe { H::resize(address, size) }, size, out), 1)
}
unsafe extern "C" fn release<H: NativeHeap>(h: *mut c_void) -> u32 {
    if h.is_null() { return record(OK, 2); }
    record(port::status(unsafe { H::release(h) }), 2)
}
unsafe extern "C" fn rw_create<R: RwLocks>(out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 3); }
    unsafe { out.write(ptr::null_mut()); }
    let status = match unsafe { R::create() } {
        Ok(p) if p.is_null() => OS_ERROR,
        Ok(p) => { unsafe { out.write(p) }; OK }
        Err(e) => e.status(),
    };
    record(status, 3)
}
macro_rules! handle_op {
    ($name:ident, $method:ident, $index:expr) => {
        unsafe extern "C" fn $name<R: RwLocks>(h: *mut c_void) -> u32 {
            if h.is_null() { return record(INVALID_ARGUMENT, $index); }
            record(port::status(unsafe { R::$method(h) }), $index)
        }
    };
}
handle_op!(rw_read, read, 4);
handle_op!(rw_write, write, 5);
handle_op!(rw_unlock, unlock, 6);
handle_op!(rw_destroy, destroy, 7);
unsafe extern "C" fn write_stderr<D: Diagnostics>(data: *const u8, size: usize, written: *mut usize) -> u32 {
    if !aligned_output(written) { return record(INVALID_ARGUMENT, 8); }
    unsafe { written.write(0); }
    if size > isize::MAX as usize || (size != 0 && (data.is_null() || (data as usize).checked_add(size).is_none())) { return record(INVALID_ARGUMENT, 8); }
    let (mut done, mut status) = if size == 0 { (0, OK) } else {
        match unsafe { D::write_stderr(data, size) } { Ok(()) => (size, OK), Err((done, e)) => (done, e.status()) }
    };
    if done > size || (status == OK && done != size) { done = 0; status = OS_ERROR; }
    unsafe { written.write(done); }
    record(status, 8)
}
unsafe extern "C" fn thread_name<N: ThreadName>(name: *const u8, len: usize) -> u32 {
    if name.is_null() || len > 255 || (name as usize).checked_add(len).is_none() { return record(INVALID_ARGUMENT, 9); }
    let bytes = unsafe { core::slice::from_raw_parts(name, len) };
    if bytes.contains(&0) { return record(INVALID_ARGUMENT, 9); }
    record(port::status(N::set(bytes)), 9)
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
/// Capability bits and callbacks for the port's helper-service providers.
/// A NULL callback is the ABI's statement of absence, never a success stub.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    let mut caps = 0;
    let mut ops = EMPTY;
    if P::NativeHeap::PROVIDED {
        caps |= CAP_HEAP;
        ops.allocate = Some(allocate::<P::NativeHeap>); ops.resize = Some(resize::<P::NativeHeap>); ops.release = Some(release::<P::NativeHeap>);
    }
    if P::RwLocks::PROVIDED {
        caps |= CAP_RWLOCK;
        ops.rw_create = Some(rw_create::<P::RwLocks>); ops.rw_read = Some(rw_read::<P::RwLocks>); ops.rw_write = Some(rw_write::<P::RwLocks>);
        ops.rw_unlock = Some(rw_unlock::<P::RwLocks>); ops.rw_destroy = Some(rw_destroy::<P::RwLocks>);
    }
    if P::ThreadName::PROVIDED { caps |= CAP_NAME; ops.thread_name = Some(thread_name::<P::ThreadName>); }
    if P::Diagnostics::PROVIDED { caps |= CAP_DIAGNOSTICS; ops.write_stderr = Some(write_stderr::<P::Diagnostics>); }
    (caps, ops)
}
