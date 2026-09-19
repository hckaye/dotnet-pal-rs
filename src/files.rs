//! Files and directories (`CAP_FILES`). Paths are byte borrows passed verbatim;
//! file handles carry no position, so every transfer names its offset and a
//! consumer that needs a cursor keeps one. Handles are opaque provider objects.
use crate::io::{self, WOULD_BLOCK, ACCESS_DENIED, ALREADY_EXISTS, CROSS_DEVICE, IS_DIRECTORY, NAME_TOO_LONG, NOT_DIRECTORY, NOT_EMPTY, NO_SPACE, READ_ONLY, TOO_MANY_HANDLES};
use crate::kernel::BUSY;
use crate::port::{Files, Port};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP: u64 = 134217728;
pub const READ: u32 = 1;
pub const WRITE: u32 = 2;
pub const CREATE: u32 = 4;
pub const EXCLUSIVE: u32 = 8;
pub const TRUNCATE: u32 = 16;
pub const NODE_FILE: u32 = 1;
pub const NODE_DIRECTORY: u32 = 2;
pub const NODE_SYMLINK: u32 = 3;
pub const NODE_OTHER: u32 = 4;
pub const MAX_ENTRY_NAME: usize = 255;
/// A time `set_times` leaves unchanged, at the C ABI.
pub const TIME_KEEP: u64 = u64::MAX;
pub const LOCK_SHARED: u32 = 1;
pub const LOCK_EXCLUSIVE: u32 = 2;
pub const LOCK_UNLOCK: u32 = 3;
const MODE_MASK: u32 = 0o7777;

/// What a path or an open file is. Times are nanoseconds since the Unix epoch,
/// zero when the target keeps none; `identity` and `device` are zero likewise.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub kind: u32,
    pub mode: u32,
    pub size: u64,
    pub modified_ns: u64, pub accessed_ns: u64, pub changed_ns: u64, pub created_ns: u64,
    pub identity: u64,
    pub device: u64,
}
impl Status {
    fn consistent(&self) -> bool { (NODE_FILE..=NODE_OTHER).contains(&self.kind) && self.mode & !MODE_MASK == 0 }
}
/// Whether `flags` is a combination `open` accepts.
pub const fn valid_open(flags: u32) -> bool {
    flags & !(READ | WRITE | CREATE | EXCLUSIVE | TRUNCATE) == 0 && flags & (READ | WRITE) != 0
        && (flags & EXCLUSIVE == 0 || flags & CREATE != 0) && (flags & TRUNCATE == 0 || flags & WRITE != 0)
}
type PathCall = unsafe extern "C" fn(*const u8, usize) -> u32;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub open: Option<unsafe extern "C" fn(*const u8, usize, u32, u32, *mut *mut c_void) -> u32>,
    pub close: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub read_at: Option<unsafe extern "C" fn(*mut c_void, u64, *mut u8, usize, *mut usize) -> u32>,
    pub write_at: Option<unsafe extern "C" fn(*mut c_void, u64, *const u8, usize, *mut usize) -> u32>,
    pub set_size: Option<unsafe extern "C" fn(*mut c_void, u64) -> u32>,
    pub flush: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub status: Option<unsafe extern "C" fn(*mut c_void, *mut Status, usize) -> u32>,
    pub path_status: Option<unsafe extern "C" fn(*const u8, usize, u32, *mut Status, usize) -> u32>,
    pub remove: Option<PathCall>,
    pub rename: Option<unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> u32>,
    pub directory_create: Option<unsafe extern "C" fn(*const u8, usize, u32) -> u32>,
    pub directory_remove: Option<PathCall>,
    pub directory_open: Option<unsafe extern "C" fn(*const u8, usize, *mut *mut c_void) -> u32>,
    pub directory_read: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize, *mut usize, *mut u32) -> u32>,
    pub directory_close: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub current_directory: Option<unsafe extern "C" fn(*mut u8, usize, *mut usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
    // Optional per call: a provider without the facility answers UNSUPPORTED.
    pub set_mode: Option<unsafe extern "C" fn(*const u8, usize, u32) -> u32>,
    pub set_file_mode: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub set_times: Option<unsafe extern "C" fn(*const u8, usize, u32, u64, u64) -> u32>,
    pub set_file_times: Option<unsafe extern "C" fn(*mut c_void, u64, u64) -> u32>,
    pub link: Option<unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> u32>,
    pub symlink: Option<unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> u32>,
    pub read_link: Option<unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> u32>,
    pub real_path: Option<unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> u32>,
    pub set_current_directory: Option<PathCall>,
    pub lock: Option<unsafe extern "C" fn(*mut c_void, u32, u32) -> u32>,
    pub lock_range: Option<unsafe extern "C" fn(*mut c_void, u64, u64, u32) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub open_ok: u64, pub close_ok: u64, pub read_ok: u64, pub write_ok: u64, pub size_ok: u64, pub flush_ok: u64,
    pub status_ok: u64, pub remove_ok: u64, pub rename_ok: u64, pub directory_ok: u64, pub rejected_or_failed: u64,
    pub attribute_ok: u64, pub link_ok: u64, pub lock_ok: u64,
}
const FAILED: usize = 10;
const ATTRIBUTE: usize = 11;
const LINK: usize = 12;
const LOCK: usize = 13;
static COUNTERS: [Counter; 14] = [const { Counter::new() }; 14];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | BUSY | NOT_FOUND | ALREADY_EXISTS | ACCESS_DENIED
        | IS_DIRECTORY | NOT_DIRECTORY | NOT_EMPTY | NO_SPACE | TOO_MANY_HANDLES | NAME_TOO_LONG | READ_ONLY | CROSS_DEVICE => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
fn valid_buffer(p: *const u8, size: usize) -> bool { size <= isize::MAX as usize && (size == 0 || (!p.is_null() && (p as usize).checked_add(size).is_some())) }
/// A handle result a consumer may keep: non-null.
fn handle(out: *mut *mut c_void, result: crate::port::Result<*mut c_void>) -> u32 {
    match result {
        Ok(value) if value.is_null() => OS_ERROR,
        Ok(value) => { unsafe { out.write(value) }; OK }
        Err(e) => e.status(),
    }
}
unsafe extern "C" fn open<F: Files>(path: *const u8, length: usize, flags: u32, mode: u32, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(ptr::null_mut()) };
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 0); };
    if !valid_open(flags) || mode & !MODE_MASK != 0 { return record(INVALID_ARGUMENT, 0); }
    record(handle(out, unsafe { F::open(path, flags, mode) }), 0)
}
unsafe extern "C" fn close<F: Files>(file: *mut c_void) -> u32 {
    if file.is_null() { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(unsafe { F::close(file) }), 1)
}
unsafe extern "C" fn read_at<F: Files>(file: *mut c_void, offset: u64, data: *mut u8, capacity: usize, got: *mut usize) -> u32 {
    if !aligned_output(got) { return record(INVALID_ARGUMENT, 2); }
    unsafe { got.write(0) };
    if file.is_null() || !valid_buffer(data, capacity) || offset > i64::MAX as u64 { return record(INVALID_ARGUMENT, 2); }
    if capacity == 0 { return record(OK, 2); }
    let status = match unsafe { F::read_at(file, offset, data, capacity) } {
        Ok(done) if done > capacity => OS_ERROR,
        Ok(done) => { unsafe { got.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 2)
}
unsafe extern "C" fn write_at<F: Files>(file: *mut c_void, offset: u64, data: *const u8, size: usize, written: *mut usize) -> u32 {
    if !aligned_output(written) { return record(INVALID_ARGUMENT, 3); }
    unsafe { written.write(0) };
    if file.is_null() || !valid_buffer(data, size) || offset > i64::MAX as u64 || (offset as u128 + size as u128) > i64::MAX as u128 { return record(INVALID_ARGUMENT, 3); }
    if size == 0 { return record(OK, 3); }
    let status = match unsafe { F::write_at(file, offset, data, size) } {
        Ok(0) => OS_ERROR, // no progress reported as success would loop callers forever
        Ok(done) if done > size => OS_ERROR,
        Ok(done) => { unsafe { written.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 3)
}
unsafe extern "C" fn set_size<F: Files>(file: *mut c_void, size: u64) -> u32 {
    if file.is_null() || size > i64::MAX as u64 { return record(INVALID_ARGUMENT, 4); }
    record(crate::port::status(unsafe { F::set_size(file, size) }), 4)
}
unsafe extern "C" fn flush<F: Files>(file: *mut c_void) -> u32 {
    if file.is_null() { return record(INVALID_ARGUMENT, 5); }
    record(crate::port::status(unsafe { F::flush(file) }), 5)
}
unsafe fn describe(out: *mut Status, result: crate::port::Result<Status>) -> u32 {
    match result {
        Ok(value) if !value.consistent() => OS_ERROR,
        Ok(value) => { unsafe { out.write(value) }; OK }
        Err(e) => e.status(),
    }
}
unsafe extern "C" fn status<F: Files>(file: *mut c_void, out: *mut Status, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Status>() { return record(INVALID_ARGUMENT, 6); }
    unsafe { out.write(Status::default()) };
    if file.is_null() { return record(INVALID_ARGUMENT, 6); }
    record(unsafe { describe(out, F::status(file)) }, 6)
}
unsafe extern "C" fn path_status<F: Files>(path: *const u8, length: usize, follow: u32, out: *mut Status, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Status>() { return record(INVALID_ARGUMENT, 6); }
    unsafe { out.write(Status::default()) };
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 6); };
    if follow > 1 { return record(INVALID_ARGUMENT, 6); }
    record(unsafe { describe(out, F::path_status(path, follow != 0)) }, 6)
}
unsafe extern "C" fn remove<F: Files>(path: *const u8, length: usize) -> u32 {
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 7); };
    record(crate::port::status(unsafe { F::remove(path) }), 7)
}
unsafe extern "C" fn rename<F: Files>(from: *const u8, from_length: usize, to: *const u8, to_length: usize) -> u32 {
    let (Some(from), Some(to)) = (unsafe { io::path(from, from_length) }, unsafe { io::path(to, to_length) }) else { return record(INVALID_ARGUMENT, 8); };
    record(crate::port::status(unsafe { F::rename(from, to) }), 8)
}
unsafe extern "C" fn directory_create<F: Files>(path: *const u8, length: usize, mode: u32) -> u32 {
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 9); };
    if mode & !MODE_MASK != 0 { return record(INVALID_ARGUMENT, 9); }
    record(crate::port::status(unsafe { F::create_directory(path, mode) }), 9)
}
unsafe extern "C" fn directory_remove<F: Files>(path: *const u8, length: usize) -> u32 {
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 9); };
    record(crate::port::status(unsafe { F::remove_directory(path) }), 9)
}
unsafe extern "C" fn directory_open<F: Files>(path: *const u8, length: usize, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 9); }
    unsafe { out.write(ptr::null_mut()) };
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 9); };
    record(handle(out, unsafe { F::open_directory(path) }), 9)
}
unsafe extern "C" fn directory_read<F: Files>(directory: *mut c_void, name: *mut u8, capacity: usize, length: *mut usize, kind: *mut u32) -> u32 {
    if !aligned_output(length) || !aligned_output(kind) { return record(INVALID_ARGUMENT, 9); }
    unsafe { length.write(0); kind.write(0); }
    if directory.is_null() || name.is_null() || capacity < MAX_ENTRY_NAME || !valid_buffer(name, capacity) { return record(INVALID_ARGUMENT, 9); }
    loop {
        unsafe { ptr::write_bytes(name, 0, capacity) };
        let (done, node) = match unsafe { F::read_directory(directory, name, capacity) } {
            Ok(entry) => entry,
            Err(e) => { unsafe { ptr::write_bytes(name, 0, capacity) }; return record(e.status(), 9); }
        };
        let text = if done == 0 || done > MAX_ENTRY_NAME { None } else { Some(unsafe { core::slice::from_raw_parts(name, done) }) };
        match text {
            // The two self references are the consumer's to synthesize, never the provider's to list.
            Some(b".") | Some(b"..") => continue,
            Some(text) if (NODE_FILE..=NODE_OTHER).contains(&node) && !text.contains(&0) && !text.contains(&b'/') => {
                unsafe { length.write(done); kind.write(node); }
                return record(OK, 9);
            }
            _ => { unsafe { ptr::write_bytes(name, 0, capacity) }; return record(OS_ERROR, 9); }
        }
    }
}
unsafe extern "C" fn directory_close<F: Files>(directory: *mut c_void) -> u32 {
    if directory.is_null() { return record(INVALID_ARGUMENT, 9); }
    record(crate::port::status(unsafe { F::close_directory(directory) }), 9)
}
unsafe extern "C" fn current_directory<F: Files>(out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    if !aligned_output(needed) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, 9); }
    unsafe { needed.write(0) };
    let status = unsafe { io::text(out, capacity, needed, crate::runtime::MAX_NAME + 1, || F::current_directory(out, capacity)) };
    // BUFFER_TOO_SMALL is this call's only extra status; record() keeps the shared set.
    if status == BUFFER_TOO_SMALL { COUNTERS[FAILED].increment(); return status; }
    record(status, 9)
}
fn times(accessed_ns: u64, modified_ns: u64) -> Option<(Option<u64>, Option<u64>)> {
    let one = |ns: u64| if ns == TIME_KEEP { Some(None) } else if ns > i64::MAX as u64 { None } else { Some(Some(ns)) };
    Some((one(accessed_ns)?, one(modified_ns)?))
}
unsafe extern "C" fn set_mode<F: Files>(path: *const u8, length: usize, mode: u32) -> u32 {
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, ATTRIBUTE); };
    if mode & !MODE_MASK != 0 { return record(INVALID_ARGUMENT, ATTRIBUTE); }
    record(crate::port::status(unsafe { F::set_mode(path, mode) }), ATTRIBUTE)
}
unsafe extern "C" fn set_file_mode<F: Files>(file: *mut c_void, mode: u32) -> u32 {
    if file.is_null() || mode & !MODE_MASK != 0 { return record(INVALID_ARGUMENT, ATTRIBUTE); }
    record(crate::port::status(unsafe { F::set_file_mode(file, mode) }), ATTRIBUTE)
}
unsafe extern "C" fn set_times<F: Files>(path: *const u8, length: usize, follow: u32, accessed_ns: u64, modified_ns: u64) -> u32 {
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, ATTRIBUTE); };
    let Some((accessed, modified)) = times(accessed_ns, modified_ns) else { return record(INVALID_ARGUMENT, ATTRIBUTE); };
    if follow > 1 { return record(INVALID_ARGUMENT, ATTRIBUTE); }
    record(crate::port::status(unsafe { F::set_times(path, follow != 0, accessed, modified) }), ATTRIBUTE)
}
unsafe extern "C" fn set_file_times<F: Files>(file: *mut c_void, accessed_ns: u64, modified_ns: u64) -> u32 {
    let Some((accessed, modified)) = times(accessed_ns, modified_ns) else { return record(INVALID_ARGUMENT, ATTRIBUTE); };
    if file.is_null() { return record(INVALID_ARGUMENT, ATTRIBUTE); }
    record(crate::port::status(unsafe { F::set_file_times(file, accessed, modified) }), ATTRIBUTE)
}
unsafe extern "C" fn link<F: Files>(existing: *const u8, existing_length: usize, created: *const u8, created_length: usize) -> u32 {
    let (Some(existing), Some(created)) = (unsafe { io::path(existing, existing_length) }, unsafe { io::path(created, created_length) }) else { return record(INVALID_ARGUMENT, LINK); };
    record(crate::port::status(unsafe { F::link(existing, created) }), LINK)
}
unsafe extern "C" fn symlink<F: Files>(target: *const u8, target_length: usize, created: *const u8, created_length: usize) -> u32 {
    let (Some(target), Some(created)) = (unsafe { io::path(target, target_length) }, unsafe { io::path(created, created_length) }) else { return record(INVALID_ARGUMENT, LINK); };
    record(crate::port::status(unsafe { F::symlink(target, created) }), LINK)
}
/// A path question answered with a text: `read_link` and `real_path`.
unsafe fn path_text(path: *const u8, length: usize, out: *mut u8, capacity: usize, needed: *mut usize, provider: impl FnOnce(&[u8]) -> crate::port::Result<usize>) -> u32 {
    if !aligned_output(needed) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, LINK); }
    unsafe { needed.write(0) };
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, LINK); };
    let status = unsafe { io::text(out, capacity, needed, crate::runtime::MAX_NAME + 1, || provider(path)) };
    if status == BUFFER_TOO_SMALL { COUNTERS[FAILED].increment(); return status; }
    record(status, LINK)
}
unsafe extern "C" fn read_link<F: Files>(path: *const u8, length: usize, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    unsafe { path_text(path, length, out, capacity, needed, |path| F::read_link(path, out, capacity)) }
}
unsafe extern "C" fn real_path<F: Files>(path: *const u8, length: usize, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    unsafe { path_text(path, length, out, capacity, needed, |path| F::real_path(path, out, capacity)) }
}
unsafe extern "C" fn set_current_directory<F: Files>(path: *const u8, length: usize) -> u32 {
    let Some(path) = (unsafe { io::path(path, length) }) else { return record(INVALID_ARGUMENT, 9); };
    record(crate::port::status(unsafe { F::set_current_directory(path) }), 9)
}
/// WOULD_BLOCK belongs to a lock request that does not wait, so it bypasses the shared set there
/// and nowhere else: from a waiting request or an unlock it is a broken provider.
fn record_lock(status: u32, may_refuse: bool) -> u32 {
    if status == WOULD_BLOCK && may_refuse { COUNTERS[FAILED].increment(); return status; }
    record(status, LOCK)
}
unsafe extern "C" fn lock<F: Files>(file: *mut c_void, mode: u32, wait: u32) -> u32 {
    if file.is_null() || !(LOCK_SHARED..=LOCK_UNLOCK).contains(&mode) || wait > 1 { return record(INVALID_ARGUMENT, LOCK); }
    record_lock(crate::port::status(unsafe { F::lock(file, mode, wait != 0) }), wait == 0 && mode != LOCK_UNLOCK)
}
unsafe extern "C" fn lock_range<F: Files>(file: *mut c_void, offset: u64, length: u64, mode: u32) -> u32 {
    if file.is_null() || !(LOCK_SHARED..=LOCK_UNLOCK).contains(&mode) || length == 0 || offset > i64::MAX as u64 || length > i64::MAX as u64 - offset {
        return record(INVALID_ARGUMENT, LOCK);
    }
    record_lock(crate::port::status(unsafe { F::lock_range(file, offset, length, mode) }), mode != LOCK_UNLOCK)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { open_ok: c(0), close_ok: c(1), read_ok: c(2), write_ok: c(3), size_ok: c(4), flush_ok: c(5),
        status_ok: c(6), remove_ok: c(7), rename_ok: c(8), directory_ok: c(9), rejected_or_failed: c(FAILED),
        attribute_ok: c(ATTRIBUTE), link_ok: c(LINK), lock_ok: c(LOCK) }) };
    OK
}
pub const EMPTY: Ops = Ops { open: None, close: None, read_at: None, write_at: None, set_size: None, flush: None, status: None, path_status: None,
    remove: None, rename: None, directory_create: None, directory_remove: None, directory_open: None, directory_read: None,
    directory_close: None, current_directory: None, read_stats: Some(read_stats), set_mode: None, set_file_mode: None, set_times: None,
    set_file_times: None, link: None, symlink: None, read_link: None, real_path: None, set_current_directory: None, lock: None, lock_range: None };
/// Capability bit and callbacks for the port's file provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Files;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { open: Some(open::<T<P>>), close: Some(close::<T<P>>), read_at: Some(read_at::<T<P>>), write_at: Some(write_at::<T<P>>),
        set_size: Some(set_size::<T<P>>), flush: Some(flush::<T<P>>), status: Some(status::<T<P>>), path_status: Some(path_status::<T<P>>),
        remove: Some(remove::<T<P>>), rename: Some(rename::<T<P>>), directory_create: Some(directory_create::<T<P>>),
        directory_remove: Some(directory_remove::<T<P>>), directory_open: Some(directory_open::<T<P>>),
        directory_read: Some(directory_read::<T<P>>), directory_close: Some(directory_close::<T<P>>),
        current_directory: Some(current_directory::<T<P>>), read_stats: Some(read_stats),
        set_mode: Some(set_mode::<T<P>>), set_file_mode: Some(set_file_mode::<T<P>>), set_times: Some(set_times::<T<P>>),
        set_file_times: Some(set_file_times::<T<P>>), link: Some(link::<T<P>>), symlink: Some(symlink::<T<P>>), read_link: Some(read_link::<T<P>>),
        real_path: Some(real_path::<T<P>>), set_current_directory: Some(set_current_directory::<T<P>>), lock: Some(lock::<T<P>>),
        lock_range: Some(lock_range::<T<P>>) })
}
