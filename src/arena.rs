//! Reference single-instance, single-threaded WASM linear-memory provider.
//! Logical reservations only: never advertises CAP_VM's native protection model.
use crate::{INVALID_ARGUMENT, OK, OS_ERROR};
use core::{ffi::c_void, ptr};

const CAPACITY: usize = 64;
#[derive(Clone, Copy)]
struct Region { base: usize, size: usize }
const EMPTY: Region = Region { base: 0, size: 0 };
struct State { base: usize, size: usize, page: usize, regions: [Region; CAPACITY] }
static mut STATE: State = State { base: 0, size: 0, page: 0, regions: [EMPTY; CAPACITY] };

pub unsafe fn initialize(base: *mut c_void, size: usize, page: usize) -> u32 {
    // SAFETY: this backend is confined to serialized, non-shared WASM instances.
    let s = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if s.page != 0 || base.is_null() || !page.is_power_of_two()
        || size == 0 || size > isize::MAX as usize || size % page != 0
        || (base as usize) % page != 0 || (base as usize).checked_add(size).is_none()
    { return INVALID_ARGUMENT; }
    s.base = base as usize;
    s.size = size;
    s.page = page;
    OK
}

pub fn page_size() -> usize {
    unsafe { (*ptr::addr_of!(STATE)).page }
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value.checked_add(alignment - 1).map(|n| n & !(alignment - 1))
}

pub unsafe fn reserve(size: usize, alignment: usize, out: *mut *mut c_void) -> u32 {
    let s = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    let Some(slot) = s.regions.iter().position(|r| r.size == 0) else { return OS_ERROR; };
    let Some(mut base) = align_up(s.base, alignment) else { return OS_ERROR; };
    loop {
        let Some(end) = base.checked_add(size).filter(|end| *end <= s.base + s.size)
            else { return OS_ERROR; };
        if let Some(r) = s.regions.iter().find(|r| r.size != 0 && base < r.base + r.size && r.base < end) {
            let Some(next) = align_up(r.base + r.size, alignment) else { return OS_ERROR; };
            base = next;
        } else {
            // SAFETY: a free subrange of the host-owned, nonmoving region.
            unsafe { ptr::write_bytes(base as *mut u8, 0, size); }
            s.regions[slot] = Region { base, size };
            unsafe { out.write(base as *mut c_void); }
            return OK;
        }
    }
}

fn contains(s: &State, address: *mut c_void, size: usize) -> bool {
    let base = address as usize;
    let Some(end) = base.checked_add(size) else { return false; };
    s.regions.iter().any(|r| r.size != 0 && base >= r.base && end <= r.base + r.size)
}

pub unsafe fn commit(address: *mut c_void, size: usize) -> u32 {
    let s = unsafe { &*ptr::addr_of!(STATE) };
    if contains(s, address, size) { OK } else { INVALID_ARGUMENT }
}

pub unsafe fn decommit(address: *mut c_void, size: usize) -> u32 {
    let s = unsafe { &*ptr::addr_of!(STATE) };
    if !contains(s, address, size) { return INVALID_ARGUMENT; }
    // No trap, mprotect, physical release, or memory.grow/shrink is implied.
    unsafe { ptr::write_bytes(address.cast::<u8>(), 0, size); }
    OK
}

pub unsafe fn release(address: *mut c_void, size: usize) -> u32 {
    let s = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    match s.regions.iter_mut().find(|r| r.size != 0 && r.base == address as usize && r.size == size) {
        Some(r) => { *r = EMPTY; OK }
        None => INVALID_ARGUMENT,
    }
}

pub unsafe fn reset(address: *mut c_void, size: usize) -> u32 {
    // Reset is an advisory discard hint, with no guarantee of changed bytes.
    unsafe { commit(address, size) }
}

pub unsafe fn abort() -> ! { core::arch::wasm32::unreachable() }
