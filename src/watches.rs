//! Changes to files and directories, queued for a reader (`CAP_WATCHES`). A
//! watcher is a queue of events; a watch names one directory or file and the
//! events the consumer wants from it.
use crate::io::{self, ACCESS_DENIED, NAME_TOO_LONG, NOT_DIRECTORY, NO_SPACE, TOO_MANY_HANDLES};
use crate::kernel::TIMEOUT;
use crate::port::{Port, Watches};
use crate::runtime::NOT_FOUND;
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP: u64 = 17179869184;
pub const ACCESS: u32 = 1;
pub const MODIFY: u32 = 2;
pub const ATTRIBUTES: u32 = 4;
pub const MOVED_FROM: u32 = 8;
pub const MOVED_TO: u32 = 16;
pub const CREATE: u32 = 32;
pub const DELETE: u32 = 64;
/// Reported, never requested: events were lost (watch 0), the watch has ended, the subject is a directory.
pub const OVERFLOW: u32 = 128;
pub const REMOVED: u32 = 256;
pub const DIRECTORY: u32 = 512;
/// Requested with the events: the path must be a directory; a symbolic link at its end is watched itself.
pub const ONLY_DIRECTORY: u32 = 1024;
pub const NO_FOLLOW: u32 = 2048;
const REQUESTABLE: u32 = ACCESS | MODIFY | ATTRIBUTES | MOVED_FROM | MOVED_TO | CREATE | DELETE;
const REPORTABLE: u32 = REQUESTABLE | OVERFLOW | REMOVED | DIRECTORY;
pub const FOREVER: u64 = u64::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event { pub watch: u32, pub events: u32, pub cookie: u32, pub name_length: u32, pub name: [u8; 256] }
impl Event {
    pub const EMPTY: Self = Self { watch: 0, events: 0, cookie: 0, name_length: 0, name: [0; 256] };
    /// An event about `name` inside the watched directory; `None` when the name does not fit or holds a NUL or a `/`.
    pub fn new(watch: u32, events: u32, cookie: u32, name: &[u8]) -> Option<Self> {
        if name.len() > crate::files::MAX_ENTRY_NAME || name.iter().any(|b| *b == 0 || *b == b'/') { return None; }
        let mut event = Self { watch, events, cookie, name_length: name.len() as u32, ..Self::EMPTY };
        event.name[..name.len()].copy_from_slice(name);
        Some(event)
    }
    pub fn name(&self) -> &[u8] { &self.name[..(self.name_length as usize).min(crate::files::MAX_ENTRY_NAME)] }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub open: Option<unsafe extern "C" fn(*mut *mut c_void) -> u32>,
    pub close: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub add: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize, u32, *mut u32) -> u32>,
    pub remove: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub read: Option<unsafe extern "C" fn(*mut c_void, u64, *mut Event, usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub open_ok: u64, pub close_ok: u64, pub add_ok: u64, pub remove_ok: u64, pub read_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 5;
static COUNTERS: [Counter; 6] = [const { Counter::new() }; 6];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY => status,
        NO_SPACE | TOO_MANY_HANDLES if index == 0 || index == 2 => status,
        NOT_FOUND | NOT_DIRECTORY | ACCESS_DENIED | NAME_TOO_LONG if index == 2 => status,
        TIMEOUT if index == 4 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
unsafe extern "C" fn open<W: Watches>(watcher: *mut *mut c_void) -> u32 {
    if !aligned_output(watcher) { return record(INVALID_ARGUMENT, 0); }
    unsafe { watcher.write(ptr::null_mut()) };
    match W::open() {
        Ok(handle) if handle.is_null() => record(OS_ERROR, 0),
        Ok(handle) => { unsafe { watcher.write(handle) }; record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn close<W: Watches>(watcher: *mut c_void) -> u32 {
    if watcher.is_null() { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(unsafe { W::close(watcher) }), 1)
}
unsafe extern "C" fn add<W: Watches>(watcher: *mut c_void, path: *const u8, path_length: usize, events: u32, watch: *mut u32) -> u32 {
    if !aligned_output(watch) { return record(INVALID_ARGUMENT, 2); }
    unsafe { watch.write(0) };
    let Some(path) = (unsafe { io::path(path, path_length) }) else { return record(INVALID_ARGUMENT, 2); };
    if watcher.is_null() || events & REQUESTABLE == 0 || events & !(REQUESTABLE | ONLY_DIRECTORY | NO_FOLLOW) != 0 { return record(INVALID_ARGUMENT, 2); }
    match unsafe { W::add(watcher, path, events) } {
        // Zero is the watch of an overflow report, so no watch may carry it.
        Ok(0) => record(OS_ERROR, 2),
        Ok(id) => { unsafe { watch.write(id) }; record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe extern "C" fn remove<W: Watches>(watcher: *mut c_void, watch: u32) -> u32 {
    if watcher.is_null() || watch == 0 { return record(INVALID_ARGUMENT, 3); }
    record(crate::port::status(unsafe { W::remove(watcher, watch) }), 3)
}
unsafe extern "C" fn read<W: Watches>(watcher: *mut c_void, timeout_ns: u64, event: *mut Event, event_size: usize) -> u32 {
    if !aligned_output(event) || event_size < mem::size_of::<Event>() { return record(INVALID_ARGUMENT, 4); }
    unsafe { event.write(Event::EMPTY) };
    if watcher.is_null() { return record(INVALID_ARGUMENT, 4); }
    // SAFETY: a writable event is the caller's contract and was just initialised.
    let out = unsafe { &mut *event };
    let status = match unsafe { W::read(watcher, timeout_ns, out) } {
        Ok(()) => {
            // An event a consumer could not act on: no kind, an unknown one, an overflow that names a watch, any
            // other without one, a name that is no entry name or is not terminated.
            let named = out.name_length as usize;
            let broken = out.events & REPORTABLE & !DIRECTORY == 0 || out.events & !REPORTABLE != 0
                || (out.events & OVERFLOW != 0) != (out.watch == 0)
                || named > crate::files::MAX_ENTRY_NAME || out.name[named] != 0 || out.name[..named].iter().any(|b| *b == 0 || *b == b'/');
            if broken { OS_ERROR } else { OK }
        }
        Err(e) => e.status(),
    };
    if status != OK { *out = Event::EMPTY; }
    record(status, 4)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { open_ok: c(0), close_ok: c(1), add_ok: c(2), remove_ok: c(3), read_ok: c(4), rejected_or_failed: c(FAILED) }) };
    OK
}
pub const EMPTY: Ops = Ops { open: None, close: None, add: None, remove: None, read: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's change watching provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Watches;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { open: Some(open::<T<P>>), close: Some(close::<T<P>>), add: Some(add::<T<P>>), remove: Some(remove::<T<P>>),
        read: Some(read::<T<P>>), read_stats: Some(read_stats) })
}
