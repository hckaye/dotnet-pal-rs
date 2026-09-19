//! Events, mutexes and reader/writer locks over the cooperative scheduler.
//!
//! All three live in fixed tables and are handed out as the address of their
//! entry, so creating one allocates nothing. Contention is resolved by the
//! waiting thread polling the object and giving the CPU to the next thread; no
//! wake-up list is needed because a single core cannot have a waiter running
//! while another thread holds the object.
//!
//! Destroying an object other threads are still inside is `Error::Busy`, which
//! is the status the boundary's kernel front end expects for that mistake.
use crate::sync::Single;
use crate::{clock, thread, Baremetal};
use core::{ffi::c_void, mem};
use dotnet_pal_rs::kernel::INFINITE;
use dotnet_pal_rs::port::{self, Error, Result};

const MAX_EVENTS: usize = 64;
const MAX_MUTEXES: usize = 64;
const MAX_RWLOCKS: usize = 32;
const NOBODY: usize = usize::MAX;

fn deadline_for(timeout_ns: u64) -> Option<u64> {
    if timeout_ns == INFINITE {
        None
    } else {
        Some(clock::monotonic_ns().unwrap_or(0).saturating_add(timeout_ns))
    }
}
/// Index of a handle in a static table, or `None` when it is not one of ours.
fn index_of<T>(table: &[T], handle: *mut c_void) -> Option<usize> {
    let base = table.as_ptr() as usize;
    let address = handle as usize;
    let size = mem::size_of::<T>();
    if address < base || (address - base) % size != 0 {
        return None;
    }
    let index = (address - base) / size;
    (index < table.len()).then_some(index)
}

#[derive(Clone, Copy)]
struct Event {
    used: bool,
    manual: bool,
    signaled: bool,
    waiters: usize,
}
static EVENTS: Single<[Event; MAX_EVENTS]> =
    Single::new([Event { used: false, manual: false, signaled: false, waiters: 0 }; MAX_EVENTS]);
fn events() -> &'static mut [Event; MAX_EVENTS] {
    // SAFETY: single core, non-preemptive; see `sync`.
    unsafe { EVENTS.get() }
}
impl port::Events for Baremetal {
    unsafe fn create(manual_reset: bool, initially_set: bool) -> Result<*mut c_void> {
        crate::trace(b"event create", manual_reset as u64);
        crate::trace(b"event create", manual_reset as u64);
        let Some(index) = (0..MAX_EVENTS).find(|&i| !events()[i].used) else {
            return Err(Error::OutOfMemory);
        };
        events()[index] =
            Event { used: true, manual: manual_reset, signaled: initially_set, waiters: 0 };
        Ok(core::ptr::addr_of_mut!(events()[index]).cast())
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let index = index_of(events(), handle).ok_or(Error::InvalidArgument)?;
        if !events()[index].used {
            return Err(Error::InvalidArgument);
        }
        if events()[index].waiters != 0 {
            return Err(Error::Busy);
        }
        events()[index] = Event { used: false, manual: false, signaled: false, waiters: 0 };
        Ok(())
    }
    unsafe fn set(handle: *mut c_void) -> Result<()> {
        let index = index_of(events(), handle).ok_or(Error::InvalidArgument)?;
        if !events()[index].used {
            return Err(Error::InvalidArgument);
        }
        // Setting a set event changes nothing: this is not a counting semaphore.
        events()[index].signaled = true;
        Ok(())
    }
    unsafe fn reset(handle: *mut c_void) -> Result<()> {
        let index = index_of(events(), handle).ok_or(Error::InvalidArgument)?;
        if !events()[index].used {
            return Err(Error::InvalidArgument);
        }
        events()[index].signaled = false;
        Ok(())
    }
    /// An auto-reset event is consumed by the first waiter that observes it, so
    /// one `set` releases exactly one of several waiting threads.
    unsafe fn wait(handle: *mut c_void, timeout_ns: u64) -> Result<()> {
        let index = index_of(events(), handle).ok_or(Error::InvalidArgument)?;
        if !events()[index].used {
            return Err(Error::InvalidArgument);
        }
        events()[index].waiters += 1;
        let outcome = thread::block_until(deadline_for(timeout_ns), || {
            let event = &mut events()[index];
            if !event.signaled {
                return false;
            }
            if !event.manual {
                event.signaled = false;
            }
            true
        });
        events()[index].waiters -= 1;
        outcome
    }
}

#[derive(Clone, Copy)]
struct Mutex {
    used: bool,
    recursive: bool,
    owner: usize,
    depth: usize,
    /// Holder plus waiters: destroying a mutex anyone is inside is `Busy`.
    users: usize,
}
const FREE_MUTEX: Mutex =
    Mutex { used: false, recursive: false, owner: NOBODY, depth: 0, users: 0 };
static MUTEXES: Single<[Mutex; MAX_MUTEXES]> = Single::new([FREE_MUTEX; MAX_MUTEXES]);
fn mutexes() -> &'static mut [Mutex; MAX_MUTEXES] {
    // SAFETY: single core, non-preemptive; see `sync`.
    unsafe { MUTEXES.get() }
}
impl port::Mutexes for Baremetal {
    unsafe fn create(recursive: bool) -> Result<*mut c_void> {
        crate::trace(b"mutex create", recursive as u64);
        crate::trace(b"mutex create", recursive as u64);
        let Some(index) = (0..MAX_MUTEXES).find(|&i| !mutexes()[i].used) else {
            return Err(Error::OutOfMemory);
        };
        mutexes()[index] = Mutex { used: true, recursive, owner: NOBODY, depth: 0, users: 0 };
        Ok(core::ptr::addr_of_mut!(mutexes()[index]).cast())
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let index = index_of(mutexes(), handle).ok_or(Error::InvalidArgument)?;
        if !mutexes()[index].used {
            return Err(Error::InvalidArgument);
        }
        if mutexes()[index].users != 0 {
            return Err(Error::Busy);
        }
        mutexes()[index] = FREE_MUTEX;
        Ok(())
    }
    /// Relocking from the owning thread is a recursion when the mutex was created
    /// recursive and an error otherwise: waiting for oneself cannot end well on a
    /// single core, and reporting it beats hanging the machine.
    unsafe fn lock(handle: *mut c_void) -> Result<()> {
        let index = index_of(mutexes(), handle).ok_or(Error::InvalidArgument)?;
        let me = thread::current();
        let mutex = &mut mutexes()[index];
        if !mutex.used {
            return Err(Error::InvalidArgument);
        }
        if mutex.owner == me {
            if !mutex.recursive || mutex.depth == usize::MAX {
                return Err(Error::Os);
            }
            mutex.depth += 1;
            mutex.users += 1;
            return Ok(());
        }
        mutex.users += 1;
        thread::block_until(None, || mutexes()[index].owner == NOBODY)?;
        let mutex = &mut mutexes()[index];
        mutex.owner = me;
        mutex.depth = 1;
        Ok(())
    }
    unsafe fn unlock(handle: *mut c_void) -> Result<()> {
        let index = index_of(mutexes(), handle).ok_or(Error::InvalidArgument)?;
        let mutex = &mut mutexes()[index];
        if !mutex.used || mutex.owner != thread::current() {
            return Err(Error::Os);
        }
        mutex.depth -= 1;
        mutex.users -= 1;
        if mutex.depth == 0 {
            mutex.owner = NOBODY;
        }
        Ok(())
    }
}

/// Readers are counted per thread so that `unlock` knows which side of the lock
/// the caller is on without the caller saying.
#[derive(Clone, Copy)]
struct RwLock {
    used: bool,
    writer: usize,
    readers: usize,
    held: [u8; thread::MAX_THREADS],
}
const FREE_RWLOCK: RwLock = RwLock {
    used: false,
    writer: NOBODY,
    readers: 0,
    held: [0; thread::MAX_THREADS],
};
static RWLOCKS: Single<[RwLock; MAX_RWLOCKS]> = Single::new([FREE_RWLOCK; MAX_RWLOCKS]);
fn rwlocks() -> &'static mut [RwLock; MAX_RWLOCKS] {
    // SAFETY: single core, non-preemptive; see `sync`.
    unsafe { RWLOCKS.get() }
}
impl port::RwLocks for Baremetal {
    unsafe fn create() -> Result<*mut c_void> {
        crate::trace(b"rwlock create", 0);
        crate::trace(b"rwlock create", 0);
        let Some(index) = (0..MAX_RWLOCKS).find(|&i| !rwlocks()[i].used) else {
            return Err(Error::OutOfMemory);
        };
        rwlocks()[index] = RwLock { used: true, ..FREE_RWLOCK };
        Ok(core::ptr::addr_of_mut!(rwlocks()[index]).cast())
    }
    /// Readers wait only for a writer. A writer that arrives while readers keep
    /// taking the lock can be starved; nothing in this port promises fairness.
    unsafe fn read(handle: *mut c_void) -> Result<()> {
        let index = index_of(rwlocks(), handle).ok_or(Error::InvalidArgument)?;
        if !rwlocks()[index].used {
            return Err(Error::InvalidArgument);
        }
        let me = thread::current();
        thread::block_until(None, || rwlocks()[index].writer == NOBODY)?;
        let lock = &mut rwlocks()[index];
        lock.readers += 1;
        lock.held[me] += 1;
        Ok(())
    }
    unsafe fn write(handle: *mut c_void) -> Result<()> {
        let index = index_of(rwlocks(), handle).ok_or(Error::InvalidArgument)?;
        if !rwlocks()[index].used {
            return Err(Error::InvalidArgument);
        }
        let me = thread::current();
        thread::block_until(None, || {
            let lock = &rwlocks()[index];
            lock.writer == NOBODY && lock.readers == 0
        })?;
        rwlocks()[index].writer = me;
        Ok(())
    }
    unsafe fn unlock(handle: *mut c_void) -> Result<()> {
        let index = index_of(rwlocks(), handle).ok_or(Error::InvalidArgument)?;
        let me = thread::current();
        let lock = &mut rwlocks()[index];
        if !lock.used {
            return Err(Error::InvalidArgument);
        }
        if lock.writer == me {
            lock.writer = NOBODY;
        } else if lock.held[me] != 0 {
            lock.held[me] -= 1;
            lock.readers -= 1;
        } else {
            return Err(Error::Os); // the caller does not hold this lock
        }
        Ok(())
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let index = index_of(rwlocks(), handle).ok_or(Error::InvalidArgument)?;
        let lock = &rwlocks()[index];
        if !lock.used {
            return Err(Error::InvalidArgument);
        }
        if lock.writer != NOBODY || lock.readers != 0 {
            return Err(Error::Busy);
        }
        rwlocks()[index] = FREE_RWLOCK;
        Ok(())
    }
}
