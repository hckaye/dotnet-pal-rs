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
//! Reads are not tracked, as on a file system mounted `noatime`: the access time of a
//! node is its creation time until `set_times` stores another. The working directory
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
//! dotnet_pal_rs::define_pal! { Files = dotnet_pal_memfs::MemFs, Volumes = dotnet_pal_memfs::MemFs, /* other providers */ }
//! dotnet_pal_memfs::set_clock(unix_time_ns); // optional: every timestamp is 0 without it
//! dotnet_pal_memfs::set_yield(scheduler_yield); // optional: what a waiting `lock` does between attempts
//! // The final image supplies the `#[global_allocator]` this crate allocates from.
//! ```
//!
//! One spin lock guards the tree. It is held across the copy of a transfer and
//! across allocator calls, never across the clock, the yield of a waiting `lock` or
//! anything that can block. It is not reentrant: an interrupt or fault handler must
//! not call the provider.
//! File content and link targets grow with fallible reservations, so an exhausted
//! allocator is `NoSpace` like an exceeded [`set_capacity`]; a directory snapshot or a
//! lock record that does not fit is `OutOfMemory`. Nodes, names and handles are small
//! and use ordinary allocation, which ends in the image's allocation-error path when
//! it fails.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
extern crate alloc;
use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use core::{cell::UnsafeCell, ffi::c_void, hint, mem, ops::{Deref, DerefMut}, ptr, slice, sync::atomic::{AtomicBool, AtomicPtr, Ordering}};
use dotnet_pal_rs::files::{Status, CREATE, EXCLUSIVE, LOCK_EXCLUSIVE, LOCK_UNLOCK, MAX_ENTRY_NAME, NODE_DIRECTORY, NODE_FILE, NODE_SYMLINK, READ, TRUNCATE, WRITE};
use dotnet_pal_rs::port::{Error, Files, Result, Volumes};
use dotnet_pal_rs::runtime::MAX_NAME;

/// The in-memory `Files` provider.
pub struct MemFs;

const ROOT: u64 = 1;
const MODE_MASK: u32 = 0o7777;
/// Links one walk follows, the bound of Linux.
const MAX_LINKS: usize = 40;
const DEFAULT_CAPACITY: u64 = 64 * 1024 * 1024;

static CLOCK: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
/// Sets the source of the Unix-epoch nanosecond timestamps stored in nodes.
/// Without one every timestamp is 0, which the boundary reads as "none kept".
pub fn set_clock(now_ns: fn() -> u64) { CLOCK.store(now_ns as *mut (), Ordering::Release); }
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
/// Bounds the total bytes of file content (64 MiB until set); growth past the bound
/// is `NoSpace`. Names, nodes, link targets and allocator slack are not counted. A
/// bound below the bytes in use frees nothing and refuses further growth.
pub fn set_capacity(bytes: u64) { Guard::acquire().capacity = bytes; }
/// Bytes of file content held now, including removed files that are still open.
pub fn used_bytes() -> u64 { Guard::acquire().used }

type Entries = BTreeMap<Box<[u8]>, u64>;
/// `Link` holds the target text of a symbolic link.
enum Body { File(Vec<u8>), Directory { parent: u64, entries: Entries }, Link(Vec<u8>) }
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
    locks: Vec<Lock>,
    body: Body,
}
/// Nodes are keyed by their identity, which is never reused. `working` is the working
/// directory, which stops being a node when it is removed.
struct State { nodes: BTreeMap<u64, Node>, next_id: u64, used: u64, capacity: u64, working: u64 }
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
    state: UnsafeCell::new(State { nodes: BTreeMap::new(), next_id: ROOT + 1, used: 0, capacity: DEFAULT_CAPACITY, working: ROOT }),
};
struct Guard;
impl Guard {
    fn acquire() -> Self {
        while SHARED.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() { hint::spin_loop(); }
        let mut guard = Self;
        if guard.nodes.is_empty() {
            // The root predates any clock the port sets: its creation time stays 0.
            let body = Body::Directory { parent: ROOT, entries: Entries::new() };
            guard.nodes.insert(ROOT, Node { mode: 0o777, created: 0, accessed: 0, modified: 0, changed: 0, links: 1, opens: 0, locks: Vec::new(), body });
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
        self.nodes.insert(id, Node { mode: mode & MODE_MASK, created: now, accessed: now, modified: now, changed: now, links: 1, opens: 0, locks: Vec::new(), body });
        self.touch(place.parent, now)?;
        Ok(id)
    }
    /// Frees a node that neither a directory entry nor a handle refers to.
    fn reap(&mut self, id: u64) {
        if self.nodes.get(&id).is_some_and(|node| node.links == 0 && node.opens == 0) {
            if let Some(Node { body: Body::File(content), .. }) = self.nodes.remove(&id) { self.used -= content.len() as u64; }
        }
    }
    /// The node lost a directory entry; the entry itself is already gone.
    fn unlinked(&mut self, id: u64, now: u64) -> Result<()> {
        let node = self.node_mut(id)?;
        (node.links, node.changed) = (node.links.saturating_sub(1), now);
        self.reap(id);
        Ok(())
    }
    fn set_mode(&mut self, id: u64, mode: u32, now: u64) -> Result<()> {
        let node = self.node_mut(id)?;
        (node.mode, node.changed) = (mode & MODE_MASK, now);
        Ok(())
    }
    fn set_times(&mut self, id: u64, accessed: Option<u64>, modified: Option<u64>, now: u64) -> Result<()> {
        let node = self.node_mut(id)?;
        // As with utimensat(2), a call that keeps both times is not a change.
        if accessed.is_some() || modified.is_some() { node.changed = now; }
        (node.accessed, node.modified) = (accessed.unwrap_or(node.accessed), modified.unwrap_or(node.modified));
        Ok(())
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
    fn content(&self, id: u64) -> Result<&Vec<u8>> {
        match &self.node(id)?.body { Body::File(content) => Ok(content), _ => Err(Error::IsDirectory) }
    }
    /// Runs `edit` on the content of file `id` with the bytes it may still grow by,
    /// then settles the accounting and the timestamps.
    fn change<T>(&mut self, id: u64, now: u64, edit: impl FnOnce(&mut Vec<u8>, u64) -> Result<T>) -> Result<T> {
        let room = self.capacity.saturating_sub(self.used);
        let node = self.nodes.get_mut(&id).ok_or(Error::InvalidArgument)?;
        let Body::File(content) = &mut node.body else { return Err(Error::IsDirectory); };
        let before = content.len() as u64;
        let result = edit(content, room)?;
        self.used = self.used - before + content.len() as u64;
        (node.modified, node.changed) = (now, now);
        Ok(result)
    }
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
/// A write that does not fit is refused whole. The range was reserved, so nothing below reallocates.
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
enum Handle { File { node: u64, access: u32 }, Directory { listing: Vec<u8>, next: usize } }
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
        Handle::Directory { .. } => Err(Error::InvalidArgument),
    }
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
            Some(node) => { if flags & TRUNCATE != 0 { fs.change(node, now, |content, room| resize(content, 0, room))?; } node }
            None if flags & CREATE == 0 => return Err(Error::NotFound),
            None if place.slash => return Err(Error::IsDirectory),
            None => fs.create(&place, mode, Body::File(Vec::new()), now)?,
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
        let content = fs.content(node)?;
        let rest = usize::try_from(offset).ok().and_then(|start| content.get(start..)).unwrap_or(&[]);
        let count = rest.len().min(capacity);
        unsafe { ptr::copy_nonoverlapping(rest.as_ptr(), out, count) };
        Ok(count)
    }
    unsafe fn write_at(file: *mut c_void, offset: u64, data: *const u8, size: usize) -> Result<usize> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, WRITE) }?;
        let data = unsafe { slice::from_raw_parts(data, size) };
        fs.change(node, now, |content, room| write(content, offset, data, room))
    }
    unsafe fn set_size(file: *mut c_void, size: u64) -> Result<()> {
        let now = clock();
        let mut fs = Guard::acquire();
        let node = unsafe { file_node(&mut fs, file, WRITE) }?;
        fs.change(node, now, |content, room| resize(content, size, room))
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
        fs.unlinked(node, now)
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
        if let Some(replaced) = replaced { fs.unlinked(replaced, now)?; }
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
        fs.unlinked(node, now)
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
        fs.entries_mut(place.parent)?.insert(place.name[..].into(), node);
        let linked = fs.node_mut(node)?;
        (linked.links, linked.changed) = (linked.links + 1, now);
        fs.touch(place.parent, now)
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

/// The tree is one volume mounted at `/`: its capacity is [`set_capacity`]'s, and what
/// file content does not occupy is free to every caller.
impl Volumes for MemFs {
    unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
        if index != 0 { return Err(Error::NotFound); }
        // SAFETY: a writable buffer of `capacity` bytes is the caller's contract.
        if capacity >= 2 { unsafe { out.copy_from_nonoverlapping(b"/\0".as_ptr(), 2) }; }
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
