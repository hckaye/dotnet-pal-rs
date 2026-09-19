#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
//! OS-independent linear-storage providers. None of them advertises `CAP_VM`.
//!
//! * [`Arena`]: a bounded static arena, eagerly accessible, reusable, no host import.
//! * [`Ledger`]: an ownership ledger over embedder-supplied storage hooks with a
//!   hard live-byte budget (`CAP_DYNAMIC_LINEAR`).
use dotnet_pal_rs::port::{Error, LinearStorage, Result};
use core::{cell::UnsafeCell, ffi::c_void, hint, sync::atomic::{AtomicBool, Ordering}};

pub const GRANULARITY: usize = 4096;
#[cfg(not(feature = "linear-gc"))]
pub const CAPACITY: usize = 8 * 1024 * 1024;
#[cfg(all(feature = "linear-gc", not(feature = "linear-gc-small")))]
pub const CAPACITY: usize = 256 * 1024 * 1024;
#[cfg(feature = "linear-gc-small")]
pub const CAPACITY: usize = 64 * 1024 * 1024;

struct SpinLock(AtomicBool);
impl SpinLock {
    const fn new() -> Self { Self(AtomicBool::new(false)) }
    fn acquire(&self) {
        while self.0.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() { hint::spin_loop(); }
    }
    fn release(&self) { self.0.store(false, Ordering::Release); }
}

/// Bounded, reusable static arena. Metadata is protected by a spin lock; raw
/// payload access is the caller's responsibility. No allocator, host import,
/// `memory.grow`, inaccessible pages or physical decommit.
#[cfg(feature = "storage-arena")]
pub struct Arena;
#[cfg(feature = "storage-arena")]
mod arena {
    use super::*;
    const BLOCKS: usize = CAPACITY / GRANULARITY;
    const TAIL: usize = usize::MAX;
    #[repr(C, align(65536))]
    struct Bytes([u8; CAPACITY]);
    struct State { bytes: UnsafeCell<Bytes>, slots: UnsafeCell<[usize; BLOCKS]>, lock: SpinLock }
    // SAFETY: metadata is accessed only under LOCK. Returned payload pointers are raw;
    // the unsafe C ABI requires users to serialize payload access with zero/release.
    unsafe impl Sync for State {}
    static ARENA: State = State {
        bytes: UnsafeCell::new(Bytes([0; CAPACITY])),
        slots: UnsafeCell::new([0; BLOCKS]),
        lock: SpinLock::new(),
    };
    struct Guard;
    impl Guard {
        fn acquire() -> Self { ARENA.lock.acquire(); Self }
        fn slots(&mut self) -> &mut [usize; BLOCKS] { unsafe { &mut *ARENA.slots.get() } }
    }
    impl Drop for Guard { fn drop(&mut self) { ARENA.lock.release(); } }
    fn base() -> *mut u8 { ARENA.bytes.get().cast::<u8>() }
    impl LinearStorage for Arena {
        const GRANULARITY: usize = GRANULARITY;
        const CAPACITY: usize = CAPACITY;
        unsafe fn allocate(size: usize, alignment: usize) -> Result<*mut c_void> {
            let count = size / GRANULARITY;
            if count == 0 || count > BLOCKS { return Err(Error::OutOfMemory); }
            let mut guard = Guard::acquire();
            let slots = guard.slots();
            let mut first = 0;
            while first <= BLOCKS - count {
                let address = unsafe { base().add(first * GRANULARITY) };
                if (address as usize) & (alignment - 1) != 0 { first += 1; continue; }
                match slots[first..first + count].iter().position(|&n| n != 0) {
                    Some(occupied) => { first += occupied + 1; }
                    None => {
                        slots[first] = count;
                        slots[first + 1..first + count].fill(TAIL);
                        unsafe { address.write_bytes(0, size); }
                        return Ok(address.cast());
                    }
                }
            }
            Err(Error::OutOfMemory)
        }
        unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
            let Some(offset) = (address as usize).checked_sub(base() as usize) else { return Err(Error::InvalidArgument); };
            if offset >= CAPACITY || offset % GRANULARITY != 0 { return Err(Error::InvalidArgument); }
            let first = offset / GRANULARITY;
            let mut guard = Guard::acquire();
            let slots = guard.slots();
            let count = slots[first];
            if count == 0 || count == TAIL || count != size / GRANULARITY { return Err(Error::InvalidArgument); }
            slots[first..first + count].fill(0);
            Ok(())
        }
        unsafe fn zero(address: *mut c_void, size: usize) -> Result<()> {
            let Some(offset) = (address as usize).checked_sub(base() as usize) else { return Err(Error::InvalidArgument); };
            let Some(end) = offset.checked_add(size) else { return Err(Error::InvalidArgument); };
            if size == 0 || end > CAPACITY { return Err(Error::InvalidArgument); }
            let mut guard = Guard::acquire();
            let slots = guard.slots();
            let mut first = offset / GRANULARITY;
            while slots[first] == TAIL {
                if first == 0 { return Err(Error::InvalidArgument); }
                first -= 1;
            }
            let count = slots[first];
            if count == 0 || end > (first + count) * GRANULARITY { return Err(Error::InvalidArgument); }
            unsafe { base().add(offset).write_bytes(0, size) };
            Ok(())
        }
    }
}

/// Provider of exclusive storage blocks for [`Ledger`].
pub trait Backing {
    /// Uninitialized exclusive storage aligned to `alignment`; fails before side effects.
    unsafe fn allocate(size: usize, alignment: usize) -> Result<*mut c_void>;
    /// All-or-nothing; must preserve the storage on failure.
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()>;
}
/// Demand-allocated linear storage with a hard live-byte budget and ownership ledger.
/// The backing must coordinate with any other allocator in the same memory.
/// Only a fixed 4,096-entry ledger is static; payload arrives from `B` on demand.
pub struct Ledger<B: Backing>(core::marker::PhantomData<B>);
mod ledger {
    use super::*;
    const MAX_REGIONS: usize = 4096;
    #[derive(Clone, Copy)]
    struct Region { base: usize, size: usize }
    const EMPTY: Region = Region { base: 0, size: 0 };
    struct State { regions: [Region; MAX_REGIONS], owned: usize }
    struct Shared { state: UnsafeCell<State>, lock: SpinLock }
    // Only Guard creates references to metadata. No reference covers host storage.
    unsafe impl Sync for Shared {}
    static LEDGER: Shared = Shared {
        state: UnsafeCell::new(State { regions: [EMPTY; MAX_REGIONS], owned: 0 }),
        lock: SpinLock::new(),
    };
    struct Guard;
    impl Guard {
        fn acquire() -> Self { LEDGER.lock.acquire(); Self }
        fn state(&mut self) -> &mut State { unsafe { &mut *LEDGER.state.get() } }
    }
    impl Drop for Guard { fn drop(&mut self) { LEDGER.lock.release(); } }
    fn normalize(status: Error) -> Error {
        match status { Error::InvalidArgument | Error::Os | Error::OutOfMemory => status, _ => Error::Os }
    }
    impl<B: Backing> LinearStorage for Ledger<B> {
        const DYNAMIC: bool = true;
        const GRANULARITY: usize = GRANULARITY;
        const CAPACITY: usize = CAPACITY;
        unsafe fn allocate(size: usize, alignment: usize) -> Result<*mut c_void> {
            let mut guard = Guard::acquire();
            let state = guard.state();
            if size == 0 || size > CAPACITY - state.owned { return Err(Error::OutOfMemory); }
            let Some(index) = state.regions.iter().position(|r| r.size == 0) else { return Err(Error::OutOfMemory); };
            let address = unsafe { B::allocate(size, alignment) }.map_err(normalize)?;
            let base = address as usize;
            let Some(end) = base.checked_add(size) else { return Err(Error::Os); };
            if base == 0 || base % alignment != 0 { return Err(Error::Os); }
            if state.regions.iter().any(|r| r.size != 0 && base < r.base + r.size && r.base < end) {
                // A broken provider returned someone else's storage. Never free/overwrite it.
                return Err(Error::Os);
            }
            // SAFETY: a successful provider grants exclusive writable storage for size bytes.
            unsafe { address.cast::<u8>().write_bytes(0, size); }
            state.regions[index] = Region { base, size };
            state.owned += size;
            Ok(address)
        }
        unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
            let mut guard = Guard::acquire();
            let state = guard.state();
            let Some(index) = state.regions.iter().position(|r| r.size != 0 && r.base == address as usize && r.size == size) else {
                return Err(Error::InvalidArgument);
            };
            unsafe { B::release(address, size) }.map_err(normalize)?;
            state.regions[index] = EMPTY;
            state.owned -= size;
            Ok(())
        }
        unsafe fn zero(address: *mut c_void, size: usize) -> Result<()> {
            let base = address as usize;
            let Some(end) = base.checked_add(size) else { return Err(Error::InvalidArgument); };
            if base == 0 || size == 0 { return Err(Error::InvalidArgument); }
            let mut guard = Guard::acquire();
            if !guard.state().regions.iter().any(|r| r.size != 0 && base >= r.base && end <= r.base + r.size) {
                return Err(Error::InvalidArgument);
            }
            unsafe { address.cast::<u8>().write_bytes(0, size); }
            Ok(())
        }
    }
}

/// Backing supplied by the embedder through the versioned C hooks in `dotnet_pal.h`.
/// On WASI this shares wasi-libc's allocator with the runtime and BCL.
#[cfg(feature = "storage-hooks")]
pub struct Hooks;
#[cfg(feature = "storage-hooks")]
impl Backing for Hooks {
    unsafe fn allocate(size: usize, alignment: usize) -> Result<*mut c_void> {
        extern "C" { fn dotnet_pal_storage_allocate_v2(size: usize, alignment: usize, out: *mut *mut c_void) -> u32; }
        let mut address = core::ptr::null_mut();
        dotnet_pal_rs::port::from_status(unsafe { dotnet_pal_storage_allocate_v2(size, alignment, &mut address) })?;
        Ok(address)
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        extern "C" { fn dotnet_pal_storage_release_v2(address: *mut c_void, size: usize) -> u32; }
        dotnet_pal_rs::port::from_status(unsafe { dotnet_pal_storage_release_v2(address, size) })
    }
}
