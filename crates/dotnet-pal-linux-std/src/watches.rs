//! Changes to files and directories of the desktop port.
//!
//! Linux has inotify, and the provider is one instance per watcher with the
//! kernel's watch descriptor as the watch id. `add` and `remove` use the
//! descriptor only, so they run beside a reader; the kernel queues IN_IGNORED for
//! a removed watch, which is what wakes a reader that waits.
//!
//! Every other target gets a portable provider on `std` alone. One thread per
//! watcher looks at the watched nodes every `INTERVAL` (100 ms) and queues what
//! differs from the look before: entries that came and went, a size or
//! modification time that changed (`MODIFY`, for anything but a directory),
//! permissions or an owner that changed (`ATTRIBUTES`). On Unix a node is known by device, inode and birth
//! time, so an entry that went under one name and came under another, in the same
//! or another watched directory of the watcher, is a rename: `MOVED_FROM` and then
//! at once `MOVED_TO`, with a cookie of their own. A look lists one directory
//! after the other, so a rename can fall between two listings; a half without its
//! partner is looked for again at once, up to `PASSES` looks in a row. Where std
//! has no identity to offer (Windows) a rename is `DELETE` and `CREATE`. What this way of looking
//! cannot see is never reported: `ACCESS`, a change undone within one interval, a
//! second write within the granularity of the file system's modification time
//! that leaves the size alone. Events of one look come watch by watch (what
//! went, what came, what changed, the node itself), not in the order they
//! happened. A watch follows its path, not its node: when the path stops leading
//! to the node that was watched (deleted, renamed, replaced), the entries the
//! watch knew are reported as deleted and the watch ends with `REMOVED`. The
//! queue holds `LIMIT` (4096) events; beyond them one `OVERFLOW` stands for
//! whatever was dropped, as with inotify. The cost is one listing and one stat per
//! entry of every watched directory ten times a second. The Windows branches
//! compile but have not been executed here.
use super::{borrow, boxed, take, Std};
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::watches::Event;
use std::ffi::c_void;

impl port::Watches for Std {
    fn open() -> Result<*mut c_void> { imp::Watcher::open().map(boxed) }
    unsafe fn close(watcher: *mut c_void) -> Result<()> { unsafe { take::<imp::Watcher>(watcher) }?.close() }
    unsafe fn add(watcher: *mut c_void, path: &[u8], events: u32) -> Result<u32> { unsafe { borrow::<imp::Watcher>(watcher) }?.add(path, events) }
    unsafe fn remove(watcher: *mut c_void, watch: u32) -> Result<()> { unsafe { borrow::<imp::Watcher>(watcher) }?.remove(watch) }
    unsafe fn read(watcher: *mut c_void, timeout_ns: u64, event: &mut Event) -> Result<()> { unsafe { borrow::<imp::Watcher>(watcher) }?.read(timeout_ns, event) }
}
/// An event nobody can be told about (its name does not fit the contract's field) is a lost one.
const LOST: Event = Event { events: dotnet_pal_rs::watches::OVERFLOW, ..Event::EMPTY };


mod imp {
    use super::{Error, Event, Result, LOST};
    use dotnet_pal_rs::watches::{ACCESS, ATTRIBUTES, CREATE, DELETE, DIRECTORY, FOREVER, MODIFY, MOVED_FROM, MOVED_TO, NO_FOLLOW, ONLY_DIRECTORY, REMOVED};
    use std::{ffi::CString, fs, io, mem, os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd}, ptr, sync::Mutex, time::{Duration, Instant}};

    /// The events a consumer asks for, as the contract and as inotify name them.
    const KINDS: [(u32, u32); 7] = [(ACCESS, libc::IN_ACCESS), (MODIFY, libc::IN_MODIFY), (ATTRIBUTES, libc::IN_ATTRIB), (MOVED_FROM, libc::IN_MOVED_FROM),
        (MOVED_TO, libc::IN_MOVED_TO), (CREATE, libc::IN_CREATE), (DELETE, libc::IN_DELETE)];
    const HEADER: usize = mem::size_of::<libc::inotify_event>();
    /// What one kernel read returned and how much of it has been handed out. Only the reader touches it.
    struct Batch { bytes: Vec<u8>, next: usize, filled: usize }
    pub struct Watcher { descriptor: OwnedFd, batch: Mutex<Batch> }

    fn code() -> i32 { io::Error::last_os_error().raw_os_error().unwrap_or(0) }
    /// The contract's word for one kernel event; `None` for IN_UNMOUNT, which the IN_IGNORED behind it makes redundant.
    fn translate(wd: i32, mask: u32, cookie: u32, name: &[u8]) -> Option<Event> {
        if mask & libc::IN_Q_OVERFLOW != 0 { return Some(LOST); }
        let mut events = KINDS.iter().filter(|(_, bit)| mask & bit != 0).fold(0, |all, (kind, _)| all | kind);
        if mask & libc::IN_IGNORED != 0 { events |= REMOVED; }
        if events == 0 || wd <= 0 { return None; }
        if mask & libc::IN_ISDIR != 0 { events |= DIRECTORY; }
        Some(Event::new(wd as u32, events, cookie, name).unwrap_or(LOST))
    }
    impl Batch {
        fn next(&mut self) -> Result<Option<Event>> {
            while self.next < self.filled {
                let rest = &self.bytes[self.next..self.filled];
                if rest.len() < HEADER { self.next = self.filled; return Err(Error::Os); }
                // SAFETY: HEADER readable bytes were just checked; the read does not rely on their alignment.
                let header = unsafe { ptr::read_unaligned(rest.as_ptr().cast::<libc::inotify_event>()) };
                let Some(name) = rest.get(HEADER..HEADER + header.len as usize) else { self.next = self.filled; return Err(Error::Os); };
                self.next += HEADER + name.len();
                // The kernel pads the name with NULs up to the next event's alignment.
                let name = &name[..name.iter().position(|byte| *byte == 0).unwrap_or(name.len())];
                if let Some(event) = translate(header.wd, header.mask, header.cookie, name) { return Ok(Some(event)); }
            }
            Ok(None)
        }
    }
    impl Watcher {
        pub fn open() -> Result<Self> {
            let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
            if fd < 0 {
                return Err(match code() {
                    // One errno for a full descriptor table and for the user's limit of inotify instances. The second is the
                    // target's limit of watchers, and it is the one that still leaves room for a descriptor.
                    libc::EMFILE => if fs::File::open("/").is_ok() { Error::NoSpace } else { Error::TooManyHandles },
                    libc::ENFILE => Error::TooManyHandles,
                    libc::ENOMEM => Error::OutOfMemory,
                    _ => Error::Os,
                });
            }
            // SAFETY: the descriptor is new and nobody else's.
            Ok(Self { descriptor: unsafe { OwnedFd::from_raw_fd(fd) }, batch: Mutex::new(Batch { bytes: vec![0; 4096], next: 0, filled: 0 }) })
        }
        pub fn close(self) -> Result<()> {
            // close releases the descriptor even when it reports EINTR, so it is never retried.
            if unsafe { libc::close(self.descriptor.into_raw_fd()) } == 0 || code() == libc::EINTR { Ok(()) } else { Err(Error::Os) }
        }
        pub fn add(&self, path: &[u8], events: u32) -> Result<u32> {
            let path = CString::new(path).map_err(|_| Error::InvalidArgument)?;
            // Without IN_MASK_ADD a node the instance already watches keeps its descriptor and takes the new mask.
            let mut mask = KINDS.iter().filter(|(kind, _)| events & kind != 0).fold(libc::IN_EXCL_UNLINK, |all, (_, bit)| all | bit);
            if events & ONLY_DIRECTORY != 0 { mask |= libc::IN_ONLYDIR; }
            if events & NO_FOLLOW != 0 { mask |= libc::IN_DONT_FOLLOW; }
            let wd = unsafe { libc::inotify_add_watch(self.descriptor.as_raw_fd(), path.as_ptr(), mask) };
            if wd > 0 { return Ok(wd as u32); }
            Err(if wd == 0 { Error::Os } else {
                match code() {
                    libc::ENOENT => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, libc::ENOTDIR => Error::NotDirectory,
                    libc::ENAMETOOLONG => Error::NameTooLong, libc::ENOSPC => Error::NoSpace, libc::ENOMEM => Error::OutOfMemory, _ => Error::Os,
                }
            })
        }
        pub fn remove(&self, watch: u32) -> Result<()> {
            let Ok(wd) = i32::try_from(watch) else { return Err(Error::InvalidArgument); };
            // EINVAL is a descriptor this instance does not have: never given out, removed before, or ended with its node.
            if unsafe { libc::inotify_rm_watch(self.descriptor.as_raw_fd(), wd as _) } == 0 { Ok(()) } else if code() == libc::EINVAL { Err(Error::InvalidArgument) } else { Err(Error::Os) }
        }
        pub fn read(&self, timeout_ns: u64, event: &mut Event) -> Result<()> {
            // Uncontended by contract: one thread reads at a time. `add` and `remove` never take it.
            let mut batch = self.batch.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut deadline = None;
            loop {
                if let Some(found) = batch.next()? { *event = found; return Ok(()); }
                // The descriptor does not block: what the kernel has queued is taken at once, whatever the timeout.
                let count = unsafe { libc::read(self.descriptor.as_raw_fd(), batch.bytes.as_mut_ptr().cast(), batch.bytes.len()) };
                if count > 0 { (batch.next, batch.filled) = (0, count as usize); continue; }
                if count == 0 { return Err(Error::Os); }
                match code() { libc::EINTR => continue, libc::EAGAIN => {} _ => return Err(Error::Os) }
                if timeout_ns == 0 { return Err(Error::Timeout); }
                let limit = if timeout_ns == FOREVER { None } else {
                    let now = Instant::now();
                    // A deadline beyond what the clock can carry is no deadline.
                    match *deadline.get_or_insert_with(|| now.checked_add(Duration::from_nanos(timeout_ns))) {
                        Some(deadline) if deadline <= now => return Err(Error::Timeout),
                        Some(deadline) => Some(deadline - now),
                        None => None,
                    }
                };
                // Seconds are held to what every time_t can carry; a wait is never that long.
                let limit = limit.map(|left| libc::timespec { tv_sec: left.as_secs().min(i32::MAX as u64) as _, tv_nsec: left.subsec_nanos() as _ });
                let mut slot = libc::pollfd { fd: self.descriptor.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // An event, an interruption and the end of the wait all lead back to the read, which tells them apart.
                if unsafe { libc::ppoll(&mut slot, 1, limit.as_ref().map_or(ptr::null(), |limit| limit as *const libc::timespec), ptr::null()) } < 0 && code() != libc::EINTR { return Err(Error::Os); }
            }
        }
    }
}
