//! Linux provider for the volumes group: the mount points of /proc/self/mounts,
//! statvfs for the space of a volume, and for its format the type text of the
//! mount that holds the path. No Rust heap and no state: every call reads the
//! table anew through bounded stack buffers, so an enumeration by index sees the
//! table as it is at each call, as the environment enumeration of the system
//! group sees the environment.
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::volumes::Status;
use core::{mem, ptr};

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// The longest mount point the boundary carries, and its terminator.
const POINT: usize = dotnet_pal_rs::runtime::MAX_NAME + 1;

/// A reader of the mount table, one byte at a time. The kernel writes one mount a line:
/// device, mount point, type, options and two numbers, separated by single spaces.
struct Table { fd: i32, buffer: [u8; 1024], filled: usize, at: usize }
impl Table {
    /// `Unsupported` without procfs: this system keeps no table a process can read.
    fn open() -> Result<Self> {
        loop {
            let fd = unsafe { libc::open(c"/proc/self/mounts".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            if fd >= 0 { return Ok(Self { fd, buffer: [0; 1024], filled: 0, at: 0 }); }
            match errno() { libc::EINTR => continue, libc::ENOENT => return Err(Error::Unsupported), libc::ENOMEM => return Err(Error::OutOfMemory), _ => return Err(Error::Os) }
        }
    }
    fn byte(&mut self) -> Result<Option<u8>> {
        while self.at == self.filled {
            let got = unsafe { libc::read(self.fd, self.buffer.as_mut_ptr().cast(), self.buffer.len()) };
            if got == 0 { return Ok(None); }
            if got < 0 { if errno() == libc::EINTR { continue; } return Err(Error::Os); }
            (self.filled, self.at) = (got as usize, 0);
        }
        self.at += 1;
        Ok(Some(self.buffer[self.at - 1]))
    }
    /// The next mount: its point in `point`, decoded and terminated, and its type text in `kind`. The length of the point is
    /// `None` when it is longer than the boundary carries; the mount keeps its place in the enumeration. A line without a
    /// mount point is no mount and is left out on every call alike, so the indices stay dense. `Ok(None)` at the end.
    fn next(&mut self, point: &mut [u8; POINT], kind: &mut [u8; 32]) -> Result<Option<(Option<usize>, usize)>> {
        loop {
            let (mut text, mut field, mut kind_length, mut any) = (Text { bytes: &mut *point, length: 0, fits: true }, 0usize, 0usize, false);
            // The kernel writes a space, a tab, a newline and a backslash of a mount point as a backslash and three octal digits.
            // `held` counts the bytes of an escape that is incomplete, `value` is what its digits say so far.
            let (mut held, mut value) = (0usize, 0u32);
            let ended = loop {
                let Some(byte) = self.byte()? else { break true; };
                any = true;
                if field == 1 {
                    if held > 0 && (b'0'..=b'7').contains(&byte) {
                        (held, value) = (held + 1, value * 8 + u32::from(byte - b'0'));
                        if held == 4 { if let Ok(decoded) = u8::try_from(value) { text.put(decoded); } else { text.literal(held, value); } held = 0; }
                        continue;
                    }
                    // No escape after all, which the kernel never writes: the bytes stand for themselves.
                    if held > 0 { text.literal(held, value); held = 0; }
                    if byte == b'\\' { (held, value) = (1, 0); continue; }
                }
                match byte {
                    b'\n' => break false,
                    b' ' | b'\t' => field += 1,
                    _ if field == 1 => text.put(byte),
                    _ if field == 2 && kind_length < kind.len() - 1 => { kind[kind_length] = byte; kind_length += 1; }
                    _ => {}
                }
            };
            if ended && !any { return Ok(None); }
            let (length, fits) = (text.length, text.fits);
            if length != 0 {
                point[length] = 0;
                kind[kind_length] = 0;
                return Ok(Some((fits.then_some(length), kind_length)));
            }
            if ended { return Ok(None); }
        }
    }
}
/// The decoded mount point: what fits the boundary's longest path, and whether all of it did.
struct Text<'a> { bytes: &'a mut [u8; POINT], length: usize, fits: bool }
impl Text<'_> {
    fn put(&mut self, byte: u8) { if self.length < POINT - 1 { self.bytes[self.length] = byte; self.length += 1; } else { self.fits = false; } }
    /// The backslash and the `held - 1` digits behind it, as they were read.
    fn literal(&mut self, held: usize, value: u32) {
        self.put(b'\\');
        for digit in (0..held - 1).rev() { self.put(b'0' + ((value >> (3 * digit)) & 7) as u8); }
    }
}
impl Drop for Table { fn drop(&mut self) { unsafe { libc::close(self.fd) }; } }

impl port::Volumes for Linux {
    unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
        let (mut table, mut point, mut kind, mut remaining) = (Table::open()?, [0u8; POINT], [0u8; 32], index);
        while let Some((length, _)) = table.next(&mut point, &mut kind)? {
            if remaining != 0 { remaining -= 1; continue; }
            // A mount point longer than any path of the boundary has an index and no text.
            let needed = length.ok_or(Error::Os)? + 1;
            if needed <= capacity { unsafe { ptr::copy_nonoverlapping(point.as_ptr(), out, needed) }; }
            return Ok(needed);
        }
        Err(Error::NotFound)
    }
    fn status(path: &[u8]) -> Result<Status> {
        let mut name = [0u8; POINT];
        if path.len() >= name.len() { return Err(Error::InvalidArgument); }
        name[..path.len()].copy_from_slice(path);
        let (mut space, mut node) = (mem::MaybeUninit::<libc::statvfs>::zeroed(), mem::MaybeUninit::<libc::stat>::zeroed());
        loop {
            if unsafe { libc::statvfs(name.as_ptr().cast(), space.as_mut_ptr()) } == 0 && unsafe { libc::stat(name.as_ptr().cast(), node.as_mut_ptr()) } == 0 { break; }
            match errno() {
                libc::EINTR => continue,
                // A path through something that is no directory names nothing either.
                libc::ENOENT | libc::ENOTDIR => return Err(Error::NotFound),
                libc::EACCES => return Err(Error::AccessDenied),
                libc::ENOMEM => return Err(Error::OutOfMemory),
                libc::ENOSYS => return Err(Error::Unsupported),
                _ => return Err(Error::Os),
            }
        }
        let (space, device) = unsafe { (space.assume_init(), node.assume_init().st_dev) };
        #[allow(clippy::unnecessary_cast)] // the block counts and the fragment size are narrower than 64 bits on some Linux targets
        let bytes = |blocks: libc::fsblkcnt_t| (blocks as u64).checked_mul(space.f_frsize as u64).ok_or(Error::Os);
        // The format is the type text of the mount whose point lies on the device of the path. A later mount covers an earlier one,
        // so the last one counts. Without a table, or without such a mount in it, this system has no name for the format.
        let (mut point, mut kind, mut format, mut format_length) = ([0u8; POINT], [0u8; 32], [0u8; 32], 0);
        if let Ok(mut table) = Table::open() {
            while let Ok(Some((length, kind_length))) = table.next(&mut point, &mut kind) {
                let mut mounted = mem::MaybeUninit::<libc::stat>::zeroed();
                if length.is_some() && unsafe { libc::stat(point.as_ptr().cast(), mounted.as_mut_ptr()) } == 0 && unsafe { mounted.assume_init() }.st_dev == device {
                    (format, format_length) = (kind, kind_length);
                }
            }
        }
        Ok(Status::new(bytes(space.f_blocks)?, bytes(space.f_bfree)?, bytes(space.f_bavail)?, &format[..format_length]))
    }
}
