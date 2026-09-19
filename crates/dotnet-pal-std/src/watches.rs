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

#[cfg(any(target_os = "linux", target_os = "android"))]
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

#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod imp {
    use super::{Error, Event, Result, LOST};
    use dotnet_pal_rs::watches::{ACCESS, ATTRIBUTES, CREATE, DELETE, DIRECTORY, FOREVER, MODIFY, MOVED_FROM, MOVED_TO, NO_FOLLOW, ONLY_DIRECTORY, OVERFLOW, REMOVED};
    use std::{collections::{BTreeMap, HashMap, VecDeque}, ffi::{OsStr, OsString}, fs, io, path::{Path, PathBuf}, sync::{Arc, Condvar, Mutex, MutexGuard}, thread, time::{Duration, Instant, SystemTime}};

    /// How often the watched nodes are looked at.
    const INTERVAL: Duration = Duration::from_millis(100);
    /// The events kept for a reader that does not keep up.
    const LIMIT: usize = 4096;
    /// Looks taken in a row while entries keep coming or going without their other half.
    const PASSES: usize = 3;

    /// Device, inode and birth time. The inode alone is not enough: some file systems hand the number of a deleted
    /// file to the next one created, and that pair must not read as a rename.
    type Identity = (u64, u64, Option<SystemTime>);
    /// What one look at a node keeps of it.
    #[derive(Clone, PartialEq)]
    struct Meta { directory: bool, size: u64, modified: Option<SystemTime>, attributes: (u32, u32, u32), identity: Option<Identity> }
    impl Meta {
        #[cfg(unix)]
        fn new(meta: &fs::Metadata) -> Self {
            use std::os::unix::fs::MetadataExt;
            Self { directory: meta.is_dir(), size: meta.len(), modified: meta.modified().ok(), attributes: (meta.mode(), meta.uid(), meta.gid()),
                identity: Some((meta.dev(), meta.ino(), meta.created().ok())) }
        }
        #[cfg(not(unix))]
        fn new(meta: &fs::Metadata) -> Self {
            Self { directory: meta.is_dir(), size: meta.len(), modified: meta.modified().ok(), attributes: (meta.permissions().readonly() as u32, 0, 0), identity: None }
        }
        /// Whether a later look found the node this one found. Without an identity a node is its name and kind.
        fn same_node(&self, later: &Meta) -> bool { self.directory == later.directory && self.identity == later.identity }
        /// The kinds of change between two looks at one node, with the mark of a directory. A directory's size and time
        /// are about its entries, which have their own events.
        fn changes(&self, later: &Meta) -> u32 {
            let modified = !later.directory && (self.size != later.size || self.modified != later.modified);
            let kinds = (if modified { MODIFY } else { 0 }) | (if self.attributes != later.attributes { ATTRIBUTES } else { 0 });
            if kinds != 0 { kinds | later.mark() } else { 0 }
        }
        fn mark(&self) -> u32 { if self.directory { DIRECTORY } else { 0 } }
    }
    /// In the order of their names, so that the events of one look come in an order that does not change from run to run.
    type Entries = BTreeMap<OsString, Meta>;
    struct Watch { id: u32, path: PathBuf, follow: bool, events: u32, own: Meta, entries: Option<Entries> }
    /// One look at a watched node: the node, and its entries when it is a directory that could be listed.
    enum Look { Gone, Node(Meta, Option<Entries>) }
    struct State { queue: VecDeque<Event>, lost: bool, watches: Vec<Watch>, last_id: u32, last_cookie: u32, closing: bool }
    struct Shared { state: Mutex<State>, arrived: Condvar, tick: Condvar }
    pub struct Watcher { shared: Arc<Shared>, scanner: thread::JoinHandle<()> }

    fn error(e: io::Error) -> Error {
        use io::ErrorKind as Kind;
        match e.kind() {
            Kind::NotFound => Error::NotFound, Kind::PermissionDenied => Error::AccessDenied, Kind::NotADirectory => Error::NotDirectory,
            Kind::OutOfMemory => Error::OutOfMemory, Kind::InvalidInput => Error::InvalidArgument,
            // Unix reports an over-long name as such; Windows folds it into malformed names, where 206 is ERROR_FILENAME_EXCED_RANGE.
            Kind::InvalidFilename => if cfg!(unix) || e.raw_os_error() == Some(206) { Error::NameTooLong } else { Error::InvalidArgument },
            _ => Error::Os,
        }
    }
    #[cfg(unix)]
    fn native(path: &[u8]) -> Result<&Path> { Ok(Path::new(<OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path))) }
    #[cfg(not(unix))]
    fn native(path: &[u8]) -> Result<&Path> { std::str::from_utf8(path).map(Path::new).map_err(|_| Error::InvalidArgument) }
    #[cfg(unix)]
    fn bytes(name: &OsStr) -> Option<&[u8]> { Some(std::os::unix::ffi::OsStrExt::as_bytes(name)) }
    #[cfg(not(unix))]
    fn bytes(name: &OsStr) -> Option<&[u8]> { name.to_str().map(str::as_bytes) }
    fn stat(path: &Path, follow: bool) -> io::Result<fs::Metadata> { if follow { fs::metadata(path) } else { fs::symlink_metadata(path) } }
    /// The entries of a directory as they are now. One that goes away while it is being listed is not there.
    fn list(path: &Path) -> io::Result<Entries> {
        let mut entries = Entries::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if let Ok(meta) = entry.metadata() { entries.insert(entry.file_name(), Meta::new(&meta)); }
        }
        Ok(entries)
    }
    fn look(path: &Path, follow: bool, known: &Meta) -> Option<Look> {
        let own = match stat(path, follow) {
            Ok(meta) => Meta::new(&meta),
            Err(e) => return matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory).then_some(Look::Gone),
        };
        if !known.same_node(&own) { return Some(Look::Gone); }
        if !own.directory { return Some(Look::Node(own, None)); }
        match list(path) {
            Ok(entries) => Some(Look::Node(own, Some(entries))),
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => Some(Look::Gone),
            // A directory that cannot be listed just now (its permissions, the descriptor table) keeps what is known of it.
            Err(_) => Some(Look::Node(own, None)),
        }
    }
    fn lock(shared: &Shared) -> MutexGuard<'_, State> { shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
    fn wait<'a>(condition: &Condvar, state: MutexGuard<'a, State>, limit: Option<Duration>) -> MutexGuard<'a, State> {
        match limit {
            None => condition.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(limit) => condition.wait_timeout(state, limit).unwrap_or_else(|poisoned| poisoned.into_inner()).0,
        }
    }

    /// An entry that went; `to` is where its other half came (difference and entry), once that is known.
    struct Went { name: OsString, meta: Meta, to: Option<(usize, usize)> }
    struct Came { name: OsString, meta: Meta, renamed: bool }
    /// What one look found different in one watch.
    struct Difference { watch: u32, wanted: u32, went: Vec<Went>, came: Vec<Came>, changed: Vec<(OsString, u32)>, own: u32, ended: bool }
    impl State {
        fn push(&mut self, wanted: u32, watch: u32, events: u32, cookie: u32, name: &OsStr) {
            if events & (wanted | REMOVED) == 0 { return; }
            let event = bytes(name).and_then(|name| Event::new(watch, events, cookie, name)).unwrap_or(LOST);
            // A full queue and a name the event cannot carry both end in the one report that says events were lost.
            if event.events & OVERFLOW != 0 || self.queue.len() >= LIMIT {
                if !self.lost { self.lost = true; self.queue.push_back(LOST); }
            } else { self.queue.push_back(event); }
        }
        /// Holds one look against what the watches knew and makes it what they know. Nothing is reported yet.
        fn compare(&mut self, looks: Vec<(u32, Look)>, differences: &mut Vec<Difference>) {
            for (id, found) in looks {
                // A watch that was removed during the look has nothing more to say.
                let Some(index) = self.watches.iter().position(|watch| watch.id == id) else { continue; };
                let watch = &mut self.watches[index];
                let mut difference = Difference { watch: id, wanted: watch.events, went: Vec::new(), came: Vec::new(), changed: Vec::new(), own: 0, ended: false };
                let entries = match found {
                    // Whatever became of the entries, they are no longer to be found below this path.
                    Look::Gone => { difference.ended = true; Some(Entries::new()) }
                    Look::Node(own, entries) => { difference.own = watch.own.changes(&own); watch.own = own; entries }
                };
                if let (Some(before), Some(after)) = (watch.entries.as_mut(), entries) {
                    for (name, old) in before.iter() {
                        match after.get(name) {
                            Some(new) if old.same_node(new) => { let kinds = old.changes(new); if kinds != 0 { difference.changed.push((name.clone(), kinds)); } }
                            _ => difference.went.push(Went { name: name.clone(), meta: old.clone(), to: None }),
                        }
                    }
                    for (name, new) in after.iter() {
                        if !before.get(name).is_some_and(|old| old.same_node(new)) { difference.came.push(Came { name: name.clone(), meta: new.clone(), renamed: false }); }
                    }
                    *before = after;
                }
                // The watch ends here, so that no later look asks about it; its REMOVED follows its last events.
                if difference.ended { self.watches.remove(index); }
                differences.push(difference);
            }
        }
        /// A node that went under one name and came under another was renamed, wherever in the watcher's directories
        /// that was. Returns whether a half is still without its other half from `first` on.
        fn pair(differences: &mut [Difference], first: usize) -> bool {
            let mut arrivals: HashMap<(Identity, bool), Vec<(usize, usize)>> = HashMap::new();
            for (d, difference) in differences.iter().enumerate() {
                for (c, came) in difference.came.iter().enumerate() {
                    if let (Some(identity), false) = (came.meta.identity, came.renamed) { arrivals.entry((identity, came.meta.directory)).or_default().push((d, c)); }
                }
            }
            for d in 0..differences.len() {
                for g in 0..differences[d].went.len() {
                    let went = &differences[d].went[g];
                    if went.to.is_some() { continue; }
                    let Some((to, c)) = went.meta.identity.and_then(|identity| arrivals.get_mut(&(identity, went.meta.directory))?.pop()) else { continue; };
                    differences[d].went[g].to = Some((to, c));
                    differences[to].came[c].renamed = true;
                }
            }
            differences[first..].iter().any(|difference| difference.went.iter().any(|went| went.meta.identity.is_some() && went.to.is_none())
                || difference.came.iter().any(|came| came.meta.identity.is_some() && !came.renamed))
        }
        fn report(&mut self, differences: &[Difference]) {
            // A watch the consumer removed between the looks has had its REMOVED: nothing may follow that.
            let wanted = |state: &Self, difference: &Difference| if difference.ended { difference.wanted } else { state.watches.iter().find(|watch| watch.id == difference.watch).map_or(0, |watch| watch.events) };
            for difference in differences {
                let own = wanted(self, difference);
                for went in &difference.went {
                    match went.to {
                        Some((to, c)) => {
                            // The second half follows the first at once, as a consumer that pairs them expects.
                            self.last_cookie = if self.last_cookie == u32::MAX { 1 } else { self.last_cookie + 1 };
                            let (target, came) = (&differences[to], &differences[to].came[c]);
                            let theirs = wanted(self, target);
                            self.push(own, difference.watch, MOVED_FROM | went.meta.mark(), self.last_cookie, &went.name);
                            self.push(theirs, target.watch, MOVED_TO | came.meta.mark(), self.last_cookie, &came.name);
                        }
                        None => self.push(own, difference.watch, DELETE | went.meta.mark(), 0, &went.name),
                    }
                }
                for came in &difference.came { if !came.renamed { self.push(own, difference.watch, CREATE | came.meta.mark(), 0, &came.name); } }
                // What happened to the watched node itself has no name.
                for (name, kinds) in difference.changed.iter().map(|(name, kinds)| (name.as_os_str(), *kinds)).chain([(OsStr::new(""), difference.own)]) {
                    for kind in [MODIFY, ATTRIBUTES] { if kinds & kind != 0 { self.push(own, difference.watch, kind | (kinds & DIRECTORY), 0, name); } }
                }
                if difference.ended { self.push(0, difference.watch, REMOVED, 0, OsStr::new("")); }
            }
        }
    }
    fn scan(shared: &Shared) {
        let mut state = lock(shared);
        loop {
            if state.closing { return; }
            // With nothing to look at the thread sleeps until a watch arrives or the watcher closes.
            let pause = (!state.watches.is_empty()).then_some(INTERVAL);
            state = wait(&shared.tick, state, pause);
            let mut differences = Vec::new();
            // A look lists one directory after the other, so a rename can fall between the listing that would show
            // one half and the listing that would show the other. A half without its partner is therefore looked for
            // again at once: the rename is over by then, and the next look shows the rest of it.
            for _ in 0..PASSES {
                if state.closing { return; }
                let targets: Vec<(u32, PathBuf, bool, Meta)> = state.watches.iter().map(|watch| (watch.id, watch.path.clone(), watch.follow, watch.own.clone())).collect();
                // The file system is read without the lock: `add`, `remove` and `read` never wait for a directory listing.
                drop(state);
                let looks: Vec<(u32, Look)> = targets.into_iter().filter_map(|(id, path, follow, known)| Some((id, look(&path, follow, &known)?))).collect();
                state = lock(shared);
                let first = differences.len();
                state.compare(looks, &mut differences);
                if !State::pair(&mut differences, first) { break; }
            }
            state.report(&differences);
            if !state.queue.is_empty() { shared.arrived.notify_all(); }
        }
    }
    impl Watcher {
        pub fn open() -> Result<Self> {
            let shared = Arc::new(Shared { state: Mutex::new(State { queue: VecDeque::new(), lost: false, watches: Vec::new(), last_id: 0, last_cookie: 0, closing: false }),
                arrived: Condvar::new(), tick: Condvar::new() });
            let scanner = thread::Builder::new().name("pal-watches".into()).spawn({ let shared = shared.clone(); move || scan(&shared) });
            // The OS's limit of threads is this provider's limit of watchers.
            let scanner = scanner.map_err(|e| if e.kind() == io::ErrorKind::WouldBlock { Error::NoSpace } else { error(e) })?;
            Ok(Self { shared, scanner })
        }
        pub fn close(self) -> Result<()> {
            lock(&self.shared).closing = true;
            self.shared.tick.notify_all();
            self.scanner.join().map_err(|_| Error::Os)
        }
        pub fn add(&self, path: &[u8], events: u32) -> Result<u32> {
            // Snapshots show no reads: a watch that asks for nothing else would stay silent for ever.
            if events & !(ONLY_DIRECTORY | NO_FOLLOW) == ACCESS { return Err(Error::Unsupported); }
            // A relative path means what it means now, wherever the process goes afterwards.
            let path = std::path::absolute(native(path)?).map_err(error)?;
            let follow = events & NO_FOLLOW == 0;
            let meta = stat(&path, follow).map_err(error)?;
            if events & ONLY_DIRECTORY != 0 && !meta.is_dir() { return Err(Error::NotDirectory); }
            // The first look is taken here, so whatever happens once `add` has returned is a change. As with inotify,
            // a node that cannot be read cannot be watched.
            let entries = if meta.is_dir() { Some(list(&path).map_err(error)?) } else { if meta.is_file() { fs::File::open(&path).map_err(error)?; } None };
            let (own, wanted) = (Meta::new(&meta), events & !(ONLY_DIRECTORY | NO_FOLLOW));
            // Without an identity the same node is the same path, links resolved as far as the watch follows them.
            let key = |path: &Path, follow: bool| if follow { fs::canonicalize(path).ok() } else { Some(path.parent().and_then(|parent| fs::canonicalize(parent).ok())?.join(path.file_name()?)) };
            let mut state = lock(&self.shared);
            let known = state.watches.iter_mut().find(|watch| match own.identity {
                Some(_) => watch.own.same_node(&own),
                None => watch.follow == follow && key(&watch.path, follow).is_some_and(|known| Some(known) == key(&path, follow)),
            });
            if let Some(watch) = known { watch.events = wanted; return Ok(watch.id); }
            if state.last_id == u32::MAX { return Err(Error::NoSpace); }
            state.last_id += 1;
            let id = state.last_id;
            state.watches.push(Watch { id, path, follow, events: wanted, own, entries });
            self.shared.tick.notify_all();
            Ok(id)
        }
        pub fn remove(&self, watch: u32) -> Result<()> {
            let mut state = lock(&self.shared);
            let Some(index) = state.watches.iter().position(|known| known.id == watch) else { return Err(Error::InvalidArgument); };
            state.watches.remove(index);
            state.push(0, watch, REMOVED, 0, OsStr::new(""));
            self.shared.arrived.notify_all();
            Ok(())
        }
        pub fn read(&self, timeout_ns: u64, event: &mut Event) -> Result<()> {
            // A deadline beyond what the clock can carry is no deadline.
            let deadline = if timeout_ns == FOREVER { None } else { Instant::now().checked_add(Duration::from_nanos(timeout_ns)) };
            let mut state = lock(&self.shared);
            loop {
                if let Some(next) = state.queue.pop_front() {
                    if next.events & OVERFLOW != 0 { state.lost = false; }
                    *event = next;
                    return Ok(());
                }
                let left = match deadline { Some(deadline) => Some(deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()).ok_or(Error::Timeout)?), None => None };
                state = wait(&self.shared.arrived, state, left);
            }
        }
    }
}
