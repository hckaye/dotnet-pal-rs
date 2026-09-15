//! Bounded, reusable linear-storage backend. Never advertises CAP_VM.
//! All metadata is protected by LOCK; raw payload access is the caller's responsibility.
//! No allocator, host import, memory.grow, inaccessible pages or physical decommit.
use crate::{OK, INVALID_ARGUMENT, OUT_OF_MEMORY};
use core::{cell::UnsafeCell, ffi::c_void, hint, sync::atomic::{AtomicBool, Ordering}};

pub const GRANULARITY: usize = 4096;
#[cfg(not(feature = "linear-gc"))]
pub const CAPACITY: usize = 8 * 1024 * 1024;
#[cfg(feature = "linear-gc")]
pub const CAPACITY: usize = 256 * 1024 * 1024;
const BLOCKS: usize = CAPACITY / GRANULARITY;
const TAIL: usize = usize::MAX;
#[repr(C, align(65536))]
struct Bytes([u8; CAPACITY]);
struct Arena {
    bytes: UnsafeCell<Bytes>,
    slots: UnsafeCell<[usize; BLOCKS]>,
    lock: AtomicBool,
}
// SAFETY: metadata is accessed only under LOCK. Returned payload pointers are raw;
// the unsafe C ABI requires users to serialize payload access with zero/release.
unsafe impl Sync for Arena {}
static ARENA: Arena = Arena {
    bytes: UnsafeCell::new(Bytes([0; CAPACITY])),
    slots: UnsafeCell::new([0; BLOCKS]),
    lock: AtomicBool::new(false),
};
struct Guard;
impl Guard {
    fn acquire() -> Self {
        while ARENA.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            hint::spin_loop();
        }
        Self
    }
    fn slots(&mut self) -> &mut [usize; BLOCKS] {
        unsafe { &mut *ARENA.slots.get() }
    }
}
impl Drop for Guard {
    fn drop(&mut self) { ARENA.lock.store(false, Ordering::Release); }
}
fn base() -> *mut u8 { ARENA.bytes.get().cast::<u8>() }

pub unsafe fn allocate(size: usize, alignment: usize, out: *mut *mut c_void) -> u32 {
    let count = size / GRANULARITY;
    if count == 0 || count > BLOCKS { return OUT_OF_MEMORY; }
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
                unsafe { address.write_bytes(0, size); out.write(address.cast()); }
                return OK;
            }
        }
    }
    OUT_OF_MEMORY
}

pub unsafe fn release(address: *mut c_void, size: usize) -> u32 {
    let Some(offset) = (address as usize).checked_sub(base() as usize) else { return INVALID_ARGUMENT; };
    if offset >= CAPACITY || offset % GRANULARITY != 0 { return INVALID_ARGUMENT; }
    let first = offset / GRANULARITY;
    let mut guard = Guard::acquire();
    let slots = guard.slots();
    let count = slots[first];
    if count == 0 || count == TAIL || count != size / GRANULARITY { return INVALID_ARGUMENT; }
    slots[first..first + count].fill(0);
    OK
}

pub unsafe fn zero(address: *mut c_void, size: usize) -> u32 {
    let Some(offset) = (address as usize).checked_sub(base() as usize) else { return INVALID_ARGUMENT; };
    let Some(end) = offset.checked_add(size) else { return INVALID_ARGUMENT; };
    if size == 0 || end > CAPACITY { return INVALID_ARGUMENT; }
    let mut guard = Guard::acquire();
    let slots = guard.slots();
    let mut first = offset / GRANULARITY;
    while slots[first] == TAIL {
        if first == 0 { return INVALID_ARGUMENT; }
        first -= 1;
    }
    let count = slots[first];
    if count == 0 || end > (first + count) * GRANULARITY { return INVALID_ARGUMENT; }
    unsafe { base().add(offset).write_bytes(0, size) };
    OK
}

#[cfg(not(test))]
pub unsafe fn abort() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();
    #[cfg(not(target_arch = "wasm32"))]
    loop { hint::spin_loop(); }
}
