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

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
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
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn retry(mut call: impl FnMut() -> i32) -> Result<()> {
    loop {
        if call() == 0 { return Ok(()); }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted { return Err(failure(e)); }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod target {
    use super::*;
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    /// The text of a mount point as the kernel escaped it.
    fn decoded(field: &[u8]) -> Vec<u8> {
        let (mut out, mut at) = (Vec::with_capacity(field.len()), 0);
        while at < field.len() {
            let escape = field[at..].get(1..4).filter(|digits| field[at] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)));
            match escape.and_then(|digits| u8::try_from(digits.iter().fold(0u32, |value, d| value * 8 + u32::from(d - b'0'))).ok()) {
                Some(byte) => { out.push(byte); at += 4; }
                None => { out.push(field[at]); at += 1; }
            }
        }
        out
    }
    /// Mount point and type text of every line that has a mount point, in the order of the table.
    fn mounts() -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let table = std::fs::read("/proc/self/mounts").map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Error::Unsupported } else { failure(e) })?;
        Ok(table.split(|byte| *byte == b'\n').filter_map(|line| {
            let mut fields = line.split(|byte| *byte == b' ' || *byte == b'\t');
            let (point, kind) = (fields.nth(1)?, fields.next().unwrap_or_default());
            (!point.is_empty()).then(|| (decoded(point), kind.to_vec()))
        }).collect())
    }
    pub fn points() -> Result<Vec<Vec<u8>>> { Ok(mounts()?.into_iter().map(|(point, _)| point).collect()) }
    pub fn status(path: &std::ffi::CStr) -> Result<Status> {
        let mut space: libc::statvfs = unsafe { std::mem::zeroed() };
        retry(|| unsafe { libc::statvfs(path.as_ptr(), &mut space) })?;
        let device = std::fs::metadata(std::ffi::OsStr::from_bytes(path.to_bytes())).map_err(failure)?.dev();
        #[allow(clippy::unnecessary_cast)] // the block counts and the fragment size are narrower than 64 bits on some Linux targets
        let bytes = |blocks: libc::fsblkcnt_t| (blocks as u64).checked_mul(space.f_frsize as u64).ok_or(Error::Os);
        // A later mount covers an earlier one, so the last one on the device counts. Without a table, or without such a mount in
        // it, this system has no name for the format.
        let format = mounts().unwrap_or_default().into_iter().rev()
            .find(|(point, _)| std::fs::metadata(std::ffi::OsStr::from_bytes(point)).is_ok_and(|node| node.dev() == device)).map(|(_, kind)| kind).unwrap_or_default();
        Ok(Status::new(bytes(space.f_blocks)?, bytes(space.f_bfree)?, bytes(space.f_bavail)?, &format))
    }
}
#[cfg(target_vendor = "apple")]
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
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        {
            let points = target::points()?;
            let point = points.get(index).ok_or(Error::NotFound)?;
            // A mount point longer than any path of the boundary has an index and no text.
            if point.len() > dotnet_pal_rs::runtime::MAX_NAME { return Err(Error::Os); }
            let needed = point.len() + 1;
            if needed <= capacity { unsafe { std::ptr::copy_nonoverlapping(point.as_ptr(), out, point.len()); out.add(point.len()).write(0); } }
            Ok(needed)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        { let _ = (index, out, capacity); Err(Error::Unsupported) }
    }
    fn status(path: &[u8]) -> Result<Status> {
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        { target::status(&std::ffi::CString::new(path).map_err(|_| Error::InvalidArgument)?) }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        { let _ = path; Err(Error::Unsupported) }
    }
}
