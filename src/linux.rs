//! Reference backend, not a generic Unix or console implementation.
use crate::{OK, OS_ERROR};
use core::{ffi::c_void, ptr};

pub fn page_size() -> usize {
    let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if value > 0 { value as usize } else { 0 }
}

pub unsafe fn reserve(size: usize, alignment: usize, out: *mut *mut c_void) -> u32 {
    let page = page_size();
    let Some(total) = size.checked_add(alignment - page) else { return OS_ERROR; };
    let raw = unsafe { libc::mmap(ptr::null_mut(), total, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
    if raw == libc::MAP_FAILED { return OS_ERROR; }
    let Some(rounded) = (raw as usize).checked_add(alignment - 1) else {
        unsafe { libc::munmap(raw, total) }; return OS_ERROR;
    };
    let aligned = rounded & !(alignment - 1);
    let prefix = aligned - raw as usize;
    let suffix = total - prefix - size;
    if prefix != 0 && unsafe { libc::munmap(raw, prefix) } != 0 {
        unsafe { libc::munmap(raw, total) }; return OS_ERROR;
    }
    if suffix != 0 && unsafe { libc::munmap((aligned + size) as *mut c_void, suffix) } != 0 {
        unsafe { libc::munmap(aligned as *mut c_void, size + suffix) }; return OS_ERROR;
    }
    // Initial PROT_NONE mapping owns the address range but not a physical-memory promise.
    unsafe { out.write(aligned as *mut c_void) };
    OK
}

pub unsafe fn commit(address: *mut c_void, size: usize) -> u32 {
    // Re-committing already committed pages preserves data.
    status(unsafe { libc::mprotect(address, size, libc::PROT_READ | libc::PROT_WRITE) })
}

pub unsafe fn decommit(address: *mut c_void, size: usize) -> u32 {
    // Replace ONLY a caller-owned reservation. Fresh anonymous pages guarantee
    // zeroes after recommit; mprotect alone would leave the old contents behind.
    let result = unsafe { libc::mmap(address, size, libc::PROT_NONE,
        libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
    if result == libc::MAP_FAILED { OS_ERROR } else { OK }
}

pub unsafe fn release(address: *mut c_void, size: usize) -> u32 {
    status(unsafe { libc::munmap(address, size) })
}

pub unsafe fn reset(address: *mut c_void, size: usize) -> u32 {
    // Contents may be discarded, but the pages stay accessible. Distinct from decommit.
    status(unsafe { libc::madvise(address, size, libc::MADV_DONTNEED) })
}

fn status(result: i32) -> u32 { if result == 0 { OK } else { OS_ERROR } }
pub unsafe fn abort() -> ! { unsafe { libc::abort() } }
