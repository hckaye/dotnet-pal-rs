//! In-memory file system for `dotnet-pal-rs` ports that have no storage hardware.
//!
//! [`MemFs`] implements [`dotnet_pal_rs::port::Files`] over a tree kept on the heap.
//! Paths are POSIX-like: `/`-separated and resolved from a root `/` that always
//! exists, or from the working directory, which is the root until
//! `set_current_directory` names another. Repeated slashes and `.` are skipped,
//! `..` must pass through an existing directory and stays at the root, and a
//! trailing slash names a directory. A file stays readable and writable through its
//! open handles after it is removed or replaced by a rename. A directory handle
//! enumerates a name-sorted snapshot taken when it was opened.
//!
//! Hard links and symbolic links behave as on Linux. A file is freed when its last
//! name and its last handle are gone. The target of a symbolic link is stored as
//! given, may dangle, and resolves against the directory holding the link. Links are
//! followed before the final name of every path, and in the final name by the calls
//! that follow there on Linux ([`Files::open`] without `EXCLUSIVE`, `set_mode`,
//! `open_directory`, `set_current_directory`, `real_path`, and `path_status` and
//! `set_times` when asked to); the calls that create, remove or rename a name work on
//! the link itself. A walk gives up after 40 links with `Os`, because the boundary has
//! no status for a link loop.
//!
//! Locks are advisory and held by open handles: `lock` on the whole file with the
//! rules of flock(2), `lock_range` on bytes with the rules of open file description
//! locks. The two kinds do not see each other. A whole-file lock asks for no access;
//! a shared range needs a handle opened with `READ` and an exclusive one a handle
//! opened with `WRITE`, as the contract of the group says.
//!
//! [`MemFs`] also implements [`dotnet_pal_rs::port::Volumes`]: the tree is one volume
//! mounted at `/`, with the capacity [`set_capacity`] gave it and the format name
//! `memfs`.
//!
//! [`MemFs`] implements [`dotnet_pal_rs::port::Watches`] without looking at the tree: the
//! call that changes something queues the events itself, so a reader gets every event,
//! in the order things happened, as soon as the call has returned. A watcher keeps up
//! to 1024 events; what comes beyond them is dropped and one `OVERFLOW` says so. A
//! watch is of a node, not of a path: it follows a directory that is renamed, watching
//! the node again returns the same id with the new kinds, and ids count up from 1 and
//! are not used twice by one watcher. A watched directory tells of its entries by
//! name, with the mark `DIRECTORY` on those that are directories, and not of what
//! happens below them; a watched node tells of itself with an empty name, after the
//! directories that hold it. What one call reports, and in which order, is what
//! inotify reports on Linux: `CREATE` for a new file, directory, symbolic link or hard
//! link, `MODIFY` for a write of at least one byte and for a new size, `ACCESS` for a
//! read of at least one byte, `ATTRIBUTES` for a new mode or new times, `DELETE` for a
//! removed entry after the events of the node that lost the name, `MOVED_FROM` and
//! `MOVED_TO` with one cookie for a rename, and for what the rename replaced no
//! `DELETE`, only the events of the replaced node itself. `REMOVED` ends
//! the watch of a directory that is removed, of any other node that loses its last
//! name, and of a watch that `remove` ends. The differences to Linux: a handle has no
//! name here, so a file with several names reports under each of them, where inotify
//! reports under the one it was opened by; a change of the link count is `ATTRIBUTES`
//! for the node's own watch as on Linux and also under every name that stays;
//! `set_times` is `ATTRIBUTES` whichever time it sets, where Linux says `ACCESS` or
//! `MODIFY` when only one is set; `set_size` to the size the file has and a truncating
//! `open` of an empty file report nothing, where Linux says `MODIFY`; and the watch of
//! a file ends with its last name even while handles keep the file, where inotify
//! waits for the last of them to close. Writes through a mapping report nothing, as on
//! Linux.
//!
//! A read that finds no event has nothing to block on in `no_std`. It lets the tree's
//! lock go and calls the function [`set_wait`] was given with the time it may stay
//! away, 10 ms at most, then looks again; `add`, `remove` and the calls that cause
//! events run in between. It takes what it asked for off its timeout, so it needs no
//! clock, and a wait function that returns early ends the timeout early. Without a
//! wait function the read calls [`set_yield`]'s function between two looks and asks
//! [`set_clock`]'s clock whether its time is up; that clock is the one of the
//! timestamps, so the timeout moves with it when it is set. With neither a wait
//! function nor a clock a timed read does not wait: it takes what is queued or answers
//! `Timeout`. A read without limit needs no clock in any case.
//!
//! [`MemFs`] implements [`dotnet_pal_rs::port::Mappings`] without an MMU. A shared
//! mapping is a pointer into the file's own bytes, so reads, writes and every other
//! shared mapping of the file see the same bytes at once, and `sync` has nothing to
//! do. For that the file moves into whole 4096-byte pages when it is first mapped,
//! and those pages stay where they are while a shared mapping exists. The limits that
//! follow: a range must end at or before the end of the page that holds the last byte
//! of the file (`InvalidArgument` beyond it, an empty file included, because there is
//! no fault to raise for a page past the end, where a POSIX target grants the
//! mapping); while a file is mapped shared it grows to the end of the pages it was
//! pinned with and no further (`NoSpace`, where on an OS the file could grow), and
//! after the last `unmap` it grows freely again; `access` is checked against the
//! handle but not enforced on the memory, so a write through a mapping made for
//! reading goes through; `EXECUTE` is `Unsupported`. The bytes between the end of the
//! file and the end of its last page read as zero, and what a consumer writes there is
//! lost when the file grows over them; a file that shrinks keeps its pages and zeroes
//! what was cut off. Two shared mappings of one range have one address, and each is
//! unmapped on its own. A private mapping is a page-aligned copy of the range as it
//! was when `map` was called, padded with zeros; it does not hold the file and does
//! not count against [`set_capacity`]. A shared mapping holds its file as an open
//! handle does: the file may be removed and the handle closed, and its bytes stay
//! counted in [`used_bytes`] until the last `unmap`.
//!
//! Reads do not move the access time, as on a file system mounted `noatime`: the access
//! time of a node is its creation time until `set_times` stores another. The working directory
//! may be renamed, and its path is then reported from where it is now. Once it is
//! removed, relative paths and `current_directory` are `NotFound` until
//! `set_current_directory` names a directory again; Linux still resolves `.` there.
//!
//! What it is not: nothing persists across a restart; `mode` is stored, changed and
//! reported but never enforced as permissions (only the `READ`/`WRITE` access of a
//! handle is); and there is one instance per program, because provider methods are
//! static.
//!
//! ```ignore
//! use dotnet_pal_memfs::MemFs;
//! dotnet_pal_rs::define_pal! { Files = MemFs, Volumes = MemFs, Watches = MemFs, Mappings = MemFs, /* other providers */ }
//! dotnet_pal_memfs::set_clock(unix_time_ns); // optional: every timestamp is 0 without it
//! dotnet_pal_memfs::set_yield(scheduler_yield); // optional: what a waiting `lock` does between attempts
//! dotnet_pal_memfs::set_wait(sleep_ns); // optional: what a watcher's `read` does while it has no event
//! // The final image supplies the `#[global_allocator]` this crate allocates from.
//! ```
//!
//! One spin lock guards the tree, the watchers and the mappings. It is held across the
//! copy of a transfer, across the queuing of the events a call causes and across
//! allocator calls, never across the clock, the yield of a waiting `lock`, the wait of
//! a reading watcher or anything that can block. It is not reentrant: an interrupt or
//! fault handler must not call the provider, and neither may the three functions a
//! port sets.
//! File content and link targets grow with fallible reservations, so an exhausted
//! allocator is `NoSpace` like an exceeded [`set_capacity`]; a directory snapshot, a
//! lock record, a watch or the pages of a mapping that do not fit are `OutOfMemory`,
//! and an event that does not fit is a lost one, which `OVERFLOW` reports. Nodes,
//! names, handles and the records of mappings are small and use ordinary allocation,
//! which ends in the image's allocation-error path when it fails.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
extern crate alloc;
use alloc::{alloc::{alloc_zeroed, dealloc, Layout}, boxed::Box, collections::{BTreeMap, VecDeque}, vec::Vec};
use core::{cell::UnsafeCell, ffi::c_void, hint, mem, ops::{Deref, DerefMut}, ptr::{self, NonNull}, slice, sync::atomic::{AtomicBool, AtomicPtr, Ordering}};
use dotnet_pal_rs::files::{Status, CREATE, EXCLUSIVE, LOCK_EXCLUSIVE, LOCK_UNLOCK, MAX_ENTRY_NAME, NODE_DIRECTORY, NODE_FILE, NODE_SYMLINK, READ, TRUNCATE, WRITE};
use dotnet_pal_rs::port::{Error, Files, Mappings, Result, Volumes, Watches};
use dotnet_pal_rs::runtime::{self, MAX_NAME};
use dotnet_pal_rs::watches::{self, Event};

/// The in-memory `Files`, `Watches`, `Mappings` and `Volumes` provider.
pub struct MemFs;

const ROOT: u64 = 1;
const MODE_MASK: u32 = 0o7777;
/// Links one walk follows, the bound of Linux.
const MAX_LINKS: usize = 40;
const DEFAULT_CAPACITY: u64 = 64 * 1024 * 1024;
/// The unit of a mapping's offset, and of the memory a mapped file and a private mapping live in.
const PAGE: usize = 4096;
/// Events a watcher keeps for a reader that does not keep up. One `OVERFLOW` stands for whatever came beyond them, as with inotify.
const QUEUE_LIMIT: usize = 1024;
/// The longest a read lets other threads run before it looks at its queue again.
const WAIT_SLICE: u64 = 10_000_000;
/// The kinds of event a watch can ask for.
const REQUESTABLE: u32 = watches::ACCESS | watches::MODIFY | watches::ATTRIBUTES | watches::MOVED_FROM | watches::MOVED_TO | watches::CREATE | watches::DELETE;

static CLOCK: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
/// Sets the source of the Unix-epoch nanosecond timestamps stored in nodes.
/// Without one every timestamp is 0, which the boundary reads as "none kept".
pub fn set_clock(now_ns: fn() -> u64) { CLOCK.store(now_ns as *mut (), Ordering::Release); }
fn has_clock() -> bool { !CLOCK.load(Ordering::Acquire).is_null() }
fn clock() -> u64 {
    let now_ns = CLOCK.load(Ordering::Acquire);
    if now_ns.is_null() { return 0; }
    // SAFETY: `set_clock` is the only writer and stores a `fn() -> u64`.
    unsafe { mem::transmute::<*mut (), fn() -> u64>(now_ns)() }
}
static YIELD: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
/// Sets what a [`Files::lock`] that waits calls between its attempts, never with the
/// tree's lock held. Until set it is a spin-loop hint, which lets the holder release
/// the file lock only where threads run in parallel or are preempted. A cooperative
/// or single-core port passes its scheduler's yield here, or never waits.
pub fn set_yield(yield_now: fn()) { YIELD.store(yield_now as *mut (), Ordering::Release); }
fn pause() {
    let yield_now = YIELD.load(Ordering::Acquire);
    if yield_now.is_null() { return hint::spin_loop(); }
    // SAFETY: `set_yield` is the only writer and stores a `fn()`.
    unsafe { mem::transmute::<*mut (), fn()>(yield_now)() }
}
static WAIT: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
/// Sets what a [`Watches::read`] that finds no event calls to let other threads run for up to the given number of
/// nanoseconds, never with the tree's lock held: a sleep, or an idle until the next timer interrupt. The read looks
/// at its queue again after every call, asks for at most 10 ms at a time, and takes what it asked for off its
/// timeout, so it needs no clock; a function that returns early makes the timeout end early. Until set, a read waits
/// with [`set_yield`]'s function and measures with [`set_clock`]'s clock, and without a clock a timed read does not
/// wait at all.
pub fn set_wait(wait: fn(u64)) { WAIT.store(wait as *mut (), Ordering::Release); }
fn wait_hook() -> Option<fn(u64)> {
    let wait = WAIT.load(Ordering::Acquire);
    // SAFETY: `set_wait` is the only writer and stores a `fn(u64)`.
    (!wait.is_null()).then(|| unsafe { mem::transmute::<*mut (), fn(u64)>(wait) })
}
/// Bounds the total bytes of file content (64 MiB until set); growth past the bound
/// is `NoSpace`. Names, nodes, link targets and allocator slack are not counted. A
/// bound below the bytes in use frees nothing and refuses further growth.
pub fn set_capacity(bytes: u64) { Guard::acquire().capacity = bytes; }
/// Bytes of file content held now, including removed files that are still open or mapped.
pub fn used_bytes() -> u64 { Guard::acquire().used }

/// `size` zeroed bytes on a page boundary; `size` is a positive multiple of `PAGE`.
fn pages(size: usize) -> Result<NonNull<u8>> {
    let layout = Layout::from_size_align(size, PAGE).map_err(|_| Error::OutOfMemory)?;
    // SAFETY: the layout has a size above zero.
    NonNull::new(unsafe { alloc_zeroed(layout) }).ok_or(Error::OutOfMemory)
}
/// Gives back what `pages(size)` returned.
unsafe fn free_pages(base: *mut u8, size: usize) {
    // SAFETY: `pages` made this layout, so it is valid, and the caller's contract is that the block came from it.
    unsafe { dealloc(base, Layout::from_size_align_unchecked(size, PAGE)) }
}
/// The pages a file lives in once it has been mapped shared: `capacity` bytes that stay where they are, of which the
/// first `size` are the file and the rest are kept zero by every change of the size. A mapping is a pointer into
/// them, so the provider reaches them through raw pointers only and never borrows them.
struct Pinned { base: NonNull<u8>, size: usize, capacity: usize }
impl Pinned {
    /// Makes the file `size` bytes, which fit. The bytes that leave it are zeroed, and so are the bytes that enter
    /// it: a consumer may have written behind the end through a mapping, and the contract has that lost.
    fn set_size(&mut self, size: usize) {
        let (from, to) = (self.size.min(size), self.size.max(size));
        // SAFETY: both ends lie inside the pages.
        unsafe { ptr::write_bytes(self.base.as_ptr().add(from), 0, to - from) };
        self.size = size;
    }
}
impl Drop for Pinned { fn drop(&mut self) { unsafe { free_pages(self.base.as_ptr(), self.capacity) } } }
/// The bytes of a file: a `Vec` until the file is first mapped shared, pinned pages from then on until a change of
/// size that no mapping is in the way of moves them back.
enum Content { Plain(Vec<u8>), Pinned(Pinned) }
impl Content {
    fn len(&self) -> usize { match self { Self::Plain(bytes) => bytes.len(), Self::Pinned(pinned) => pinned.size } }
    /// Copies what the file holds from `offset` on, `capacity` bytes at most.
    unsafe fn read(&self, offset: u64, out: *mut u8, capacity: usize) -> usize {
        let Some(start) = usize::try_from(offset).ok().filter(|start| *start < self.len()) else { return 0; };
        let count = (self.len() - start).min(capacity);
        match self {
            Self::Plain(bytes) => unsafe { ptr::copy_nonoverlapping(bytes.as_ptr().add(start), out, count) },
            // `out` may lie in a mapping of these very bytes.
            Self::Pinned(pinned) => unsafe { ptr::copy(pinned.base.as_ptr().add(start), out, count) },
        }
        count
    }
    /// Moves the bytes into `capacity` bytes of pages unless they are pinned already, and tells where they are.
    fn pin(&mut self, capacity: usize) -> Result<NonNull<u8>> {
        if let Self::Plain(bytes) = self {
            let (base, size) = (pages(capacity)?, bytes.len());
            // SAFETY: the pages are new and at least as long as the file.
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), base.as_ptr(), size) };
            *self = Self::Pinned(Pinned { base, size, capacity });
        }
        match self { Self::Pinned(pinned) => Ok(pinned.base), Self::Plain(_) => Err(Error::Os) }
    }
    /// Moves pinned bytes back into a `Vec` with room for `wanted` bytes, keeping no more than that.
    fn unpin(&mut self, wanted: usize) -> Result<()> {
        let Self::Pinned(pinned) = self else { return Ok(()); };
        let (mut bytes, kept) = (Vec::new(), pinned.size.min(wanted));
        bytes.try_reserve_exact(wanted).map_err(|_| Error::NoSpace)?;
        // SAFETY: `kept` bytes are readable in the pages and fit the reservation.
        unsafe { ptr::copy_nonoverlapping(pinned.base.as_ptr(), bytes.as_mut_ptr(), kept); bytes.set_len(kept); }
        *self = Self::Plain(bytes);
        Ok(())
    }
    /// Gets pinned bytes ready to become `wanted` bytes. While the file is `mapped` they stay where they are, and
    /// what their pages cannot hold is `NoSpace`. Otherwise they go back into a `Vec` when they outgrow the pages
    /// or leave half of them unused.
    fn prepare(&mut self, wanted: usize, room: u64, mapped: bool) -> Result<()> {
        let Self::Pinned(pinned) = self else { return Ok(()); };
        if wanted.saturating_sub(pinned.size) as u64 > room { return Err(Error::NoSpace); }
        let (outgrown, wasteful) = (wanted > pinned.capacity, wanted <= pinned.capacity / 2);
        if mapped { return if outgrown { Err(Error::NoSpace) } else { Ok(()) }; }
        if !outgrown && !wasteful { return Ok(()); }
        // Pages that still fit stay in use when there is no memory to move out of them.
        match self.unpin(wanted) { Err(refused) if outgrown => Err(refused), _ => Ok(()) }
    }
    fn resize(&mut self, size: u64, room: u64, mapped: bool) -> Result<()> {
        let wanted = usize::try_from(size).map_err(|_| Error::NoSpace)?;
        self.prepare(wanted, room, mapped)?;
        match self { Self::Plain(bytes) => resize(bytes, size, room), Self::Pinned(pinned) => { pinned.set_size(wanted); Ok(()) } }
    }
    /// A write that does not fit is refused whole.
    unsafe fn write(&mut self, offset: u64, data: *const u8, size: usize, room: u64, mapped: bool) -> Result<usize> {
        let end = offset.checked_add(size as u64).and_then(|end| usize::try_from(end).ok()).ok_or(Error::NoSpace)?;
        self.prepare(end.max(self.len()), room, mapped)?;
        match self {
            Self::Plain(bytes) => write(bytes, offset, unsafe { slice::from_raw_parts(data, size) }, room),
            Self::Pinned(pinned) => {
                if end > pinned.size { pinned.set_size(end); }
                // SAFETY: `prepare` found the range inside the pages. `data` may lie in a mapping of these very bytes.
                unsafe { ptr::copy(data, pinned.base.as_ptr().add(end - size), size) };
                Ok(size)
            }
        }
    }
}

type Entries = BTreeMap<Box<[u8]>, u64>;
/// `Link` holds the target text of a symbolic link.
enum Body { File(Content), Directory { parent: u64, entries: Entries }, Link(Vec<u8>) }
/// An advisory lock the handle at address `owner` holds on bytes `start..end`. A lock
/// on the `whole` file spans every byte and is blind to range locks, as they are to it:
/// the locks of flock(2) and fcntl(2) do not see each other on Linux either.
#[derive(Clone, Copy)]
struct Lock { owner: usize, whole: bool, start: u64, end: u64, exclusive: bool }
struct Node {
    mode: u32,
    created: u64, accessed: u64, modified: u64, changed: u64,
    /// Directory entries that name the node. With none it lives as long as a handle does.
    links: usize,
    opens: usize,
    /// Shared mappings that point into the content. They hold the node as handles do.
    maps: usize,
    locks: Vec<Lock>,
    body: Body,
}
/// One watched node of a watcher, with the kinds of event its consumer asked for.
#[derive(Clone, Copy)]
struct Watch { id: u32, node: u64, events: u32 }
/// The events of a watcher that wait for its reader, each as watch, kinds, cookie, name length and name. An event
/// is queued by the call that causes it, under the tree's lock, so nothing here may fail for good: the bytes grow
/// by fallible reservation, and an event without room is a lost one like an event beyond `QUEUE_LIMIT`. Every
/// reservation leaves room for the `OVERFLOW` record that says so.
struct Queue { bytes: VecDeque<u8>, count: usize, lost: bool }
impl Queue {
    const HEADER: usize = 13;
    fn new() -> Result<Self> {
        let mut bytes = VecDeque::new();
        bytes.try_reserve(Self::HEADER).map_err(|_| Error::OutOfMemory)?;
        Ok(Self { bytes, count: 0, lost: false })
    }
    fn push(&mut self, watch: u32, events: u32, cookie: u32, name: &[u8]) {
        if self.count >= QUEUE_LIMIT || self.bytes.try_reserve(2 * Self::HEADER + name.len()).is_err() {
            if !mem::replace(&mut self.lost, true) { self.record(0, watches::OVERFLOW, 0, b""); }
        } else {
            self.record(watch, events, cookie, name);
        }
    }
    fn record(&mut self, watch: u32, events: u32, cookie: u32, name: &[u8]) {
        for word in [watch, events, cookie] { self.bytes.extend(word.to_ne_bytes()); }
        self.bytes.push_back(name.len() as u8);
        self.bytes.extend(name);
        self.count += 1;
    }
    fn pop(&mut self) -> Option<Event> {
        if self.count == 0 { return None; }
        let mut words = [0u32; 3];
        for word in &mut words {
            let mut bytes = [0; 4];
            for (byte, queued) in bytes.iter_mut().zip(self.bytes.drain(..4)) { *byte = queued; }
            *word = u32::from_ne_bytes(bytes);
        }
        let length = self.bytes.pop_front()? as usize;
        let mut name = [0; MAX_ENTRY_NAME];
        for (byte, queued) in name.iter_mut().zip(self.bytes.drain(..length)) { *byte = queued; }
        self.count -= 1;
        if words[1] & watches::OVERFLOW != 0 { self.lost = false; }
        // A queue that ran full gives its memory back once it is read empty.
        if self.count == 0 && self.bytes.capacity() > PAGE {
            let mut fresh = VecDeque::new();
            if fresh.try_reserve(Self::HEADER).is_ok() { self.bytes = fresh; }
        }
        // A name the event cannot carry is an event the consumer did not get, and that is what an overflow says.
        Some(Event::new(words[0], words[1], words[2], &name[..length]).unwrap_or(Event { events: watches::OVERFLOW, ..Event::EMPTY }))
    }
}
/// A watch is known by its node, so it follows a directory that is renamed, as with inotify. `last` is the id
/// given out last: ids count up and are not used twice.
struct Watcher { queue: Queue, watches: Vec<Watch>, last: u32 }
/// What `unmap` and `sync` know a range by: a window into the pinned content of a file, as many times as `map`
/// returned it, or the copy a private mapping is.
enum Mapped { Shared { node: u64, count: usize }, Private }
/// Nodes are keyed by their identity, which is never reused. `working` is the working
/// directory, which stops being a node when it is removed. Mappings are keyed by their
/// address and length.
struct State {
    nodes: BTreeMap<u64, Node>, next_id: u64, used: u64, capacity: u64, working: u64,
    watchers: BTreeMap<u64, Watcher>, next_watcher: u64, cookie: u32, mappings: BTreeMap<(usize, usize), Mapped>,
}
/// A copy of an entry name: the text a walk finds it in may be a link target inside the tree.
struct Name { length: u8, bytes: [u8; MAX_ENTRY_NAME] }
impl Name {
    fn new(name: &[u8]) -> Self {
        let mut bytes = [0; MAX_ENTRY_NAME];
        bytes[..name.len()].copy_from_slice(name);
        Self { length: name.len() as u8, bytes }
    }
}
impl Deref for Name {
    type Target = [u8];
    fn deref(&self) -> &[u8] { &self.bytes[..self.length as usize] }
}
/// Where a path leads: the directory holding its final name, and the node of that
/// name if there is one. `name` is empty when the path ends at a directory reached
/// without a name of its own (`/`, a final `.` or `..`); `node` is then that directory.
struct Place { parent: u64, name: Name, node: Option<u64>, slash: bool }
/// What a walk does with a symbolic link in the final name of its path. The rules are
/// those of Linux, where a slash after the link asks a lookup for the directory behind
/// it, but not a call that removes, replaces or creates the name.
#[derive(Clone, Copy, PartialEq)]
enum Last {
    /// Resolves it, like every link before the final name.
    Follow,
    /// Keeps it, unless a slash comes after it.
    Keep,
    /// Keeps it as the entry to remove or replace; a slash after it is `NotDirectory`.
    Entry,
    /// Keeps it as what is in the way of the name to create, slash or not.
    Create,
}
/// The texts a walk still has to resolve, the innermost last: the rest of the path,
/// then the rest of the target of each link entered on the way.
struct Pending<'t> { texts: [&'t [u8]; MAX_LINKS + 1], depth: usize, entered: usize, slash: bool }
impl<'t> Pending<'t> {
    /// The next component. Once there is none, `slash` tells whether one ended the walk.
    fn next(&mut self) -> Option<&'t [u8]> {
        while let Some(text) = self.texts[..self.depth].last_mut() {
            let whole: &'t [u8] = text;
            let start = whole.iter().position(|byte| *byte != b'/').unwrap_or(whole.len());
            self.slash |= start != 0;
            let rest = &whole[start..];
            if rest.is_empty() { self.depth -= 1; continue; }
            let end = rest.iter().position(|byte| *byte == b'/').unwrap_or(rest.len());
            *text = &rest[end..];
            self.slash = false;
            return Some(&rest[..end]);
        }
        None
    }
    /// Whether the link just found in a name is to be entered.
    fn follows(&self, last: Last) -> bool {
        let mut left = self.texts[..self.depth].iter().flat_map(|text| text.iter());
        match last { Last::Follow => true, Last::Keep => left.next().is_some(), Last::Entry | Last::Create => left.any(|byte| *byte != b'/') }
    }
    fn enter(&mut self, target: &'t [u8]) -> Result<()> {
        // The boundary has no status for a link loop: the walk that gives up is an `Os` failure.
        if self.entered == MAX_LINKS { return Err(Error::Os); }
        self.texts[self.depth] = target;
        (self.depth, self.entered) = (self.depth + 1, self.entered + 1);
        Ok(())
    }
}

struct Shared { locked: AtomicBool, state: UnsafeCell<State> }
// SAFETY: the state is reached only through `Guard`, which holds the lock.
unsafe impl Sync for Shared {}
static SHARED: Shared = Shared {
    locked: AtomicBool::new(false),
    state: UnsafeCell::new(State { nodes: BTreeMap::new(), next_id: ROOT + 1, used: 0, capacity: DEFAULT_CAPACITY, working: ROOT,
        watchers: BTreeMap::new(), next_watcher: 1, cookie: 0, mappings: BTreeMap::new() }),
};
struct Guard;
impl Guard {
    fn acquire() -> Self {
        while SHARED.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() { hint::spin_loop(); }
        let mut guard = Self;
        if guard.nodes.is_empty() {
            // The root predates any clock the port sets: its creation time stays 0.
            let body = Body::Directory { parent: ROOT, entries: Entries::new() };
            guard.nodes.insert(ROOT, Node { mode: 0o777, created: 0, accessed: 0, modified: 0, changed: 0, links: 1, opens: 0, maps: 0, locks: Vec::new(), body });
        }
        guard
    }
}
impl Deref for Guard {
    type Target = State;
    fn deref(&self) -> &State { unsafe { &*SHARED.state.get() } }
}
impl DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut State { unsafe { &mut *SHARED.state.get() } }
}
impl Drop for Guard { fn drop(&mut self) { SHARED.locked.store(false, Ordering::Release); } }

impl State {
    // Entries and open handles keep their node in the table, so a miss means a stale handle: report it, never panic.
    fn node(&self, id: u64) -> Result<&Node> { self.nodes.get(&id).ok_or(Error::InvalidArgument) }
    fn node_mut(&mut self, id: u64) -> Result<&mut Node> { self.nodes.get_mut(&id).ok_or(Error::InvalidArgument) }
    fn kind(&self, id: u64) -> Result<u32> {
        Ok(match self.node(id)?.body { Body::File(_) => NODE_FILE, Body::Directory { .. } => NODE_DIRECTORY, Body::Link(_) => NODE_SYMLINK })
    }
    fn is_directory(&self, id: u64) -> Result<bool> { Ok(self.kind(id)? == NODE_DIRECTORY) }
    fn entries(&self, id: u64) -> Result<&Entries> {
        match &self.node(id)?.body { Body::Directory { entries, .. } => Ok(entries), _ => Err(Error::NotDirectory) }
    }
    /// The directory containing directory `id`; the root contains itself.
    fn parent(&self, id: u64) -> Result<u64> {
        match self.node(id)?.body { Body::Directory { parent, .. } => Ok(parent), _ => Err(Error::NotDirectory) }
    }
    fn entries_mut(&mut self, id: u64) -> Result<&mut Entries> {
        match &mut self.node_mut(id)?.body { Body::Directory { entries, .. } => Ok(entries), _ => Err(Error::NotDirectory) }
    }
    fn working(&self) -> Result<u64> { if self.nodes.contains_key(&self.working) { Ok(self.working) } else { Err(Error::NotFound) } }
    fn walk(&self, path: &[u8], last: Last) -> Result<Place> {
        let mut parent = if path.starts_with(b"/") { ROOT } else { self.working()? };
        let mut pending = Pending { texts: [&[]; MAX_LINKS + 1], depth: 1, entered: 0, slash: false };
        pending.texts[0] = path;
        let mut found: Option<(&[u8], Option<u64>)> = None;
        while let Some(component) = pending.next() {
            // Whatever follows a name resolves inside it, so it has to be a directory.
            if let Some((_, node)) = found.take() { parent = node.ok_or(Error::NotFound)?; }
            match component {
                b"." => { self.entries(parent)?; }
                b".." => parent = self.parent(parent)?,
                name if name.len() > MAX_ENTRY_NAME => return Err(Error::NameTooLong),
                name => {
                    let node = self.entries(parent)?.get(name).copied();
                    match node.and_then(|id| self.nodes.get(&id)) {
                        // A relative target resolves in the directory holding the link.
                        Some(Node { body: Body::Link(target), .. }) if pending.follows(last) => {
                            pending.enter(target)?;
                            if target.starts_with(b"/") { parent = ROOT; }
                        }
                        _ => found = Some((name, node)),
                    }
                }
            }
        }
        let slash = pending.slash;
        match found {
            Some((_, Some(node))) if slash && last != Last::Create && !self.is_directory(node)? => Err(Error::NotDirectory),
            Some((name, node)) => Ok(Place { parent, name: Name::new(name), node, slash }),
            None => Ok(Place { parent, name: Name::new(b""), node: Some(parent), slash }),
        }
    }
    /// Where a call may create the non-directory that `path` names.
    fn vacant(&self, path: &[u8]) -> Result<Place> {
        let place = self.walk(path, Last::Create)?;
        if place.node.is_some() { Err(Error::AlreadyExists) } else if place.slash { Err(Error::NotFound) } else { Ok(place) }
    }
    /// Visits `name` unless it is empty, then the name of `directory` and of each
    /// directory above it. A directory does not know its name: every level is a search
    /// of the entries that hold it.
    fn climb(&self, mut directory: u64, name: &[u8], mut visit: impl FnMut(&[u8])) -> Result<()> {
        if !name.is_empty() { visit(name); }
        while directory != ROOT {
            let above = self.parent(directory)?;
            let (own, _) = self.entries(above)?.iter().find(|(_, id)| **id == directory).ok_or(Error::NotFound)?;
            visit(own);
            directory = above;
        }
        Ok(())
    }
    /// Writes the absolute path of `name` in `directory`, or of `directory` for an
    /// empty name, under the text contract. Measured first and written from its end,
    /// so that no list of names is allocated.
    unsafe fn spell(&self, directory: u64, name: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        let mut length = 0;
        self.climb(directory, name, |part| length += 1 + part.len())?;
        let needed = length.max(1) + 1;
        if needed > MAX_NAME + 1 { return Err(Error::NameTooLong); }
        if needed <= capacity {
            let mut end = needed - 1;
            unsafe { out.write(b'/'); out.add(end).write(0); }
            self.climb(directory, name, |part| {
                end -= part.len() + 1;
                unsafe { out.add(end).write(b'/'); ptr::copy_nonoverlapping(part.as_ptr(), out.add(end + 1), part.len()); }
            })?;
        }
        Ok(needed)
    }
    fn status(&self, id: u64) -> Result<Status> {
        let node = self.node(id)?;
        let (kind, size) = match &node.body {
            Body::File(content) => (NODE_FILE, content.len() as u64),
            Body::Directory { .. } => (NODE_DIRECTORY, 0),
            Body::Link(target) => (NODE_SYMLINK, target.len() as u64),
        };
        Ok(Status { kind, mode: node.mode, size, modified_ns: node.modified, accessed_ns: node.accessed,
            changed_ns: node.changed, created_ns: node.created, identity: id, device: 1 })
    }
    fn touch(&mut self, id: u64, now: u64) -> Result<()> {
        let node = self.node_mut(id)?;
        (node.modified, node.changed) = (now, now);
        Ok(())
    }
    fn create(&mut self, place: &Place, mode: u32, body: Body, now: u64) -> Result<u64> {
        let id = self.next_id;
        self.entries_mut(place.parent)?.insert(place.name[..].into(), id);
        self.next_id += 1;
        self.nodes.insert(id, Node { mode: mode & MODE_MASK, created: now, accessed: now, modified: now, changed: now, links: 1, opens: 0, maps: 0, locks: Vec::new(), body });
        self.touch(place.parent, now)?;
        let mark = if self.is_directory(id)? { watches::DIRECTORY } else { 0 };
        self.report_entry(place.parent, &place.name, watches::CREATE | mark, 0);
        Ok(id)
    }
    /// Frees a node that no directory entry, handle or mapping refers to.
    fn reap(&mut self, id: u64) {
        if self.nodes.get(&id).is_some_and(|node| node.links == 0 && node.opens == 0 && node.maps == 0) {
            if let Some(Node { body: Body::File(content), .. }) = self.nodes.remove(&id) { self.used -= content.len() as u64; }
        }
    }
    /// The node lost a directory entry; the entry itself is already gone. Its watches hear of the link count when
    /// `counted`, which is what Linux tells them except after rmdir(2), and end with the last name.
    fn unlinked(&mut self, id: u64, now: u64, counted: bool) -> Result<()> {
        let node = self.node_mut(id)?;
        (node.links, node.changed) = (node.links.saturating_sub(1), now);
        let ended = node.links == 0;
        if counted { self.report(id, watches::ATTRIBUTES); }
        if ended { self.ended(id); }
        self.reap(id);
        Ok(())
    }
    fn set_mode(&mut self, id: u64, mode: u32, now: u64) -> Result<()> {
        let node = self.node_mut(id)?;
        (node.mode, node.changed) = (mode & MODE_MASK, now);
        self.report(id, watches::ATTRIBUTES);
        Ok(())
    }
    fn set_times(&mut self, id: u64, accessed: Option<u64>, modified: Option<u64>, now: u64) -> Result<()> {
        let node = self.node_mut(id)?;
        // As with utimensat(2), a call that keeps both times is not a change.
        if accessed.is_none() && modified.is_none() { return Ok(()); }
        (node.accessed, node.modified, node.changed) = (accessed.unwrap_or(node.accessed), modified.unwrap_or(node.modified), now);
        self.report(id, watches::ATTRIBUTES);
        Ok(())
    }
    /// Tells the watches that `kinds` happened to node `id`: first the watches of the directories that hold it,
    /// under each of its names there, then the watch of the node itself. That is the order of inotify, which knows
    /// an open file by one name only; here a handle has no name, so every name reports.
    fn report(&mut self, id: u64, kinds: u32) {
        if self.watchers.is_empty() { return; }
        let Self { nodes, watchers, .. } = self;
        let Some(node) = nodes.get(&id) else { return; };
        // A directory has one name, in its parent; the names of anything else may be in any directory.
        let (mark, holder) = match node.body { Body::Directory { parent, .. } => (watches::DIRECTORY, Some(parent)), _ => (0, None) };
        for Watcher { queue, watches, .. } in watchers.values_mut() {
            let holders = watches.iter().filter(|watch| watch.events & kinds != 0 && watch.node != id && holder.is_none_or(|holder| holder == watch.node));
            for watch in holders {
                let Some(Node { body: Body::Directory { entries, .. }, .. }) = nodes.get(&watch.node) else { continue; };
                for (name, _) in entries.iter().filter(|(_, child)| **child == id).take(node.links) { queue.push(watch.id, kinds | mark, 0, name); }
            }
            if let Some(own) = watches.iter().find(|watch| watch.node == id && watch.events & kinds != 0) { queue.push(own.id, kinds | mark, 0, b""); }
        }
    }
    /// Tells the watches of `directory` what happened to its entry `name`; `kinds` carries the mark of a directory.
    fn report_entry(&mut self, directory: u64, name: &[u8], kinds: u32, cookie: u32) {
        for Watcher { queue, watches, .. } in self.watchers.values_mut() {
            if let Some(watch) = watches.iter().find(|watch| watch.node == directory && watch.events & kinds != 0) { queue.push(watch.id, kinds, cookie, name); }
        }
    }
    /// Node `id` has no name left: its watches end, which they hear whatever they asked for.
    fn ended(&mut self, id: u64) {
        for Watcher { queue, watches, .. } in self.watchers.values_mut() {
            if let Some(index) = watches.iter().position(|watch| watch.node == id) { queue.push(watches.remove(index).id, watches::REMOVED, 0, b""); }
        }
    }
    /// Makes `wanted` what its owner holds on its bytes of node `id`, or nothing for
    /// `LOCK_UNLOCK`. What the owner held beside those bytes stays, and a lock of
    /// another handle in the way leaves everything as it was.
    fn relock(&mut self, id: u64, mode: u32, wanted: Lock) -> Result<()> {
        let locks = &mut self.node_mut(id)?.locks;
        let meets = |held: &Lock| held.whole == wanted.whole && held.start < wanted.end && wanted.start < held.end;
        if mode != LOCK_UNLOCK && locks.iter().any(|held| held.owner != wanted.owner && meets(held) && (held.exclusive || wanted.exclusive)) {
            return Err(Error::WouldBlock);
        }
        // Room for the lock and the piece a split adds, so that nothing fails halfway.
        locks.try_reserve(2).map_err(|_| Error::OutOfMemory)?;
        let mut index = 0;
        while let Some(held) = locks.get(index).copied() {
            if held.owner != wanted.owner || !meets(&held) { index += 1; continue; }
            locks.swap_remove(index);
            if held.start < wanted.start { locks.push(Lock { end: wanted.start, ..held }); }
            if wanted.end < held.end { locks.push(Lock { start: wanted.end, ..held }); }
        }
        if mode != LOCK_UNLOCK { locks.push(wanted); }
        Ok(())
    }
    fn content(&self, id: u64) -> Result<&Content> {
        match &self.node(id)?.body { Body::File(content) => Ok(content), _ => Err(Error::IsDirectory) }
    }
    /// Runs `edit` on the content of file `id` with the bytes it may still grow by and whether it is mapped, then
    /// settles the accounting and the timestamps. The watches hear of a change of size, and of any change when `told`.
    fn change<T>(&mut self, id: u64, now: u64, told: bool, edit: impl FnOnce(&mut Content, u64, bool) -> Result<T>) -> Result<T> {
        let room = self.capacity.saturating_sub(self.used);
        let node = self.nodes.get_mut(&id).ok_or(Error::InvalidArgument)?;
        let mapped = node.maps != 0;
        let Body::File(content) = &mut node.body else { return Err(Error::IsDirectory); };
        let before = content.len();
        let result = edit(content, room, mapped)?;
        let after = content.len();
        self.used = self.used - before as u64 + after as u64;
        (node.modified, node.changed) = (now, now);
        if told || before != after { self.report(id, watches::MODIFY); }
        Ok(result)
    }
    fn watcher(&mut self, id: u64) -> Result<&mut Watcher> { self.watchers.get_mut(&id).ok_or(Error::InvalidArgument) }
}

/// Makes room for `additional` more bytes without ever aborting on exhaustion.
fn reserve(content: &mut Vec<u8>, additional: usize, room: u64) -> Result<()> {
    if additional as u64 > room { return Err(Error::NoSpace); }
    // Amortized growth first; the exact size is the fallback when memory is tight.
    if content.try_reserve(additional).is_ok() || content.try_reserve_exact(additional).is_ok() { Ok(()) } else { Err(Error::NoSpace) }
}
fn resize(content: &mut Vec<u8>, size: u64, room: u64) -> Result<()> {
    let size = usize::try_from(size).map_err(|_| Error::NoSpace)?;
    if size > content.len() {
        reserve(content, size - content.len(), room)?;
        content.resize(size, 0);
    } else if size == 0 {
        *content = Vec::new();
    } else {
        content.truncate(size);
        // Give the tail back without a shrinking reallocation, which may abort where
        // the allocator implements it as allocate, copy and free.
        if size <= content.capacity() / 2 {
            let mut exact = Vec::new();
            if exact.try_reserve_exact(size).is_ok() { exact.extend_from_slice(content); *content = exact; }
        }
    }
    Ok(())
}
/// The range was reserved, so nothing below reallocates.
fn write(content: &mut Vec<u8>, offset: u64, data: &[u8], room: u64) -> Result<usize> {
    let end = offset.checked_add(data.len() as u64).and_then(|end| usize::try_from(end).ok()).ok_or(Error::NoSpace)?;
    let start = end - data.len();
    if end > content.len() {
        reserve(content, end - content.len(), room)?;
        if start > content.len() { content.resize(start, 0); }
    }
    let inside = content.len().min(end) - start;
    content[start..start + inside].copy_from_slice(&data[..inside]);
    content.extend_from_slice(&data[inside..]);
    Ok(data.len())
}

/// What a handle points to. File handles hold the node, so its content outlives
/// its names; directory handles hold the snapshot: `kind, length, name` per entry.
/// The address of a file handle is the owner of the locks taken through it.
/// A watcher handle holds the key of its watcher, which lives in the tree where the calls that cause events find it.
enum Handle { File { node: u64, access: u32 }, Directory { listing: Vec<u8>, next: usize }, Watcher { id: u64 } }
fn handle(value: Handle) -> *mut c_void { Box::into_raw(Box::new(value)).cast() }
/// Handles are trusted borrows of boxes made by `handle`; holding the lock makes the borrow exclusive.
unsafe fn borrow(_held: &mut Guard, handle: *mut c_void) -> Result<&mut Handle> {
    if handle.is_null() || !(handle as usize).is_multiple_of(mem::align_of::<Handle>()) { return Err(Error::InvalidArgument); }
    Ok(unsafe { &mut *handle.cast::<Handle>() })
}
/// The node of a file handle opened with at least `access`.
unsafe fn file_node(held: &mut Guard, handle: *mut c_void, access: u32) -> Result<u64> {
    match *unsafe { borrow(held, handle) }? {
        Handle::File { node, access: granted } if granted & access == access => Ok(node),
        Handle::File { .. } => Err(Error::AccessDenied),
        _ => Err(Error::InvalidArgument),
    }
}
unsafe fn watcher_id(held: &mut Guard, handle: *mut c_void) -> Result<u64> {
    match *unsafe { borrow(held, handle) }? { Handle::Watcher { id } => Ok(id), _ => Err(Error::InvalidArgument) }
}

/// A path that ends at `/`, `.` or `..` names no directory entry. As on Linux, renaming
/// from or to one is `Busy`, removing the root is `Busy`, and removing a directory
/// through a final `.` is `InvalidArgument`. A handle of the wrong kind is
/// `InvalidArgument` and stays open, because it still belongs to the other closing call.
impl Files for MemFs {
    unsafe fn open(path: &[u8], flags: u32, mode: u32) -> Result<*mut c_void> {
        let now = if flags & (CREATE | TRUNCATE) != 0 { clock() } else { 0 };
        let mut fs = Guard::acquire();
        // Without `EXCLUSIVE` a dangling link leads to the place of the file to create.
        let place = fs.walk(path, if flags & EXCLUSIVE != 0 { Last::Create } else { Last::Follow })?;
        let node = match place.node {
            Some(_) if flags & EXCLUSIVE != 0 => return Err(Error::AlreadyExists),
            Some(node) if fs.is_directory(node)? => return Err(Error::IsDirectory),
            Some(node) => { if flags & TRUNCATE != 0 { fs.change(node, now, false, |content, room, mapped| content.resize(0, room, mapped))?; } node }
            None if flags & CREATE == 0 => return Err(Error::NotFound),
            None if place.slash => return Err(Error::IsDirectory),
            None => fs.create(&place, mode, Body::File(Content::Plain(Vec::new())), now)?,
        };
        fs.node_mut(node)?.opens += 1;
        Ok(handle(Handle::File { node, access: flags & (READ | WRITE) }))
    }
    unsafe fn close(file: *mut c_void) -> Result<()> {
        let mut fs = Guard::acquire();
        let Handle::File { node, .. } = *unsafe { borrow(&mut fs, file) }? else { return Err(Error::InvalidArgument); };
        drop(unsafe { Box::from_raw(file.cast::<Handle>()) });
        let opened = fs.node_mut(node)?;
        opened.opens = opened.opens.saturating_sub(1);
        opened.locks.retain(|held| held.owner != file.addr());
        if opened.locks.is_empty() { opened.locks = Vec::new(); }
        fs.reap(node);
        Ok(())
    }
    unsafe fn read_at(file: *mut c_void, offset: u64, out: *mut u8, capacity: usize) -> Result<usize> {
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, READ) }?;
        let count = unsafe { fs.content(node)?.read(offset, out, capacity) };
        // As with inotify, a read at the end of the file is no access.
        if count != 0 { fs.report(node, watches::ACCESS); }
        Ok(count)
    }
    unsafe fn write_at(file: *mut c_void, offset: u64, data: *const u8, size: usize) -> Result<usize> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, WRITE) }?;
        fs.change(node, now, size != 0, |content, room, mapped| unsafe { content.write(offset, data, size, room, mapped) })
    }
    unsafe fn set_size(file: *mut c_void, size: u64) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, WRITE) }?;
        fs.change(node, now, false, |content, room, mapped| content.resize(size, room, mapped))
    }
    unsafe fn flush(file: *mut c_void) -> Result<()> {
        unsafe { file_node(&mut Guard::acquire(), file, 0) }.map(|_| ())
    }
    unsafe fn status(file: *mut c_void) -> Result<Status> {
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, 0) }?;
        fs.status(node)
    }
    unsafe fn path_status(path: &[u8], follow: bool) -> Result<Status> {
        let fs = Guard::acquire();
        let node = fs.walk(path, if follow { Last::Follow } else { Last::Keep })?.node.ok_or(Error::NotFound)?;
        fs.status(node)
    }
    unsafe fn remove(path: &[u8]) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let place = fs.walk(path, Last::Entry)?;
        let node = place.node.ok_or(Error::NotFound)?;
        if fs.is_directory(node)? { return Err(Error::IsDirectory); }
        fs.entries_mut(place.parent)?.remove(&place.name[..]);
        fs.touch(place.parent, now)?;
        // The order of inotify: what the file's own watches hear comes before the DELETE of its directory.
        fs.unlinked(node, now, true)?;
        fs.report_entry(place.parent, &place.name, watches::DELETE, 0);
        Ok(())
    }
    unsafe fn rename(from: &[u8], to: &[u8]) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let source = fs.walk(from, Last::Entry)?;
        let node = source.node.ok_or(Error::NotFound)?;
        let target = fs.walk(to, Last::Entry)?;
        if source.name.is_empty() || target.name.is_empty() { return Err(Error::Busy); }
        // The same path, or two names of one file: POSIX has this do nothing.
        if target.node == Some(node) { return Ok(()); }
        let directory = fs.is_directory(node)?;
        if directory {
            // The destination may not lie inside the directory being moved.
            let mut ancestor = target.parent;
            while ancestor != ROOT {
                if ancestor == node { return Err(Error::InvalidArgument); }
                ancestor = fs.parent(ancestor)?;
            }
        } else if target.slash {
            return Err(Error::NotDirectory);
        }
        if let Some(replaced) = target.node {
            match (directory, fs.is_directory(replaced)?) {
                (false, true) => return Err(Error::IsDirectory),
                (true, false) => return Err(Error::NotDirectory),
                (true, true) if !fs.entries(replaced)?.is_empty() => return Err(Error::NotEmpty),
                _ => {}
            }
        }
        // Every refusal is decided above: what follows cannot stop halfway.
        fs.entries_mut(source.parent)?.remove(&source.name[..]);
        let replaced = fs.entries_mut(target.parent)?.insert(target.name[..].into(), node);
        fs.cookie = if fs.cookie == u32::MAX { 1 } else { fs.cookie + 1 };
        let (mark, cookie) = (if directory { watches::DIRECTORY } else { 0 }, fs.cookie);
        fs.report_entry(source.parent, &source.name, watches::MOVED_FROM | mark, cookie);
        fs.report_entry(target.parent, &target.name, watches::MOVED_TO | mark, cookie);
        // As with inotify, what was replaced is no DELETE for its directory: only its own watches hear of it.
        if let Some(replaced) = replaced { fs.unlinked(replaced, now, true)?; }
        let moved = fs.node_mut(node)?;
        moved.changed = now;
        if let Body::Directory { parent, .. } = &mut moved.body { *parent = target.parent; }
        fs.touch(source.parent, now)?;
        fs.touch(target.parent, now)
    }
    unsafe fn create_directory(path: &[u8], mode: u32) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let place = fs.walk(path, Last::Create)?;
        if place.node.is_some() { return Err(Error::AlreadyExists); }
        fs.create(&place, mode, Body::Directory { parent: place.parent, entries: Entries::new() }, now).map(|_| ())
    }
    unsafe fn remove_directory(path: &[u8]) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let place = fs.walk(path, Last::Entry)?;
        let node = place.node.ok_or(Error::NotFound)?;
        let empty = fs.entries(node)?.is_empty();
        if node == ROOT { return Err(Error::Busy); }
        // What a final `..` reaches still holds the directory the path came through.
        if !empty { return Err(Error::NotEmpty); }
        if place.name.is_empty() { return Err(Error::InvalidArgument); }
        fs.entries_mut(place.parent)?.remove(&place.name[..]);
        fs.touch(place.parent, now)?;
        fs.unlinked(node, now, false)?;
        fs.report_entry(place.parent, &place.name, watches::DELETE | watches::DIRECTORY, 0);
        Ok(())
    }
    unsafe fn open_directory(path: &[u8]) -> Result<*mut c_void> {
        let fs = Guard::acquire();
        let node = fs.walk(path, Last::Follow)?.node.ok_or(Error::NotFound)?;
        let entries = fs.entries(node)?;
        let mut listing = Vec::new();
        listing.try_reserve_exact(entries.keys().map(|name| name.len() + 2).sum()).map_err(|_| Error::OutOfMemory)?;
        for (name, child) in entries {
            listing.extend_from_slice(&[fs.kind(*child)? as u8, name.len() as u8]);
            listing.extend_from_slice(name);
        }
        Ok(handle(Handle::Directory { listing, next: 0 }))
    }
    unsafe fn read_directory(directory: *mut c_void, name: *mut u8, capacity: usize) -> Result<(usize, u32)> {
        let mut fs = Guard::acquire();
        let Handle::Directory { listing, next } = (unsafe { borrow(&mut fs, directory) })? else { return Err(Error::InvalidArgument); };
        let [kind, length, rest @ ..] = &listing[*next..] else { return Err(Error::NotFound); };
        let text = rest.get(..*length as usize).ok_or(Error::Os)?;
        if text.len() > capacity { return Err(Error::InvalidArgument); }
        unsafe { ptr::copy_nonoverlapping(text.as_ptr(), name, text.len()) };
        *next += 2 + text.len();
        Ok((text.len(), *kind as u32))
    }
    unsafe fn close_directory(directory: *mut c_void) -> Result<()> {
        let mut fs = Guard::acquire();
        let Handle::Directory { .. } = (unsafe { borrow(&mut fs, directory) })? else { return Err(Error::InvalidArgument); };
        drop(unsafe { Box::from_raw(directory.cast::<Handle>()) });
        Ok(())
    }
    unsafe fn current_directory(out: *mut u8, capacity: usize) -> Result<usize> {
        let fs = Guard::acquire();
        unsafe { fs.spell(fs.working()?, b"", out, capacity) }
    }
    unsafe fn set_mode(path: &[u8], mode: u32) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = fs.walk(path, Last::Follow)?.node.ok_or(Error::NotFound)?;
        fs.set_mode(node, mode, now)
    }
    unsafe fn set_file_mode(file: *mut c_void, mode: u32) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, 0) }?;
        fs.set_mode(node, mode, now)
    }
    unsafe fn set_times(path: &[u8], follow: bool, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = fs.walk(path, if follow { Last::Follow } else { Last::Keep })?.node.ok_or(Error::NotFound)?;
        fs.set_times(node, accessed_ns, modified_ns, now)
    }
    unsafe fn set_file_times(file: *mut c_void, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, 0) }?;
        fs.set_times(node, accessed_ns, modified_ns, now)
    }
    /// As link(2): a link in the final name of `existing` is itself what gets the new
    /// name, and a directory is refused with `AccessDenied` once `created` is known to be free.
    unsafe fn link(existing: &[u8], created: &[u8]) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = fs.walk(existing, Last::Keep)?.node.ok_or(Error::NotFound)?;
        let place = fs.vacant(created)?;
        if fs.is_directory(node)? { return Err(Error::AccessDenied); }
        // The order of inotify: the link count under the names the file had, then the new name.
        fs.report(node, watches::ATTRIBUTES);
        fs.entries_mut(place.parent)?.insert(place.name[..].into(), node);
        let linked = fs.node_mut(node)?;
        (linked.links, linked.changed) = (linked.links + 1, now);
        fs.touch(place.parent, now)?;
        fs.report_entry(place.parent, &place.name, watches::CREATE, 0);
        Ok(())
    }
    unsafe fn symlink(target: &[u8], created: &[u8]) -> Result<()> {
        if target.is_empty() { return Err(Error::NotFound); }
        if target.len() > MAX_NAME { return Err(Error::NameTooLong); }
        let now = clock();
        let mut text = Vec::new();
        text.try_reserve_exact(target.len()).map_err(|_| Error::NoSpace)?;
        text.extend_from_slice(target);
        let mut fs = Guard::acquire();
        let place = fs.vacant(created)?;
        fs.create(&place, 0o777, Body::Link(text), now).map(|_| ())
    }
    unsafe fn read_link(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        let fs = Guard::acquire();
        let node = fs.walk(path, Last::Keep)?.node.ok_or(Error::NotFound)?;
        let Body::Link(target) = &fs.node(node)?.body else { return Err(Error::InvalidArgument); };
        if target.len() < capacity { unsafe { ptr::copy_nonoverlapping(target.as_ptr(), out, target.len()); out.add(target.len()).write(0); } }
        Ok(target.len() + 1)
    }
    unsafe fn real_path(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        let fs = Guard::acquire();
        let place = fs.walk(path, Last::Follow)?;
        place.node.ok_or(Error::NotFound)?;
        unsafe { fs.spell(place.parent, &place.name, out, capacity) }
    }
    unsafe fn set_current_directory(path: &[u8]) -> Result<()> {
        let mut fs = Guard::acquire();
        let node = fs.walk(path, Last::Follow)?.node.ok_or(Error::NotFound)?;
        fs.entries(node)?;
        fs.working = node;
        Ok(())
    }
    /// The rules of flock(2). Any number of handles hold `LOCK_SHARED`, or one holds
    /// `LOCK_EXCLUSIVE`. A handle that asks for the other mode gives up the one it
    /// holds first: a refused conversion leaves it with no lock, and two handles that
    /// convert at once do not wait for each other forever.
    ///
    /// `no_std` has nothing to block on, so a wait is a loop of attempts, with the
    /// tree's lock free and [`set_yield`]'s function called between them. On a
    /// cooperative or single-core port the holder runs, and unlocks, only if the waiter
    /// yields: such a port sets its scheduler's yield or never waits. The BCL never
    /// does: `FileStream` always locks without waiting.
    unsafe fn lock(file: *mut c_void, mode: u32, wait: bool) -> Result<()> {
        let wanted = Lock { owner: file.addr(), whole: true, start: 0, end: u64::MAX, exclusive: mode == LOCK_EXCLUSIVE };
        loop {
            {
                let mut fs = Guard::acquire();
                let node = unsafe { file_node(&mut fs, file, 0) }?;
                fs.relock(node, LOCK_UNLOCK, wanted)?;
                match fs.relock(node, mode, wanted) { Err(Error::WouldBlock) if wait => {} done => return done }
            }
            pause();
        }
    }
    /// The rules of open file description locks (fcntl(2) `F_OFD_SETLK`): a refused
    /// call changes nothing, unlocking the middle of a lock leaves its two ends, and
    /// a shared lock needs a handle opened for reading, an exclusive one a handle
    /// opened for writing.
    unsafe fn lock_range(file: *mut c_void, offset: u64, length: u64, mode: u32) -> Result<()> {
        let mut fs = Guard::acquire();
        let access = match mode { LOCK_UNLOCK => 0, LOCK_EXCLUSIVE => WRITE, _ => READ };
        let node = unsafe { file_node(&mut fs, file, access) }?;
        let end = offset.checked_add(length).filter(|end| *end > offset).ok_or(Error::InvalidArgument)?;
        fs.relock(node, mode, Lock { owner: file.addr(), whole: false, start: offset, end, exclusive: mode == LOCK_EXCLUSIVE })
    }
}

/// The calls that change the tree queue their events themselves, so a reader gets them complete, in the order they
/// happened and without a look at the tree. What the file's own watches and the watches of its directories hear of
/// one call, and in which order, is what inotify tells on Linux, with these differences: a file with several names
/// reports under each of them, a change of the link count is `ATTRIBUTES` under the names that stay as well,
/// `set_times` is `ATTRIBUTES` whichever time it sets, a change of size to the same size is nothing, and a file's own
/// watch ends with its last name even while handles keep the file. Writes through a mapping report nothing.
impl Watches for MemFs {
    fn open() -> Result<*mut c_void> {
        let queue = Queue::new()?;
        let mut fs = Guard::acquire();
        let id = fs.next_watcher;
        fs.next_watcher += 1;
        fs.watchers.insert(id, Watcher { queue, watches: Vec::new(), last: 0 });
        Ok(handle(Handle::Watcher { id }))
    }
    unsafe fn close(watcher: *mut c_void) -> Result<()> {
        let mut fs = Guard::acquire();
        let id = unsafe { watcher_id(&mut fs, watcher) }?;
        drop(unsafe { Box::from_raw(watcher.cast::<Handle>()) });
        fs.watchers.remove(&id);
        Ok(())
    }
    unsafe fn add(watcher: *mut c_void, path: &[u8], events: u32) -> Result<u32> {
        let wanted = events & REQUESTABLE;
        if wanted == 0 { return Err(Error::InvalidArgument); }
        let mut fs = Guard::acquire();
        let id = unsafe { watcher_id(&mut fs, watcher) }?;
        let node = fs.walk(path, if events & watches::NO_FOLLOW != 0 { Last::Keep } else { Last::Follow })?.node.ok_or(Error::NotFound)?;
        if events & watches::ONLY_DIRECTORY != 0 && !fs.is_directory(node)? { return Err(Error::NotDirectory); }
        let watcher = fs.watcher(id)?;
        if let Some(known) = watcher.watches.iter_mut().find(|watch| watch.node == node) {
            known.events = wanted;
            return Ok(known.id);
        }
        if watcher.last == u32::MAX { return Err(Error::NoSpace); }
        watcher.watches.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        watcher.last += 1;
        watcher.watches.push(Watch { id: watcher.last, node, events: wanted });
        Ok(watcher.last)
    }
    /// A watch that has ended, by this call or with its node, is `InvalidArgument`.
    unsafe fn remove(watcher: *mut c_void, watch: u32) -> Result<()> {
        let mut fs = Guard::acquire();
        let id = unsafe { watcher_id(&mut fs, watcher) }?;
        let watcher = fs.watcher(id)?;
        let index = watcher.watches.iter().position(|known| known.id == watch).ok_or(Error::InvalidArgument)?;
        watcher.watches.remove(index);
        watcher.queue.push(watch, watches::REMOVED, 0, b"");
        Ok(())
    }
    /// `no_std` has nothing to block on, so a read that finds its queue empty lets the tree's lock go and looks again
    /// after [`set_wait`]'s function has returned, which is when `add`, `remove` and the calls that cause events
    /// run. Without that function it looks again after [`set_yield`]'s, until [`set_clock`]'s clock says the time
    /// is up; a timed read that has neither does not wait.
    unsafe fn read(watcher: *mut c_void, timeout_ns: u64, event: &mut Event) -> Result<()> {
        let (mut left, mut deadline) = (timeout_ns, None);
        loop {
            {
                let mut fs = Guard::acquire();
                let id = unsafe { watcher_id(&mut fs, watcher) }?;
                if let Some(next) = fs.watcher(id)?.queue.pop() { *event = next; return Ok(()); }
            }
            if left == 0 { return Err(Error::Timeout); }
            if let Some(wait) = wait_hook() {
                let slice = left.min(WAIT_SLICE);
                wait(slice);
                if timeout_ns != watches::FOREVER { left -= slice; }
                continue;
            }
            if timeout_ns != watches::FOREVER {
                if !has_clock() { return Err(Error::Timeout); }
                let now = clock();
                if now >= *deadline.get_or_insert(now.saturating_add(timeout_ns)) { return Err(Error::Timeout); }
            }
            pause();
        }
    }
}

/// There is no MMU behind this provider, which decides what a mapping can be. A shared mapping is a pointer into the
/// file's own bytes: the file moves into whole pages when it is first mapped, and they stay where they are while a
/// shared mapping exists, so until the last `unmap` the file grows to the end of those pages and no further
/// (`NoSpace`). A private mapping is a copy of the range as it was when `map` was called. Nothing past the pages of
/// the file can be mapped (`InvalidArgument`), because there is no fault to raise for it, and `access` is checked
/// against the handle but not enforced on the memory. `EXECUTE` is `Unsupported`.
impl Mappings for MemFs {
    unsafe fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8> {
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, READ) }?;
        if shared && access & runtime::WRITE != 0 { unsafe { file_node(&mut fs, file, WRITE) }?; }
        if access & runtime::EXECUTE != 0 { return Err(Error::Unsupported); }
        // The page that holds the last byte is mapped whole, and no page begins behind it.
        let limit = fs.content(node)?.len().checked_next_multiple_of(PAGE).ok_or(Error::InvalidArgument)?;
        let start = usize::try_from(offset).ok().filter(|start| start.is_multiple_of(PAGE)).ok_or(Error::InvalidArgument)?;
        start.checked_add(length).filter(|end| length != 0 && *end <= limit).ok_or(Error::InvalidArgument)?;
        if shared {
            let Body::File(content) = &mut fs.node_mut(node)?.body else { return Err(Error::InvalidArgument); };
            // Every range a mapping may ask for ends inside the pages of the file as it is now, so they are what is pinned.
            let base = content.pin(limit)?;
            // SAFETY: `start` lies inside the pinned pages.
            let address = unsafe { base.as_ptr().add(start) };
            // Two mappings of one range are one address: the record counts them.
            match fs.mappings.entry((address.addr(), length)).or_insert(Mapped::Shared { node, count: 0 }) {
                Mapped::Shared { count, .. } => *count += 1,
                Mapped::Private => return Err(Error::Os),
            }
            fs.node_mut(node)?.maps += 1;
            return Ok(address);
        }
        let content = fs.content(node)?;
        let base = pages(length.checked_next_multiple_of(PAGE).ok_or(Error::OutOfMemory)?)?;
        // SAFETY: the pages are new and hold `length` bytes; what the file does not fill stays zero.
        unsafe { content.read(offset, base.as_ptr(), length) };
        fs.mappings.insert((base.addr().get(), length), Mapped::Private);
        Ok(base.as_ptr())
    }
    unsafe fn unmap(address: *mut u8, length: usize) -> Result<()> {
        let mut fs = Guard::acquire();
        let key = (address.addr(), length);
        match fs.mappings.get_mut(&key).ok_or(Error::InvalidArgument)? {
            Mapped::Shared { node, count } => {
                let node = *node;
                *count -= 1;
                if *count == 0 { fs.mappings.remove(&key); }
                let mapped = fs.node_mut(node)?;
                mapped.maps = mapped.maps.saturating_sub(1);
                fs.reap(node);
            }
            Mapped::Private => {
                fs.mappings.remove(&key);
                // SAFETY: the record says these are the pages `map` made for a copy of this length.
                unsafe { free_pages(address, length.next_multiple_of(PAGE)) };
            }
        }
        Ok(())
    }
    /// A shared mapping is the file, so there is nothing to carry over.
    unsafe fn sync(address: *mut u8, length: usize) -> Result<()> {
        if Guard::acquire().mappings.contains_key(&(address.addr(), length)) { Ok(()) } else { Err(Error::InvalidArgument) }
    }
}

/// The tree is one volume mounted at `/`: its capacity is [`set_capacity`]'s, and what
/// file content does not occupy is free to every caller.
impl Volumes for MemFs {
    unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
        if index != 0 { return Err(Error::NotFound); }
        // SAFETY: a writable buffer of `capacity` bytes is the caller's contract.
        if capacity >= 2 { unsafe { out.copy_from_nonoverlapping(c"/".as_ptr().cast(), 2) }; }
        Ok(2)
    }
    fn status(path: &[u8]) -> Result<dotnet_pal_rs::volumes::Status> {
        let fs = Guard::acquire();
        fs.walk(path, Last::Follow)?.node.ok_or(Error::NotFound)?;
        // A capacity lowered below what is stored leaves nothing free rather than a negative amount.
        let (total, free) = (fs.capacity.max(fs.used), fs.capacity.saturating_sub(fs.used));
        Ok(dotnet_pal_rs::volumes::Status::new(total, free, free, b"memfs"))
    }
}
