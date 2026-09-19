//! Linux provider for the watches group: one inotify instance per watcher, with
//! the kernel's watch descriptor (1 and up) as the watch id. No Rust heap; the
//! handle is one CRT allocation that holds the descriptor and what the last
//! kernel read returned beyond the event that was asked for.
//!
//! The descriptor never changes once `open` has returned, and it is all `add` and
//! `remove` look at, so they run beside a reader without a lock: the kernel
//! serializes the instance. The buffer and its two offsets are the reader's alone.
//! `remove` wakes a reader that waits because the kernel queues IN_IGNORED for it.
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::watches::{Event, ACCESS, ATTRIBUTES, CREATE, DELETE, DIRECTORY, FOREVER, MODIFY, MOVED_FROM, MOVED_TO, NO_FOLLOW, ONLY_DIRECTORY, OVERFLOW, REMOVED};
use core::{ffi::c_void, mem, ptr};

/// The events a consumer asks for, as the contract and as inotify name them.
const KINDS: [(u32, u32); 7] = [(ACCESS, libc::IN_ACCESS), (MODIFY, libc::IN_MODIFY), (ATTRIBUTES, libc::IN_ATTRIB), (MOVED_FROM, libc::IN_MOVED_FROM),
    (MOVED_TO, libc::IN_MOVED_TO), (CREATE, libc::IN_CREATE), (DELETE, libc::IN_DELETE)];
/// `struct inotify_event` without the name that follows it.
const HEADER: usize = mem::size_of::<libc::inotify_event>();
/// Room for a dozen events with the longest name; the kernel refuses a read that cannot hold the next event.
const CAPACITY: usize = 4096;
const LOST: Event = Event { events: OVERFLOW, ..Event::EMPTY };

#[repr(C)]
struct Watcher { fd: i32, next: usize, filled: usize, buffer: [u8; CAPACITY] }

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn watcher(handle: *mut c_void) -> Result<*mut Watcher> {
    if handle.is_null() || !(handle as usize).is_multiple_of(mem::align_of::<Watcher>()) { return Err(Error::InvalidArgument); }
    Ok(handle.cast())
}
/// A path argument with the terminator the C library needs.
struct CPath([u8; 4096]);
impl CPath {
    fn new(path: &[u8]) -> Result<Self> {
        let mut bytes = [0u8; 4096];
        if path.len() >= bytes.len() { return Err(Error::NameTooLong); }
        bytes[..path.len()].copy_from_slice(path);
        Ok(Self(bytes))
    }
    fn as_ptr(&self) -> *const libc::c_char { self.0.as_ptr().cast() }
}
/// The contract's word for one kernel event; `None` for an event it has no word for. That is IN_UNMOUNT: the
/// IN_IGNORED the kernel sends behind it is what ends the watch.
fn translate(wd: i32, mask: u32, cookie: u32, name: &[u8]) -> Option<Event> {
    if mask & libc::IN_Q_OVERFLOW != 0 { return Some(LOST); }
    let mut events = KINDS.iter().fold(0, |all, (kind, bit)| if mask & bit != 0 { all | kind } else { all });
    if mask & libc::IN_IGNORED != 0 { events |= REMOVED; }
    if events == 0 || wd <= 0 { return None; }
    if mask & libc::IN_ISDIR != 0 { events |= DIRECTORY; }
    // A name the event cannot carry is an event the consumer did not get, and that is what an overflow says.
    Some(Event::new(wd as u32, events, cookie, name).unwrap_or(LOST))
}
/// The next event of the last kernel read that the contract has a word for.
unsafe fn buffered(w: *mut Watcher) -> Result<Option<Event>> {
    loop {
        let (next, filled) = unsafe { ((*w).next, (*w).filled) };
        if next == filled { return Ok(None); }
        // SAFETY: only the reader touches the buffer, and the contract allows one reader at a time.
        let buffer = unsafe { &(&(*w).buffer)[next..filled] };
        if buffer.len() < HEADER { unsafe { (*w).next = filled }; return Err(Error::Os); }
        // SAFETY: HEADER readable bytes were just checked; the read does not rely on their alignment.
        let header = unsafe { ptr::read_unaligned(buffer.as_ptr().cast::<libc::inotify_event>()) };
        let Some(name) = buffer.get(HEADER..HEADER + header.len as usize) else { unsafe { (*w).next = filled }; return Err(Error::Os); };
        unsafe { (*w).next = next + HEADER + name.len() };
        // The kernel pads the name with NULs up to the next event's alignment.
        let name = &name[..name.iter().position(|byte| *byte == 0).unwrap_or(name.len())];
        if let Some(event) = translate(header.wd, header.mask, header.cookie, name) { return Ok(Some(event)); }
    }
}

impl port::Watches for Linux {
    fn open() -> Result<*mut c_void> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(match errno() {
                // The kernel has one errno for a full descriptor table and for the user's limit of inotify instances.
                // The second is the target's limit of watchers, and it is the one that still leaves room for a descriptor.
                libc::EMFILE => {
                    let spare = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
                    if spare < 0 { Error::TooManyHandles } else { unsafe { libc::close(spare) }; Error::NoSpace }
                }
                libc::ENFILE => Error::TooManyHandles,
                libc::ENOMEM => Error::OutOfMemory,
                _ => Error::Os,
            });
        }
        let handle = unsafe { libc::calloc(1, mem::size_of::<Watcher>()) }.cast::<Watcher>();
        if handle.is_null() { unsafe { libc::close(fd) }; return Err(Error::OutOfMemory); }
        unsafe { (*handle).fd = fd };
        Ok(handle.cast())
    }
    unsafe fn close(watcher_handle: *mut c_void) -> Result<()> {
        let w = watcher(watcher_handle)?;
        // close releases the descriptor even when it reports EINTR, so it is never retried.
        let code = if unsafe { libc::close((*w).fd) } == 0 { 0 } else { errno() };
        unsafe { libc::free(watcher_handle) };
        if code == 0 || code == libc::EINTR { Ok(()) } else { Err(Error::Os) }
    }
    unsafe fn add(watcher_handle: *mut c_void, path: &[u8], events: u32) -> Result<u32> {
        let w = watcher(watcher_handle)?;
        let path = CPath::new(path)?;
        // Without IN_MASK_ADD a node the instance already watches keeps its descriptor and takes the new mask. A file that
        // was unlinked while something still holds it open stops reporting, as a path-based consumer expects.
        let mut mask = KINDS.iter().fold(libc::IN_EXCL_UNLINK, |all, (kind, bit)| if events & kind != 0 { all | bit } else { all });
        if events & ONLY_DIRECTORY != 0 { mask |= libc::IN_ONLYDIR; }
        if events & NO_FOLLOW != 0 { mask |= libc::IN_DONT_FOLLOW; }
        let wd = unsafe { libc::inotify_add_watch((*w).fd, path.as_ptr(), mask) };
        if wd > 0 { return Ok(wd as u32); }
        Err(if wd == 0 { Error::Os } else {
            match errno() {
                libc::ENOENT => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, libc::ENOTDIR => Error::NotDirectory,
                libc::ENAMETOOLONG => Error::NameTooLong, libc::ENOSPC => Error::NoSpace, libc::ENOMEM => Error::OutOfMemory, _ => Error::Os,
            }
        })
    }
    unsafe fn remove(watcher_handle: *mut c_void, watch: u32) -> Result<()> {
        let w = watcher(watcher_handle)?;
        let Ok(wd) = i32::try_from(watch) else { return Err(Error::InvalidArgument); };
        // EINVAL is a descriptor this instance does not have: never given out, removed before, or ended with its node.
        if unsafe { libc::inotify_rm_watch((*w).fd, wd) } == 0 { Ok(()) } else if errno() == libc::EINVAL { Err(Error::InvalidArgument) } else { Err(Error::Os) }
    }
    unsafe fn read(watcher_handle: *mut c_void, timeout_ns: u64, event: &mut Event) -> Result<()> {
        let w = watcher(watcher_handle)?;
        let mut deadline = None;
        loop {
            if let Some(found) = unsafe { buffered(w) }? { *event = found; return Ok(()); }
            // The descriptor does not block: what the kernel has queued is taken at once, whatever the timeout.
            let count = unsafe { libc::read((*w).fd, ptr::addr_of_mut!((*w).buffer).cast(), CAPACITY) };
            if count > 0 { unsafe { (*w).next = 0; (*w).filled = count as usize }; continue; }
            if count == 0 { return Err(Error::Os); }
            match errno() { libc::EINTR => continue, libc::EAGAIN => {} _ => return Err(Error::Os) }
            if timeout_ns == 0 { return Err(Error::Timeout); }
            let now = <Linux as port::Clock>::monotonic_ns()?;
            let remaining = if timeout_ns == FOREVER { FOREVER } else { deadline.get_or_insert(now.saturating_add(timeout_ns)).saturating_sub(now) };
            if remaining == 0 { return Err(Error::Timeout); }
            // Seconds are held to what every time_t can carry; a wait is never that long.
            let limit = libc::timespec { tv_sec: (remaining / 1_000_000_000).min(i32::MAX as u64) as _, tv_nsec: (remaining % 1_000_000_000) as _ };
            let mut slot = libc::pollfd { fd: unsafe { (*w).fd }, events: libc::POLLIN, revents: 0 };
            // An event, an interruption and the end of the wait all lead back to the read, which tells them apart.
            if unsafe { libc::ppoll(&mut slot, 1, if timeout_ns == FOREVER { ptr::null() } else { &limit }, ptr::null()) } < 0 && errno() != libc::EINTR { return Err(Error::Os); }
        }
    }
}
