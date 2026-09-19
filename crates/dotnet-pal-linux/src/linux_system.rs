//! Linux provider for the system group: the C library's environment block, the
//! executable link in procfs, uname, the passwd entry of the effective user,
//! resource usage and the boot-time clock. No Rust heap; bounded stack buffers only.
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::system::{TEXT_EXECUTABLE_PATH, TEXT_HOME_DIRECTORY, TEXT_OS_NAME, TEXT_OS_RELEASE, TEXT_OS_VERSION, TEXT_USER_NAME};
use core::{ffi::{c_char, CStr}, mem, ptr};

extern "C" { static environ: *const *const c_char; }

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// The text contract: the bytes and their terminator when they fit, the length needed either way.
/// The boundary has no empty text, so an empty one is a question this system has no answer to.
unsafe fn deliver(text: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
    if text.is_empty() { return Err(Error::Unsupported); }
    let needed = text.len() + 1;
    if needed <= capacity { unsafe { ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()); out.add(text.len()).write(0); } }
    Ok(needed)
}
fn nanoseconds(seconds: impl TryInto<u64>, fraction: impl TryInto<u64>, scale: u64) -> Result<u64> {
    let (Ok(seconds), Ok(fraction)) = (seconds.try_into(), fraction.try_into()) else { return Err(Error::Os); };
    seconds.checked_mul(1_000_000_000).and_then(|s| s.checked_add(fraction.checked_mul(scale)?)).ok_or(Error::Os)
}

impl port::SystemInfo for Linux {
    unsafe fn environment_entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
        // The caller prevents concurrent environment mutation for the full call, as for `Environment::get`.
        let mut slot = unsafe { environ };
        let mut remaining = index;
        while !slot.is_null() {
            let entry = unsafe { slot.read() };
            if entry.is_null() { break; }
            let text = unsafe { CStr::from_ptr(entry) }.to_bytes();
            // The block holds whatever strings the parent passed. One without '=' or without a name is
            // skipped on every call alike, so the indices of the others stay dense.
            if matches!(text.iter().position(|b| *b == b'='), Some(at) if at > 0) {
                if remaining == 0 { return unsafe { deliver(text, out, capacity) }; }
                remaining -= 1;
            }
            slot = unsafe { slot.add(1) };
        }
        Err(Error::NotFound)
    }
    unsafe fn text(what: u32, out: *mut u8, capacity: usize) -> Result<usize> {
        match what {
            TEXT_EXECUTABLE_PATH => {
                let mut buffer = [0u8; 4096];
                let length = unsafe { libc::readlink(c"/proc/self/exe".as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
                // Without procfs this system cannot say. readlink truncates silently: a full buffer is a path longer than any the boundary carries.
                if length < 0 { return Err(if errno() == libc::ENOENT { Error::Unsupported } else { Error::Os }); }
                if length as usize >= buffer.len() { return Err(Error::Os); }
                unsafe { deliver(&buffer[..length as usize], out, capacity) }
            }
            TEXT_OS_NAME | TEXT_OS_RELEASE | TEXT_OS_VERSION => {
                let mut names = mem::MaybeUninit::<libc::utsname>::zeroed();
                if unsafe { libc::uname(names.as_mut_ptr()) } != 0 { return Err(Error::Os); }
                let names = unsafe { names.assume_init() };
                let field = match what { TEXT_OS_NAME => &names.sysname, TEXT_OS_RELEASE => &names.release, _ => &names.version };
                let bytes = unsafe { core::slice::from_raw_parts(field.as_ptr().cast::<u8>(), field.len()) };
                unsafe { deliver(CStr::from_bytes_until_nul(bytes).map_err(|_| Error::Os)?.to_bytes(), out, capacity) }
            }
            TEXT_USER_NAME | TEXT_HOME_DIRECTORY => {
                let mut entry = mem::MaybeUninit::<libc::passwd>::zeroed();
                let mut strings = [0u8; 4096];
                let mut found = ptr::null_mut();
                let code = loop {
                    let code = unsafe { libc::getpwuid_r(libc::geteuid(), entry.as_mut_ptr(), strings.as_mut_ptr().cast(), strings.len(), &mut found) };
                    if code != libc::EINTR { break code; }
                };
                // A user without an entry (0 and no result, or ENOENT/ESRCH from a database that is not there) is a question
                // this system has no answer to: NOT_FOUND is not a status of this call. ERANGE is an entry larger than the buffer.
                if found.is_null() { return Err(if matches!(code, 0 | libc::ENOENT | libc::ESRCH) { Error::Unsupported } else { Error::Os }); }
                let field = unsafe { if what == TEXT_USER_NAME { (*found).pw_name } else { (*found).pw_dir } };
                if field.is_null() { return Err(Error::Unsupported); }
                unsafe { deliver(CStr::from_ptr(field).to_bytes(), out, capacity) }
            }
            _ => Err(Error::InvalidArgument),
        }
    }
    fn process_times() -> Result<(u64, u64)> {
        let mut usage = mem::MaybeUninit::<libc::rusage>::zeroed();
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 { return Err(Error::Os); }
        let usage = unsafe { usage.assume_init() };
        Ok((nanoseconds(usage.ru_utime.tv_sec, usage.ru_utime.tv_usec, 1000)?, nanoseconds(usage.ru_stime.tv_sec, usage.ru_stime.tv_usec, 1000)?))
    }
    fn uptime_ns() -> Result<u64> {
        // CLOCK_BOOTTIME keeps counting while the machine is suspended; CLOCK_MONOTONIC does not.
        let mut value = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } != 0 { return Err(Error::Os); }
        nanoseconds(value.tv_sec, value.tv_nsec, 1)
    }
    fn user_ids() -> Result<(u32, u32)> { Ok(unsafe { (libc::geteuid(), libc::getegid()) }) }
}
