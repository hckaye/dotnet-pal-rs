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


fn failure(e: std::io::Error) -> Error {
    match e.raw_os_error() {
        // A path through something that is no directory names nothing either.
        Some(libc::ENOENT) | Some(libc::ENOTDIR) => Error::NotFound,
        Some(libc::EACCES) => Error::AccessDenied,
        Some(libc::ENOMEM) => Error::OutOfMemory,
        _ => Error::Os,
    }
}
/// Repeats a call the kernel interrupted.

fn retry(mut call: impl FnMut() -> i32) -> Result<()> {
    loop {
        if call() == 0 { return Ok(()); }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted { return Err(failure(e)); }
    }
}



mod target {
    use super::*;
    fn text(field: &[std::ffi::c_char]) -> Vec<u8> { field.iter().map(|c| *c as u8).take_while(|byte| *byte != 0).collect() }
    pub fn points() -> Result<Vec<Vec<u8>>> {
        loop {
            // The table may grow between the question for its size and the one for its entries: a full buffer is asked again.
            let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
            if count < 0 { return Err(failure(std::io::Error::last_os_error())); }
            let mut list = Vec::<libc::statfs>::with_capacity(count as usize + 8);
            let size = i32::try_from(list.capacity() * std::mem::size_of::<libc::statfs>()).map_err(|_| Error::Os)?;
            let filled = unsafe { libc::getfsstat(list.as_mut_ptr(), size, libc::MNT_NOWAIT) };
            if filled < 0 { return Err(failure(std::io::Error::last_os_error())); }
            if filled as usize == list.capacity() { continue; }
            // SAFETY: the kernel wrote `filled` entries, fewer than the capacity.
            unsafe { list.set_len(filled as usize) };
            return Ok(list.iter().map(|mount| text(&mount.f_mntonname)).filter(|point| !point.is_empty()).collect());
        }
    }
    pub fn status(path: &std::ffi::CStr) -> Result<Status> {
        let mut space: libc::statfs = unsafe { std::mem::zeroed() };
        retry(|| unsafe { libc::statfs(path.as_ptr(), &mut space) })?;
        let bytes = |blocks: u64| blocks.checked_mul(u64::from(space.f_bsize)).ok_or(Error::Os);
        Ok(Status::new(bytes(space.f_blocks)?, bytes(space.f_bfree)?, bytes(space.f_bavail)?, &text(&space.f_fstypename)))
    }
}

impl port::Volumes for Std {
    /// Absent where the port has no implementation: a capability that answers `Unsupported` to every call helps nobody.
    const PROVIDED: bool = cfg!(any(target_os = "linux", target_os = "android", target_vendor = "apple"));
    unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {

        {
            let points = target::points()?;
            let point = points.get(index).ok_or(Error::NotFound)?;
            // A mount point longer than any path of the boundary has an index and no text.
            if point.len() > dotnet_pal_rs::runtime::MAX_NAME { return Err(Error::Os); }
            let needed = point.len() + 1;
            if needed <= capacity { unsafe { std::ptr::copy_nonoverlapping(point.as_ptr(), out, point.len()); out.add(point.len()).write(0); } }
            Ok(needed)
        }

    }
    fn status(path: &[u8]) -> Result<Status> {

        { target::status(&std::ffi::CString::new(path).map_err(|_| Error::InvalidArgument)?) }

    }
}
