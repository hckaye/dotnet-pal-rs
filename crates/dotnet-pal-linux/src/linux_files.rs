//! Linux provider for the files group: descriptors and directory streams of the
//! C library behind opaque handles. No Rust heap; a path is copied into a bounded
//! stack buffer to gain the terminator the C library needs.
use dotnet_pal_rs::files::{Status, CREATE, EXCLUSIVE, LOCK_EXCLUSIVE, LOCK_SHARED, LOCK_UNLOCK, NODE_DIRECTORY, NODE_FILE, NODE_OTHER, NODE_SYMLINK, READ, TRUNCATE, WRITE};
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use core::{ffi::{c_void, CStr}, mem, ptr};

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn error(code: i32) -> Error {
    match code {
        libc::ENOENT => Error::NotFound, libc::EEXIST => Error::AlreadyExists, libc::EACCES | libc::EPERM => Error::AccessDenied,
        libc::EISDIR => Error::IsDirectory, libc::ENOTDIR => Error::NotDirectory, libc::ENOTEMPTY => Error::NotEmpty,
        libc::ENOSPC | libc::EDQUOT => Error::NoSpace, libc::EMFILE | libc::ENFILE => Error::TooManyHandles,
        libc::ENAMETOOLONG => Error::NameTooLong, libc::EROFS => Error::ReadOnly, libc::EXDEV => Error::CrossDevice,
        libc::ENOMEM => Error::OutOfMemory, libc::EINVAL => Error::InvalidArgument, libc::EBUSY => Error::Busy, _ => Error::Os,
    }
}
/// Repeats a call the kernel interrupted; a negative result becomes the mapped errno.
fn retry(mut call: impl FnMut() -> isize) -> Result<usize> {
    loop {
        let value = call();
        if value >= 0 { return Ok(value as usize); }
        if errno() != libc::EINTR { return Err(error(errno())); }
    }
}
/// The kernel answers a transfer the descriptor was not opened for with EBADF (EINVAL
/// from ftruncate), the errors it also has for a descriptor that is not one. The
/// contract's answer is `AccessDenied`, so a live descriptor decides which it was.
fn refused(fd: i32, error: Error) -> Error {
    if matches!(error, Error::Os | Error::InvalidArgument) && unsafe { libc::fcntl(fd, libc::F_GETFL) } >= 0 { Error::AccessDenied } else { error }
}
/// A path argument with the terminator the C library needs.
struct CPath([u8; 4096]);
impl CPath {
    fn new(path: &[u8]) -> Result<Self> {
        let mut bytes = [0u8; 4096];
        if path.len() >= bytes.len() { return Err(Error::NameTooLong); }
        bytes[..path.len()].copy_from_slice(path);
        Ok(Self(bytes))
    }
    fn as_ptr(&self) -> *const libc::c_char { self.0.as_ptr().cast() }
}
#[repr(C)]
struct File { descriptor: i32 }
pub(crate) fn descriptor(file: *mut c_void) -> Result<i32> {
    if file.is_null() || !(file as usize).is_multiple_of(mem::align_of::<File>()) { return Err(Error::InvalidArgument); }
    Ok(unsafe { (*file.cast::<File>()).descriptor })
}
fn position(offset: u64) -> Result<libc::off_t> { offset.try_into().map_err(|_| Error::InvalidArgument) }
/// A time for utimensat and futimens; `None` is the one the node keeps.
fn timespec(ns: Option<u64>) -> Result<libc::timespec> {
    let mut value: libc::timespec = unsafe { mem::zeroed() }; // some targets pad the struct
    match ns {
        Some(ns) => { value.tv_sec = (ns / 1_000_000_000).try_into().map_err(|_| Error::InvalidArgument)?; value.tv_nsec = (ns % 1_000_000_000) as _; }
        None => value.tv_nsec = libc::UTIME_OMIT,
    }
    Ok(value)
}
/// The text contract of `current_directory`: the text and a NUL when they fit, the length needed either way.
unsafe fn deliver(text: &[u8], out: *mut u8, capacity: usize) -> usize {
    let needed = text.len() + 1;
    if needed <= capacity { unsafe { ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()); out.add(text.len()).write(0); } }
    needed
}

// Linux UAPI struct statx (256 bytes). Only the birth time is read from it:
// stat has no such field, and the raw call needs no particular C library version.
const STATX_INO: u32 = 0x100;
const STATX_BTIME: u32 = 0x800;
#[repr(C)]
struct StatxTime { seconds: i64, nanoseconds: u32, reserved: i32 }
#[repr(C)]
struct Statx {
    mask: u32, block_size: u32, attributes: u64, links: u32, user: u32, group: u32, mode: u16, reserved: u16,
    inode: u64, size: u64, blocks: u64, attributes_mask: u64,
    accessed: StatxTime, created: StatxTime, changed: StatxTime, modified: StatxTime, rest: [u64; 16],
}
/// Nanoseconds since the epoch; zero for a time the unsigned range cannot hold.
fn nanoseconds(seconds: impl TryInto<u64>, nanos: impl TryInto<u64>) -> u64 {
    let (Ok(seconds), Ok(nanos)) = (seconds.try_into(), nanos.try_into()) else { return 0; };
    seconds.checked_mul(1_000_000_000).and_then(|s| s.checked_add(nanos)).unwrap_or(0)
}
/// Birth time of the node `stat` just described, zero when the kernel or the
/// filesystem keeps none. The inode comparison discards an answer about a node
/// that took the path between the two calls.
fn created(directory: i32, path: *const libc::c_char, flags: i32, inode: u64) -> u64 {
    let mut value = mem::MaybeUninit::<Statx>::zeroed();
    if unsafe { libc::syscall(libc::SYS_statx, directory, path, flags, STATX_BTIME | STATX_INO, value.as_mut_ptr()) } != 0 { return 0; }
    let value = unsafe { value.assume_init() };
    if value.mask & (STATX_BTIME | STATX_INO) != STATX_BTIME | STATX_INO || value.inode != inode { return 0; }
    nanoseconds(value.created.seconds, value.created.nanoseconds)
}
fn node(mode: libc::mode_t) -> u32 {
    match mode & libc::S_IFMT { libc::S_IFREG => NODE_FILE, libc::S_IFDIR => NODE_DIRECTORY, libc::S_IFLNK => NODE_SYMLINK, _ => NODE_OTHER }
}
/// The status of the node `value` describes; `directory`, `path` and `flags` name the same node to statx.
#[allow(clippy::unnecessary_cast)] // ino_t and dev_t are narrower than 64 bits on some Linux targets
fn describe(value: &libc::stat, directory: i32, path: *const libc::c_char, flags: i32) -> Status {
    let identity = value.st_ino as u64;
    Status {
        kind: node(value.st_mode), mode: value.st_mode & 0o7777, size: value.st_size.try_into().unwrap_or(0),
        modified_ns: nanoseconds(value.st_mtime, value.st_mtime_nsec), accessed_ns: nanoseconds(value.st_atime, value.st_atime_nsec),
        changed_ns: nanoseconds(value.st_ctime, value.st_ctime_nsec), created_ns: created(directory, path, flags, identity),
        identity, device: value.st_dev as u64,
    }
}

impl port::Files for Linux {
    unsafe fn open(path: &[u8], flags: u32, mode: u32) -> Result<*mut c_void> {
        let path = CPath::new(path)?;
        let mut native = libc::O_CLOEXEC | match flags & (READ | WRITE) { READ => libc::O_RDONLY, WRITE => libc::O_WRONLY, _ => libc::O_RDWR };
        if flags & CREATE != 0 { native |= libc::O_CREAT; }
        if flags & EXCLUSIVE != 0 { native |= libc::O_EXCL; }
        if flags & TRUNCATE != 0 { native |= libc::O_TRUNC; }
        // Allocate first: a failure after the open could not undo a creation or a truncation.
        let file = unsafe { libc::calloc(1, mem::size_of::<File>()) }.cast::<File>();
        if file.is_null() { return Err(Error::OutOfMemory); }
        let fd = match retry(|| unsafe { libc::open(path.as_ptr(), native, mode as libc::c_uint) } as isize) {
            Ok(fd) => fd as i32,
            Err(e) => { unsafe { libc::free(file.cast()) }; return Err(e); }
        };
        // Linux opens a directory for reading like a file; this group opens those with open_directory.
        let mut value = mem::MaybeUninit::<libc::stat>::uninit();
        let failure = if unsafe { libc::fstat(fd, value.as_mut_ptr()) } != 0 { Some(error(errno())) }
            else if node(unsafe { value.assume_init() }.st_mode) == NODE_DIRECTORY { Some(Error::IsDirectory) } else { None };
        if let Some(e) = failure { unsafe { libc::close(fd); libc::free(file.cast()); } return Err(e); }
        unsafe { (*file).descriptor = fd; }
        Ok(file.cast())
    }
    unsafe fn close(file: *mut c_void) -> Result<()> {
        let fd = descriptor(file)?;
        unsafe { libc::free(file) };
        // Linux releases the descriptor even when close is interrupted: a retry could close another thread's file.
        if unsafe { libc::close(fd) } == 0 || errno() == libc::EINTR { Ok(()) } else { Err(error(errno())) }
    }
    unsafe fn read_at(file: *mut c_void, offset: u64, out: *mut u8, capacity: usize) -> Result<usize> {
        let (fd, offset) = (descriptor(file)?, position(offset)?);
        retry(|| unsafe { libc::pread(fd, out.cast(), capacity, offset) }).map_err(|e| refused(fd, e))
    }
    unsafe fn write_at(file: *mut c_void, offset: u64, data: *const u8, size: usize) -> Result<usize> {
        let (fd, offset) = (descriptor(file)?, position(offset)?);
        match retry(|| unsafe { libc::pwrite(fd, data.cast(), size, offset) }).map_err(|e| refused(fd, e))? { 0 => Err(Error::Os), done => Ok(done) }
    }
    unsafe fn set_size(file: *mut c_void, size: u64) -> Result<()> {
        let (fd, size) = (descriptor(file)?, position(size)?);
        retry(|| unsafe { libc::ftruncate(fd, size) } as isize).map(|_| ()).map_err(|e| refused(fd, e))
    }
    unsafe fn flush(file: *mut c_void) -> Result<()> {
        let fd = descriptor(file)?;
        retry(|| unsafe { libc::fsync(fd) } as isize).map(|_| ())
    }
    unsafe fn status(file: *mut c_void) -> Result<Status> {
        let fd = descriptor(file)?;
        let mut value = mem::MaybeUninit::<libc::stat>::uninit();
        retry(|| unsafe { libc::fstat(fd, value.as_mut_ptr()) } as isize)?;
        Ok(describe(&unsafe { value.assume_init() }, fd, c"".as_ptr(), libc::AT_EMPTY_PATH))
    }
    unsafe fn path_status(path: &[u8], follow: bool) -> Result<Status> {
        let path = CPath::new(path)?;
        let mut value = mem::MaybeUninit::<libc::stat>::uninit();
        retry(|| unsafe { if follow { libc::stat(path.as_ptr(), value.as_mut_ptr()) } else { libc::lstat(path.as_ptr(), value.as_mut_ptr()) } } as isize)?;
        Ok(describe(&unsafe { value.assume_init() }, libc::AT_FDCWD, path.as_ptr(), if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW }))
    }
    unsafe fn remove(path: &[u8]) -> Result<()> {
        let path = CPath::new(path)?;
        // Linux answers EISDIR for a directory where POSIX names EPERM, so the mapping needs no special case.
        retry(|| unsafe { libc::unlink(path.as_ptr()) } as isize).map(|_| ())
    }
    unsafe fn rename(from: &[u8], to: &[u8]) -> Result<()> {
        let (from, to) = (CPath::new(from)?, CPath::new(to)?);
        retry(|| unsafe { libc::rename(from.as_ptr(), to.as_ptr()) } as isize).map(|_| ())
    }
    unsafe fn create_directory(path: &[u8], mode: u32) -> Result<()> {
        let path = CPath::new(path)?;
        retry(|| unsafe { libc::mkdir(path.as_ptr(), mode as libc::mode_t) } as isize).map(|_| ())
    }
    unsafe fn remove_directory(path: &[u8]) -> Result<()> {
        let path = CPath::new(path)?;
        match retry(|| unsafe { libc::rmdir(path.as_ptr()) } as isize) {
            Err(Error::AlreadyExists) => Err(Error::NotEmpty), // POSIX lets rmdir say EEXIST for a directory with entries
            result => result.map(|_| ()),
        }
    }
    unsafe fn open_directory(path: &[u8]) -> Result<*mut c_void> {
        let path = CPath::new(path)?;
        loop {
            // The C library opens the stream's descriptor with O_CLOEXEC.
            let stream = unsafe { libc::opendir(path.as_ptr()) };
            if !stream.is_null() { return Ok(stream.cast()); }
            if errno() != libc::EINTR { return Err(error(errno())); }
        }
    }
    unsafe fn read_directory(directory: *mut c_void, name: *mut u8, capacity: usize) -> Result<(usize, u32)> {
        let stream = directory.cast::<libc::DIR>();
        loop {
            // readdir answers NULL at the end and on failure alike; only a cleared errno tells them apart.
            unsafe { *libc::__errno_location() = 0 };
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                match errno() { 0 => return Err(Error::NotFound), libc::EINTR => continue, code => return Err(error(code)) }
            }
            // A record may be shorter than the declared array: never a reference to the whole field.
            let text = unsafe { CStr::from_ptr(ptr::addr_of!((*entry).d_name).cast()) }.to_bytes();
            if text == b"." || text == b".." { continue; }
            if text.len() > capacity { return Err(Error::Os); }
            let kind = match unsafe { (*entry).d_type } {
                libc::DT_REG => NODE_FILE, libc::DT_DIR => NODE_DIRECTORY, libc::DT_LNK => NODE_SYMLINK,
                libc::DT_UNKNOWN => {
                    // The filesystem leaves the type to a lookup by the name, which keeps its terminator in the record.
                    // An entry deleted since it was listed is skipped.
                    let mut value = mem::MaybeUninit::<libc::stat>::uninit();
                    match retry(|| unsafe { libc::fstatat(libc::dirfd(stream), text.as_ptr().cast(), value.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) } as isize) {
                        Ok(_) => node(unsafe { value.assume_init() }.st_mode),
                        Err(Error::NotFound) => continue,
                        Err(e) => return Err(e),
                    }
                }
                _ => NODE_OTHER,
            };
            unsafe { ptr::copy_nonoverlapping(text.as_ptr(), name, text.len()) };
            return Ok((text.len(), kind));
        }
    }
    unsafe fn close_directory(directory: *mut c_void) -> Result<()> {
        // The stream and its descriptor are gone whatever closedir reports; see close.
        if unsafe { libc::closedir(directory.cast()) } == 0 || errno() == libc::EINTR { Ok(()) } else { Err(error(errno())) }
    }
    unsafe fn current_directory(out: *mut u8, capacity: usize) -> Result<usize> {
        // Read into a buffer of the longest path the boundary can report, so a short caller buffer still learns the length.
        let mut buffer = [0u8; 4096];
        if unsafe { libc::getcwd(buffer.as_mut_ptr().cast(), buffer.len()) }.is_null() {
            return Err(if errno() == libc::ERANGE { Error::NameTooLong } else { error(errno()) });
        }
        let length = unsafe { CStr::from_ptr(buffer.as_ptr().cast()) }.to_bytes_with_nul().len();
        if length <= capacity { unsafe { ptr::copy_nonoverlapping(buffer.as_ptr(), out, length) }; }
        Ok(length)
    }
    unsafe fn set_mode(path: &[u8], mode: u32) -> Result<()> {
        let path = CPath::new(path)?;
        retry(|| unsafe { libc::chmod(path.as_ptr(), mode as libc::mode_t) } as isize).map(|_| ())
    }
    unsafe fn set_file_mode(file: *mut c_void, mode: u32) -> Result<()> {
        let fd = descriptor(file)?;
        retry(|| unsafe { libc::fchmod(fd, mode as libc::mode_t) } as isize).map(|_| ())
    }
    unsafe fn set_times(path: &[u8], follow: bool, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
        let (path, times) = (CPath::new(path)?, [timespec(accessed_ns)?, timespec(modified_ns)?]);
        retry(|| unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW }) } as isize).map(|_| ())
    }
    unsafe fn set_file_times(file: *mut c_void, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
        let (fd, times) = (descriptor(file)?, [timespec(accessed_ns)?, timespec(modified_ns)?]);
        retry(|| unsafe { libc::futimens(fd, times.as_ptr()) } as isize).map(|_| ())
    }
    unsafe fn link(existing: &[u8], created: &[u8]) -> Result<()> {
        let (existing, created) = (CPath::new(existing)?, CPath::new(created)?);
        retry(|| unsafe { libc::link(existing.as_ptr(), created.as_ptr()) } as isize).map(|_| ())
    }
    unsafe fn symlink(target: &[u8], created: &[u8]) -> Result<()> {
        let (target, created) = (CPath::new(target)?, CPath::new(created)?);
        retry(|| unsafe { libc::symlink(target.as_ptr(), created.as_ptr()) } as isize).map(|_| ())
    }
    unsafe fn read_link(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        let path = CPath::new(path)?;
        // readlink truncates without saying so. The longest link text is one byte shorter than this buffer,
        // so an answer that leaves a byte free is whole. A path that is not a link is EINVAL.
        let mut buffer = [0u8; 4096];
        let length = retry(|| unsafe { libc::readlink(path.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) })?;
        if length >= buffer.len() { return Err(Error::NameTooLong); }
        Ok(unsafe { deliver(&buffer[..length], out, capacity) })
    }
    unsafe fn real_path(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
        let path = CPath::new(path)?;
        let mut buffer = [0u8; libc::PATH_MAX as usize]; // the size realpath requires of a caller's buffer
        loop {
            if !unsafe { libc::realpath(path.as_ptr(), buffer.as_mut_ptr().cast()) }.is_null() { break; }
            if errno() != libc::EINTR { return Err(error(errno())); }
        }
        Ok(unsafe { deliver(CStr::from_ptr(buffer.as_ptr().cast()).to_bytes(), out, capacity) })
    }
    unsafe fn set_current_directory(path: &[u8]) -> Result<()> {
        let path = CPath::new(path)?;
        retry(|| unsafe { libc::chdir(path.as_ptr()) } as isize).map(|_| ())
    }
    unsafe fn lock(file: *mut c_void, mode: u32, wait: bool) -> Result<()> {
        let fd = descriptor(file)?;
        // flock: its lock belongs to the open file description, which is what a handle is, and goes with the last close of it.
        let operation = match mode { LOCK_SHARED => libc::LOCK_SH, LOCK_EXCLUSIVE => libc::LOCK_EX, LOCK_UNLOCK => libc::LOCK_UN, _ => return Err(Error::InvalidArgument) };
        loop {
            if unsafe { libc::flock(fd, if wait { operation } else { operation | libc::LOCK_NB }) } == 0 { return Ok(()); }
            match errno() { libc::EINTR => continue, libc::EWOULDBLOCK => return Err(Error::WouldBlock), code => return Err(error(code)) }
        }
    }
    unsafe fn lock_range(file: *mut c_void, offset: u64, length: u64, mode: u32) -> Result<()> {
        let fd = descriptor(file)?;
        // An open-file-description lock. The classic F_SETLK lock belongs to the process: two handles of one
        // process would never exclude each other, and closing any descriptor of the file would release it.
        let mut range: libc::flock = unsafe { mem::zeroed() }; // l_pid stays zero, as the command demands
        range.l_type = match mode { LOCK_SHARED => libc::F_RDLCK, LOCK_EXCLUSIVE => libc::F_WRLCK, LOCK_UNLOCK => libc::F_UNLCK, _ => return Err(Error::InvalidArgument) } as _;
        range.l_whence = libc::SEEK_SET as _;
        (range.l_start, range.l_len) = (position(offset)?, position(length)?);
        loop {
            if unsafe { libc::fcntl(fd, libc::F_OFD_SETLK, &range) } == 0 { return Ok(()); }
            match errno() {
                libc::EINTR => continue,
                libc::EAGAIN | libc::EACCES => return Err(Error::WouldBlock),
                // A shared lock needs a descriptor open for reading, an exclusive one a descriptor open for writing.
                libc::EBADF => return Err(refused(fd, Error::Os)),
                // Every argument is valid here, so this is a kernel older than the command (Linux 3.15).
                libc::EINVAL => return Err(Error::Unsupported),
                code => return Err(error(code)),
            }
        }
    }
}
