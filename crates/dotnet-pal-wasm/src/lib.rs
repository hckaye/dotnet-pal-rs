#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(target_arch = "wasm32")]
//! Single-owner Wasm memory growth. Never combine it with another allocator.
use dotnet_pal_rs::port::{Error, Result};
use dotnet_pal_storage::Backing;
use core::{cell::UnsafeCell, ffi::c_void};

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
