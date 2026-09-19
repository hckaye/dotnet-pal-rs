//! Mounted volumes of the desktop port.
//!
//! Linux reads /proc/self/mounts anew on every call, so an enumeration by index
//! sees the table as it is at each call; the kernel writes a space, a tab, a
//! newline and a backslash of a mount point as a backslash and three octal
//! digits. The space of a volume is statvfs, its format the type text of the
//! mount whose point lies on the device of the path. macOS asks getfsstat for
//! the table (getmntinfo answers from a buffer the C library shares between
//! threads) and statfs for a path, which names the format itself and counts
//! blocks in 64 bits where its statvfs has 32. Other Unix systems and Windows are
//! `Unsupported`: the volume functions of Windows sit behind the
//! `Win32_Storage_FileSystem` feature of `windows-sys`, which this crate does not
//! enable.
use super::Std;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::volumes::Status;


/// Repeats a call the kernel interrupted.





impl port::Volumes for Std {
    /// Absent where the port has no implementation: a capability that answers `Unsupported` to every call helps nobody.
    const PROVIDED: bool = cfg!(any(target_os = "linux", target_os = "android", target_vendor = "apple"));
    unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {


        { let _ = (index, out, capacity); Err(Error::Unsupported) }
    }
    fn status(path: &[u8]) -> Result<Status> {


        { let _ = path; Err(Error::Unsupported) }
    }
}
