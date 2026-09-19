//! Files and directories of the desktop port, on `std::fs`.
//!
//! Unix hands path bytes to the OS verbatim and reports permission bits, inode
//! and device numbers and the status-change time. Windows requires UTF-8 paths,
//! reports zero for mode, identity, device and change time, and cannot list an
//! entry whose name is not Unicode; its branches compile but have not been
//! executed here. A file handle is a boxed `std::fs::File` that is only ever used
//! positionally, with the access it was opened for, because the contract's answer
//! to a transfer the handle was not opened for (`AccessDenied`) is not an error
//! every OS reports the same way. A directory handle is a boxed
//! `std::fs::ReadDir` behind a mutex.
//!
//! The optional operations are std's where std has them. Times of a path are set
//! by name on Unix (`utimensat`): std sets them through an open file, and an open
//! needs an access the change does not and waits on a FIFO. Windows opens the
//! path, so it cannot name a link itself, and of the permission bits it has the
//! read-only attribute alone. Whole-file locks are `File::lock` and its kin:
//! flock on Unix, LockFileEx on Windows, where a lock is mandatory rather than
//! advisory. Range locks are open-file-description locks (Linux, Apple); the
//! F_SETLK lock every Unix has belongs to the process, not to the handle, and
//! std has no door to the range locks of Windows, so other targets have none.
use super::{borrow, boxed, take, Std};
use dotnet_pal_rs::files::{self, Status};
use dotnet_pal_rs::port::{self, Error, Result};
use std::{ffi::{c_void, OsStr}, fs, io, path::Path, ptr, sync::Mutex, time::{Duration, SystemTime}};

struct Directory(Mutex<fs::ReadDir>);
struct OpenFile { file: fs::File, flags: u32 }
impl OpenFile {
    fn allows(&self, access: u32) -> Result<&fs::File> { if self.flags & access != 0 { Ok(&self.file) } else { Err(Error::AccessDenied) } }
}
/// The file behind a handle and the access it was opened with, for the mappings group.


/// The boundary status of an I/O failure: errno on Unix, `ErrorKind` elsewhere
/// and for the errors std raises without asking the OS.
fn error(e: io::Error) -> Error {

    use io::ErrorKind as Kind;
    match e.kind() {
        Kind::NotFound => Error::NotFound, Kind::AlreadyExists => Error::AlreadyExists, Kind::PermissionDenied => Error::AccessDenied,
        Kind::IsADirectory => Error::IsDirectory, Kind::NotADirectory => Error::NotDirectory, Kind::DirectoryNotEmpty => Error::NotEmpty,
        Kind::StorageFull | Kind::QuotaExceeded => Error::NoSpace, Kind::ReadOnlyFilesystem => Error::ReadOnly,
        Kind::CrossesDevices => Error::CrossDevice, Kind::OutOfMemory => Error::OutOfMemory, Kind::InvalidInput => Error::InvalidArgument,
        Kind::ResourceBusy => Error::Busy,
        // Windows folds malformed and over-long names into this kind; 206 is ERROR_FILENAME_EXCED_RANGE.
        Kind::InvalidFilename => if e.raw_os_error() == Some(206) { Error::NameTooLong } else { Error::InvalidArgument },
        _ => Error::Os,
    }
}
/// A file operation refused on a directory: unlink reports EPERM on macOS (EISDIR
/// on Linux), and Windows denies access to both the open and the delete.
fn refused(e: io::Error, path: &Path) -> Error {
    match error(e) { Error::AccessDenied if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) => Error::IsDirectory, other => other }
}
fn retry<T>(mut call: impl FnMut() -> io::Result<T>) -> Result<T> {
    loop { match call() { Err(e) if e.kind() == io::ErrorKind::Interrupted => continue, other => return other.map_err(error) } }
}


fn native(path: &[u8]) -> Result<&Path> { std::str::from_utf8(path).map(Path::new).map_err(|_| Error::InvalidArgument) }


fn bytes(text: &OsStr) -> Option<&[u8]> { text.to_str().map(str::as_bytes) }

fn node(kind: fs::FileType) -> u32 {
    if kind.is_file() { files::NODE_FILE } else if kind.is_dir() { files::NODE_DIRECTORY } else if kind.is_symlink() { files::NODE_SYMLINK } else { files::NODE_OTHER }
}
/// Nanoseconds since the Unix epoch; zero for a time the target does not keep or one before the epoch.
fn ns(time: io::Result<SystemTime>) -> u64 {
    time.ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).and_then(|d| u64::try_from(d.as_nanos()).ok()).unwrap_or(0)
}
fn describe(meta: &fs::Metadata) -> Status {


    let (mode, changed_ns, identity, device) = (0, 0, 0, 0);
    Status { kind: node(meta.file_type()), mode, size: meta.len(), modified_ns: ns(meta.modified()), accessed_ns: ns(meta.accessed()),
        changed_ns, created_ns: ns(meta.created()), identity, device }
}
fn read_some(file: &fs::File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {


    { std::os::windows::fs::FileExt::seek_read(file, buffer, offset) }
}
fn write_some(file: &fs::File, data: &[u8], offset: u64) -> io::Result<usize> {


    { std::os::windows::fs::FileExt::seek_write(file, data, offset) }
}
/// The text contract of `current_directory`: the text and a NUL when they fit, the length needed either way.
unsafe fn deliver(text: &OsStr, out: *mut u8, capacity: usize) -> Result<usize> {
    let text = bytes(text).ok_or(Error::Os)?;
    if text.len() > dotnet_pal_rs::runtime::MAX_NAME { return Err(Error::NameTooLong); }
    let needed = text.len() + 1;
    if needed <= capacity { unsafe { ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()); out.add(text.len()).write(0); } }
    Ok(needed)
}
/// Unix takes the twelve bits. Windows has one to offer: a node its owner may not write is read-only.


fn permissions(mode: u32, current: impl FnOnce() -> io::Result<fs::Metadata>) -> Result<fs::Permissions> {
    let mut value = current().map_err(error)?.permissions();
    value.set_readonly(mode & 0o200 == 0);
    Ok(value)
}
/// The times std sets through a file; `None` is the one the node keeps.
fn file_times(accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<fs::FileTimes> {
    let moment = |ns: u64| SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(ns)).ok_or(Error::InvalidArgument);
    let mut times = fs::FileTimes::new();
    if let Some(ns) = accessed_ns { times = times.set_accessed(moment(ns)?); }
    if let Some(ns) = modified_ns { times = times.set_modified(moment(ns)?); }
    Ok(times)
}


fn set_path_times(path: &[u8], follow: bool, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let path = native(path)?;
    // An open always follows, so the times of a link itself are out of reach.
    if !follow && fs::symlink_metadata(path).map_err(error)?.is_symlink() { return Err(Error::Unsupported); }
    // FILE_WRITE_ATTRIBUTES is all the change needs; FILE_FLAG_BACKUP_SEMANTICS lets the path be a directory.
    let file = fs::OpenOptions::new().access_mode(0x0100).custom_flags(0x0200_0000).open(path).map_err(error)?;
    file.set_times(file_times(accessed_ns, modified_ns)?).map_err(error)
}
fn attempt(result: std::result::Result<(), fs::TryLockError>) -> Result<()> {
    match result { Ok(()) => Ok(()), Err(fs::TryLockError::WouldBlock) => Err(Error::WouldBlock), Err(fs::TryLockError::Error(e)) => Err(error(e)) }
}
/// An open-file-description lock. The F_SETLK lock of every Unix belongs to the process: two handles of one process
/// would never exclude each other, and closing any descriptor of the file would release it.


fn lock_range(_file: &fs::File, _offset: u64, _length: u64, _mode: u32) -> Result<()> { Err(Error::Unsupported) }

impl port::Files for Std {
    unsafe fn open(path: &[u8], flags: u32, mode: u32) -> Result<*mut c_void> {
        let path = native(path)?;
        let (write, create, exclusive) = (flags & files::WRITE != 0, flags & files::CREATE != 0, flags & files::EXCLUSIVE != 0);
        let mut options = fs::OpenOptions::new();
        options.read(flags & files::READ != 0).write(write).truncate(flags & files::TRUNCATE != 0);
        if write { options.create(create).create_new(exclusive); }
        // std refuses to create without write access, which the boundary allows just as open(2) and CreateFile do.


        {
            use std::os::windows::fs::OpenOptionsExt;
            let _ = mode;
            if create && !write { options.write(true).create(true).create_new(exclusive).access_mode(windows_sys::Win32::Foundation::GENERIC_READ); }
        }
        let file = options.open(path).map_err(|e| refused(e, path))?;
        // Unix opens a directory for reading without complaint.
        if file.metadata().map_err(error)?.is_dir() { return Err(Error::IsDirectory); }
        Ok(boxed(OpenFile { file, flags }))
    }
    unsafe fn close(file: *mut c_void) -> Result<()> { drop(unsafe { take::<OpenFile>(file) }?); Ok(()) }
    unsafe fn read_at(file: *mut c_void, offset: u64, out: *mut u8, capacity: usize) -> Result<usize> {
        let file = unsafe { borrow::<OpenFile>(file) }?.allows(files::READ)?;
        let buffer = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
        retry(|| read_some(file, buffer, offset))
    }
    unsafe fn write_at(file: *mut c_void, offset: u64, data: *const u8, size: usize) -> Result<usize> {
        let file = unsafe { borrow::<OpenFile>(file) }?.allows(files::WRITE)?;
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        retry(|| write_some(file, bytes, offset))
    }
    unsafe fn set_size(file: *mut c_void, size: u64) -> Result<()> {
        let file = unsafe { borrow::<OpenFile>(file) }?.allows(files::WRITE)?;
        retry(|| file.set_len(size))
    }
    unsafe fn flush(file: *mut c_void) -> Result<()> {
        let file = &unsafe { borrow::<OpenFile>(file) }?.file;
        retry(|| file.sync_all())
    }
    unsafe fn status(file: *mut c_void) -> Result<Status> {
        let file = &unsafe { borrow::<OpenFile>(file) }?.file;
        file.metadata().map(|m| describe(&m)).map_err(error)
    }
    unsafe fn path_status(path: &[u8], follow: bool) -> Result<Status> {
        let path = native(path)?;
        if follow { fs::metadata(path) } else { fs::symlink_metadata(path) }.map(|m| describe(&m)).map_err(error)
    }
    unsafe fn remove(path: &[u8]) -> Result<()> {
        let path = native(path)?;
        fs::remove_file(path).map_err(|e| refused(e, path))
    }
    unsafe fn rename(from: &[u8], to: &[u8]) -> Result<()> { fs::rename(native(from)?, native(to)?).map_err(error) }
    unsafe fn create_directory(path: &[u8], mode: u32) -> Result<()> {
        let path = native(path)?;


        let result = { let _ = mode; fs::create_dir(path) };
        result.map_err(error)
    }
    unsafe fn remove_directory(path: &[u8]) -> Result<()> { fs::remove_dir(native(path)?).map_err(error) }
    unsafe fn open_directory(path: &[u8]) -> Result<*mut c_void> {
        Ok(boxed(Directory(Mutex::new(fs::read_dir(native(path)?).map_err(error)?))))
    }
    unsafe fn read_directory(directory: *mut c_void, name: *mut u8, capacity: usize) -> Result<(usize, u32)> {
        if capacity < files::MAX_ENTRY_NAME { return Err(Error::InvalidArgument); }
        let mut entries = unsafe { borrow::<Directory>(directory) }?.0.lock().map_err(|_| Error::Os)?;
        loop {
            // std already leaves out "." and "..".
            let entry = entries.next().ok_or(Error::NotFound)?.map_err(error)?;
            let text = entry.file_name();
            // A name the boundary cannot carry (over 255 bytes, or not Unicode on Windows) is left out.
            let Some(text) = bytes(&text).filter(|t| t.len() <= files::MAX_ENTRY_NAME) else { continue; };
            let kind = match entry.file_type() {
                Ok(kind) => node(kind),
                // The entry vanished after it was listed; reporting NotFound here would read as the end of the directory.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(error(e)),
            };
            unsafe { ptr::copy_nonoverlapping(text.as_ptr(), name, text.len()) };
            return Ok((text.len(), kind));
        }
    }
    unsafe fn close_directory(directory: *mut c_void) -> Result<()> { drop(unsafe { take::<Directory>(directory) }?); Ok(()) }
    unsafe fn current_directory(out: *mut u8, capacity: usize) -> Result<usize> {
        let directory = std::env::current_dir().map_err(error)?;
        unsafe { deliver(directory.as_os_str(), out, capacity) }
    }
    unsafe fn set_mode(path: &[u8], mode: u32) -> Result<()> {
        let path = native(path)?;
        let value = permissions(mode, || fs::metadata(path))?;
        retry(|| fs::set_permissions(path, value.clone()))
    }
    unsafe fn set_file_mode(file: *mut c_void, mode: u32) -> Result<()> {
        let file = &unsafe { borrow::<OpenFile>(file) }?.file;
        let value = permissions(mode, || file.metadata())?;
        retry(|| file.set_permissions(value.clone()))
    }
    unsafe fn set_times(path: &[u8], follow: bool, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
        set_path_times(path, follow, accessed_ns, modified_ns)
    }
    unsafe fn set_file_times(file: *mut c_void, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
        let (file, times) = (&unsafe { borrow::<OpenFile>(file) }?.file, file_times(accessed_ns, modified_ns)?);
        retry(|| file.set_times(times))
    }
    unsafe fn link(existing: &[u8], created: &[u8]) -> Result<()> { fs::hard_link(native(existing)?, native(created)?).map_err(error) }
    unsafe fn symlink(target: &[u8], created: &[u8]) -> Result<()> {
        let (target, created) = (native(target)?, native(created)?);

        // Windows keeps the kind of the target in the link; a relative target starts at the link's directory.

        let result = if created.parent().is_some_and(|parent| parent.join(target).is_dir()) { std::os::windows::fs::symlink_dir(target, created) }
            else { std::os::windows::fs::symlink_file(target, created) };
        result.map_err(error)
    }
    unsafe fn read_link(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        // A path that is not a link is EINVAL on Unix and ERROR_NOT_A_REPARSE_POINT (4390) on Windows.
        let target = fs::read_link(native(path)?).map_err(|e| if cfg!(windows) && e.raw_os_error() == Some(4390) { Error::InvalidArgument } else { error(e) })?;
        unsafe { deliver(target.as_os_str(), out, capacity) }
    }
    unsafe fn real_path(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        // Windows answers in its verbatim form (`\\?\C:\...`), the only one in which every path it has is valid.
        let resolved = fs::canonicalize(native(path)?).map_err(error)?;
        unsafe { deliver(resolved.as_os_str(), out, capacity) }
    }
    unsafe fn set_current_directory(path: &[u8]) -> Result<()> { std::env::set_current_dir(native(path)?).map_err(error) }
    unsafe fn lock(file: *mut c_void, mode: u32, wait: bool) -> Result<()> {
        let file = &unsafe { borrow::<OpenFile>(file) }?.file;
        match (mode, wait) {
            (files::LOCK_SHARED, true) => retry(|| file.lock_shared()),
            (files::LOCK_EXCLUSIVE, true) => retry(|| file.lock()),
            (files::LOCK_SHARED, false) => attempt(file.try_lock_shared()),
            (files::LOCK_EXCLUSIVE, false) => attempt(file.try_lock()),
            // Windows objects to an unlock without a lock (ERROR_NOT_LOCKED, 158); Unix has nothing to do, and so has the boundary.
            (files::LOCK_UNLOCK, _) => file.unlock().or_else(|e| if cfg!(windows) && e.raw_os_error() == Some(158) { Ok(()) } else { Err(error(e)) }),
            _ => Err(Error::InvalidArgument),
        }
    }
    unsafe fn lock_range(file: *mut c_void, offset: u64, length: u64, mode: u32) -> Result<()> {
        lock_range(&unsafe { borrow::<OpenFile>(file) }?.file, offset, length, mode)
    }
}
