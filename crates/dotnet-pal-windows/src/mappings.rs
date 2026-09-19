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



impl port::Mappings for Std {
    /// Absent where the port has no implementation: a capability that answers `Unsupported` to every call helps nobody.
    const PROVIDED: bool = cfg!(unix);
    unsafe fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8> {


        { let _ = (file, offset, length, access, shared); Err(Error::Unsupported) }
    }
    unsafe fn unmap(address: *mut u8, length: usize) -> Result<()> {


        { let _ = (address, length); Err(Error::Unsupported) }
    }
    unsafe fn sync(address: *mut u8, length: usize) -> Result<()> {


        { let _ = (address, length); Err(Error::Unsupported) }
    }
}
