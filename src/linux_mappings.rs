//! Linux provider for the mappings group: mmap, munmap and msync on the descriptor
//! behind a handle of the files group. The kernel enforces the access rule of the
//! contract itself: it maps nothing from a descriptor that is not open for reading,
//! and a shared writable mapping needs one open for writing as well (EACCES both
//! times), while a private writable mapping of a read-only descriptor is granted.
//! No Rust heap, and no state: the kernel keeps the mapping when the handle closes.
use crate::linux::Linux;
use crate::linux_files::descriptor;
use crate::port::{self, Error, Result};
use crate::runtime::{EXECUTE, READ, WRITE};
use core::{ffi::c_void, mem, ptr};

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn is_directory(fd: i32) -> bool {
    let mut value = mem::MaybeUninit::<libc::stat>::uninit();
    unsafe { libc::fstat(fd, value.as_mut_ptr()) == 0 && value.assume_init().st_mode & libc::S_IFMT == libc::S_IFDIR }
}

impl port::Mappings for Linux {
    unsafe fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8> {
        let fd = descriptor(file)?;
        let offset: libc::off_t = offset.try_into().map_err(|_| Error::InvalidArgument)?;
        let protection = (if access & READ != 0 { libc::PROT_READ } else { 0 }) | (if access & WRITE != 0 { libc::PROT_WRITE } else { 0 })
            | (if access & EXECUTE != 0 { libc::PROT_EXEC } else { 0 });
        // SAFETY: the kernel chooses the address, so the call replaces no mapping of the process.
        let address = unsafe { libc::mmap(ptr::null_mut(), length, protection, if shared { libc::MAP_SHARED } else { libc::MAP_PRIVATE }, fd, offset) };
        if address != libc::MAP_FAILED { return Ok(address.cast()); }
        Err(match errno() {
            // EPERM is an executable mapping from a file system mounted noexec, or a write seal.
            libc::EACCES | libc::EPERM => Error::AccessDenied,
            libc::ENOMEM => Error::OutOfMemory,
            // An offset off the page grid, a range the offset type cannot hold, or a handle without a descriptor behind it.
            libc::EINVAL | libc::EOVERFLOW | libc::EBADF => Error::InvalidArgument,
            // A node or a file system that has no mappings. The kernel says the same of a directory, which no handle of the files group is.
            libc::ENODEV => if is_directory(fd) { Error::InvalidArgument } else { Error::Unsupported },
            _ => Error::Os,
        })
    }
    unsafe fn unmap(address: *mut u8, length: usize) -> Result<()> {
        if unsafe { libc::munmap(address.cast(), length) } == 0 { return Ok(()); }
        // ENOMEM: the range is a part of a mapping, and splitting it would exceed the process's number of mappings.
        Err(match errno() { libc::EINVAL => Error::InvalidArgument, libc::ENOMEM => Error::OutOfMemory, _ => Error::Os })
    }
    unsafe fn sync(address: *mut u8, length: usize) -> Result<()> {
        loop {
            if unsafe { libc::msync(address.cast(), length, libc::MS_SYNC) } == 0 { return Ok(()); }
            // ENOMEM is msync's word for a range that is not mapped: not what a map returned.
            match errno() { libc::EINTR => continue, libc::EINVAL | libc::ENOMEM => return Err(Error::InvalidArgument), _ => return Err(Error::Os) }
        }
    }
}
