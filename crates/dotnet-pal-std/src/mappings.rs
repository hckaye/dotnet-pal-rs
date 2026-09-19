//! Files mapped into memory for the desktop port: mmap, munmap and msync on the
//! descriptor of a handle of the files group.
//!
//! The access rule is checked against the access the handle was opened with
//! before the OS is asked, as the files group does for its transfers; Linux and
//! macOS answer the same requests with EACCES themselves. The two differ in what
//! they say of an object without mappings: ENODEV on Linux, on macOS EINVAL for a
//! FIFO, which is the error both have for an offset off the page grid, so the
//! offset is checked here and EINVAL for anything but a regular file is
//! `Unsupported`. macOS also refuses what Linux grants: a mapping of /dev/zero
//! (ENODEV) and an executable mapping of an ordinary file (EPERM, `AccessDenied`).
//! Windows has the facility, but `CreateFileMappingW` sits behind the
//! `Win32_Security` feature of `windows-sys`, which this crate does not enable:
//! every call is `Unsupported` there.
use super::Std;
use dotnet_pal_rs::port::{self, Error, Result};
use std::ffi::c_void;

#[cfg(unix)]
fn failure(code: Option<i32>, regular: impl FnOnce() -> bool) -> Error {
    match code {
        Some(libc::EACCES) | Some(libc::EPERM) => Error::AccessDenied,
        Some(libc::ENOMEM) => Error::OutOfMemory,
        Some(libc::ENODEV) => Error::Unsupported,
        Some(libc::EINVAL) if !regular() => Error::Unsupported,
        Some(libc::EINVAL) | Some(libc::EOVERFLOW) => Error::InvalidArgument,
        _ => Error::Os,
    }
}

impl port::Mappings for Std {
    /// Absent where the port has no implementation: a capability that answers `Unsupported` to every call helps nobody.
    const PROVIDED: bool = cfg!(unix);
    unsafe fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8> {
        #[cfg(unix)]
        {
            use dotnet_pal_rs::{files, runtime::{EXECUTE, READ, WRITE}};
            let (file, flags) = unsafe { super::files::opened(file) }?;
            if flags & files::READ == 0 || (shared && access & WRITE != 0 && flags & files::WRITE == 0) { return Err(Error::AccessDenied); }
            let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).map_err(|_| Error::Os)?;
            if page == 0 || !offset.is_multiple_of(page) { return Err(Error::InvalidArgument); }
            let offset: libc::off_t = offset.try_into().map_err(|_| Error::InvalidArgument)?;
            let protection = (if access & READ != 0 { libc::PROT_READ } else { 0 }) | (if access & WRITE != 0 { libc::PROT_WRITE } else { 0 })
                | (if access & EXECUTE != 0 { libc::PROT_EXEC } else { 0 });
            // SAFETY: the OS chooses the address, so the call replaces no mapping of the process.
            let address = unsafe { libc::mmap(std::ptr::null_mut(), length, protection, if shared { libc::MAP_SHARED } else { libc::MAP_PRIVATE }, std::os::fd::AsRawFd::as_raw_fd(file), offset) };
            if address != libc::MAP_FAILED { return Ok(address.cast()); }
            Err(failure(std::io::Error::last_os_error().raw_os_error(), || file.metadata().is_ok_and(|m| m.is_file())))
        }
        #[cfg(not(unix))]
        { let _ = (file, offset, length, access, shared); Err(Error::Unsupported) }
    }
    unsafe fn unmap(address: *mut u8, length: usize) -> Result<()> {
        #[cfg(unix)]
        {
            if unsafe { libc::munmap(address.cast(), length) } == 0 { return Ok(()); }
            // ENOMEM: the range is a part of a mapping, and splitting it would exceed the process's number of mappings.
            Err(match std::io::Error::last_os_error().raw_os_error() { Some(libc::EINVAL) => Error::InvalidArgument, Some(libc::ENOMEM) => Error::OutOfMemory, _ => Error::Os })
        }
        #[cfg(not(unix))]
        { let _ = (address, length); Err(Error::Unsupported) }
    }
    unsafe fn sync(address: *mut u8, length: usize) -> Result<()> {
        #[cfg(unix)]
        loop {
            if unsafe { libc::msync(address.cast(), length, libc::MS_SYNC) } == 0 { return Ok(()); }
            // ENOMEM is msync's word for a range that is not mapped: not what a map returned.
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EINVAL) | Some(libc::ENOMEM) => return Err(Error::InvalidArgument),
                _ => return Err(Error::Os),
            }
        }
        #[cfg(not(unix))]
        { let _ = (address, length); Err(Error::Unsupported) }
    }
}
