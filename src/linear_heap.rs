//! Demand-allocated linear storage with a hard live-byte budget and ownership ledger.
//! The backend must coordinate with any other native allocator in the same memory.
//! In particular, this module NEVER calls Wasm memory.grow independently of libc.
use crate::{OK, INVALID_ARGUMENT, OS_ERROR, OUT_OF_MEMORY};
use core::{cell::UnsafeCell, ffi::c_void, hint, ptr, sync::atomic::{AtomicBool, Ordering}};

pub const GRANULARITY: usize = 4096;
#[cfg(not(feature="linear-gc"))]
pub const CAPACITY: usize = 8 * 1024 * 1024;
#[cfg(all(feature="linear-gc", not(feature="linear-gc-small")))]
pub const CAPACITY: usize = 256 * 1024 * 1024;
#[cfg(feature="linear-gc-small")]
pub const CAPACITY: usize = 64 * 1024 * 1024;
const MAX_REGIONS: usize = 4096;
#[derive(Clone, Copy)]
struct Region { base: usize, size: usize }
const EMPTY: Region = Region { base: 0, size: 0 };
struct State { regions: [Region; MAX_REGIONS], owned: usize }
struct Ledger { state: UnsafeCell<State>, lock: AtomicBool }
// Only Guard creates references to metadata. No reference covers host storage.
unsafe impl Sync for Ledger {}
static LEDGER: Ledger = Ledger {
    state: UnsafeCell::new(State { regions: [EMPTY; MAX_REGIONS], owned: 0 }),
    lock: AtomicBool::new(false),
};
struct Guard;
impl Guard {
    fn acquire() -> Self {
        while LEDGER.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            hint::spin_loop();
        }
        Self
    }
    fn state(&mut self) -> &mut State { unsafe { &mut *LEDGER.state.get() } }
}
impl Drop for Guard { fn drop(&mut self) { LEDGER.lock.store(false, Ordering::Release); } }
extern "C" {
    fn dotnet_pal_storage_allocate_v2(size: usize, alignment: usize, out: *mut *mut c_void) -> u32;
    fn dotnet_pal_storage_release_v2(address: *mut c_void, size: usize) -> u32;
}
fn normalize(status: u32) -> u32 {
    match status { OK | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY => status, _ => OS_ERROR }
}
pub unsafe fn allocate(size: usize, alignment: usize, out: *mut *mut c_void) -> u32 {
    let mut guard = Guard::acquire();
    let state = guard.state();
    if size == 0 || size > CAPACITY - state.owned { return OUT_OF_MEMORY; }
    let Some(index) = state.regions.iter().position(|r| r.size == 0) else { return OUT_OF_MEMORY; };
    let mut address = ptr::null_mut();
    let status = normalize(unsafe { dotnet_pal_storage_allocate_v2(size, alignment, &mut address) });
    if status != OK { return status; }
    let base = address as usize;
    let Some(end) = base.checked_add(size) else { return OS_ERROR; };
    if base == 0 || base % alignment != 0 { return OS_ERROR; }
    if state.regions.iter().any(|r| r.size != 0 && base < r.base + r.size && r.base < end) {
        // A broken provider returned someone else's storage. Never free/overwrite it.
        return OS_ERROR;
    }
    // SAFETY: a successful provider grants exclusive writable storage for size bytes.
    unsafe { address.cast::<u8>().write_bytes(0, size); }
    state.regions[index] = Region { base, size };
    state.owned += size;
    unsafe { out.write(address); }
    OK
}
pub unsafe fn release(address: *mut c_void, size: usize) -> u32 {
    let mut guard = Guard::acquire();
    let state = guard.state();
    let Some(index) = state.regions.iter().position(|r| r.size != 0 && r.base == address as usize && r.size == size) else {
        return INVALID_ARGUMENT;
    };
    let status = normalize(unsafe { dotnet_pal_storage_release_v2(address, size) });
    if status == OK {
        state.regions[index] = EMPTY;
        state.owned -= size;
    }
    status
}
pub unsafe fn zero(address: *mut c_void, size: usize) -> u32 {
    let base = address as usize;
    let Some(end) = base.checked_add(size) else { return INVALID_ARGUMENT; };
    if base == 0 || size == 0 { return INVALID_ARGUMENT; }
    let mut guard = Guard::acquire();
    if !guard.state().regions.iter().any(|r| r.size != 0 && base >= r.base && end <= r.base + r.size) {
        return INVALID_ARGUMENT;
    }
    unsafe { address.cast::<u8>().write_bytes(0, size); }
    OK
}
#[cfg(not(test))]
pub unsafe fn abort() -> ! {
    #[cfg(target_arch="wasm32")]
    core::arch::wasm32::unreachable();
    #[cfg(not(target_arch="wasm32"))]
    loop { hint::spin_loop(); }
}
