//! OS-independent linear-storage providers. None of them advertises `CAP_VM`.
//!
//! * [`Arena`]: a bounded static arena, eagerly accessible, reusable, no host import.
//! * [`Ledger`]: an ownership ledger over embedder-supplied storage hooks with a
//!   hard live-byte budget (`CAP_DYNAMIC_LINEAR`).
//! * [`Grow`]: a wasm32 `memory.grow` provider for modules that contain no other
//!   allocator; used through `Ledger` in the `linear-grow` configuration.
use crate::port::{Error, LinearStorage, Result};
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
        crate::port::from_status(unsafe { dotnet_pal_storage_allocate_v2(size, alignment, &mut address) })?;
        Ok(address)
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        extern "C" { fn dotnet_pal_storage_release_v2(address: *mut c_void, size: usize) -> u32; }
        crate::port::from_status(unsafe { dotnet_pal_storage_release_v2(address, size) })
    }
}

/// Module-owned demand storage for wasm32: pages obtained with `memory.grow`
/// are handed out from a sorted, coalescing free list.
///
/// This provider assumes it is the ONLY allocator growing the instance memory.
/// Linking it next to a libc allocator (wasi-libc, Emscripten) would let two
/// owners race for the same pages; such embeddings supply the hooks instead.
/// Freed pages stay mapped: Wasm memory never shrinks.
#[cfg(all(feature = "storage-grow", target_arch = "wasm32"))]
pub struct Grow;
#[cfg(all(feature = "storage-grow", target_arch = "wasm32"))]
mod grow {
    use super::*;
    const PAGE: usize = 65536;
    const MAX_FREE: usize = 512;
    #[derive(Clone, Copy)]
    struct Block { base: usize, size: usize }
    struct FreeList { blocks: [Block; MAX_FREE], count: usize }
    struct State(UnsafeCell<FreeList>);
    // SAFETY: the provider is documented single-threaded (no shared memory) and is
    // always entered under the ledger lock. No reference escapes a call.
    unsafe impl Sync for State {}
    static STATE: State = State(UnsafeCell::new(FreeList { blocks: [Block { base: 0, size: 0 }; MAX_FREE], count: 0 }));
    fn state() -> &'static mut FreeList { unsafe { &mut *STATE.0.get() } }
    fn align_up(value: usize, alignment: usize) -> Option<usize> {
        value.checked_add(alignment - 1).map(|v| v & !(alignment - 1))
    }
    /// Insert a block keeping the list sorted by address and coalescing neighbors.
    /// Fails without side effects when the list is full and nothing merges.
    fn insert(list: &mut FreeList, base: usize, size: usize) -> bool {
        let end = base + size;
        let index = list.blocks[..list.count].iter().position(|b| b.base > base).unwrap_or(list.count);
        let merges_previous = index > 0 && list.blocks[index - 1].base + list.blocks[index - 1].size == base;
        let merges_next = index < list.count && list.blocks[index].base == end;
        match (merges_previous, merges_next) {
            (true, true) => {
                list.blocks[index - 1].size += size + list.blocks[index].size;
                list.blocks.copy_within(index + 1..list.count, index);
                list.count -= 1;
            }
            (true, false) => list.blocks[index - 1].size += size,
            (false, true) => { list.blocks[index].base = base; list.blocks[index].size += size; }
            (false, false) => {
                if list.count == MAX_FREE { return false; }
                list.blocks.copy_within(index..list.count, index + 1);
                list.blocks[index] = Block { base, size };
                list.count += 1;
            }
        }
        true
    }
    /// First fit with alignment. Leading and trailing remainders return to the list.
    fn take(list: &mut FreeList, size: usize, alignment: usize) -> Option<usize> {
        for index in 0..list.count {
            let block = list.blocks[index];
            let Some(start) = align_up(block.base, alignment) else { continue; };
            let Some(end) = start.checked_add(size) else { continue; };
            if end > block.base + block.size { continue; }
            let lead = start - block.base;
            let trail = block.base + block.size - end;
            // Two remainders may need one extra record; refuse instead of losing storage.
            if lead != 0 && trail != 0 && list.count == MAX_FREE { continue; }
            list.blocks.copy_within(index + 1..list.count, index);
            list.count -= 1;
            if lead != 0 { assert!(insert(list, block.base, lead)); }
            if trail != 0 { assert!(insert(list, end, trail)); }
            return Some(start);
        }
        None
    }
    fn grow(list: &mut FreeList, size: usize, alignment: usize) -> Result<()> {
        let current = core::arch::wasm32::memory_size(0).saturating_mul(PAGE);
        let Some(start) = align_up(current, alignment) else { return Err(Error::OutOfMemory); };
        let Some(needed) = (start - current).checked_add(size) else { return Err(Error::OutOfMemory); };
        let pages = needed.div_ceil(PAGE);
        if list.count == MAX_FREE || pages == 0 { return Err(Error::OutOfMemory); }
        let previous = core::arch::wasm32::memory_grow(0, pages);
        if previous == usize::MAX { return Err(Error::OutOfMemory); }
        if previous * PAGE != current { return Err(Error::Os); } // another party grew memory concurrently
        // Cannot fail: an empty slot was checked before growing.
        assert!(insert(list, current, pages * PAGE));
        Ok(())
    }
    impl Backing for Grow {
        unsafe fn allocate(size: usize, alignment: usize) -> Result<*mut c_void> {
            if size == 0 || !alignment.is_power_of_two() { return Err(Error::InvalidArgument); }
            let list = state();
            if let Some(address) = take(list, size, alignment) { return Ok(address as *mut c_void); }
            grow(list, size, alignment)?;
            take(list, size, alignment).map(|a| a as *mut c_void).ok_or(Error::Os)
        }
        unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
            let base = address as usize;
            if base == 0 || size == 0 || base.checked_add(size).is_none() { return Err(Error::InvalidArgument); }
            // The ledger already proved ownership; a full free list is the only failure.
            if insert(state(), base, size) { Ok(()) } else { Err(Error::Os) }
        }
    }
}
