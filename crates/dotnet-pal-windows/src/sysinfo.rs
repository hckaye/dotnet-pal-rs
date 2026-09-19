//! System information of the desktop port: the environment as an enumeration,
//! the executable's path, operating system texts, the user, CPU time and uptime.
//!
//! Unix passes the bytes of variables and paths on verbatim, reads the operating
//! system texts from uname and the user from the passwd entry of the effective
//! user id. Uptime counts the time the machine was asleep: CLOCK_BOOTTIME on
//! Linux, `kern.boottime` against the wall clock on macOS, where CLOCK_UPTIME_RAW
//! stops while the machine sleeps; other Unix systems report it unsupported.
//! Windows carries Unicode text only, names the user from USERNAME and
//! USERPROFILE, reports no release or version text and no numeric ids; its
//! branches compile but have not been executed here.
use super::Std;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::system::{TEXT_EXECUTABLE_PATH, TEXT_HOME_DIRECTORY, TEXT_OS_NAME, TEXT_OS_RELEASE, TEXT_OS_VERSION, TEXT_USER_NAME};
use std::{ffi::OsStr, io, ptr};



fn bytes(text: &OsStr) -> Option<&[u8]> { text.to_str().map(str::as_bytes) }
/// The text contract: the bytes and their terminator when they fit, the length needed either way.
/// The boundary has no empty text, so an empty one is a question this system has no answer to.
unsafe fn deliver(text: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
    if text.is_empty() { return Err(Error::Unsupported); }
    let needed = text.len() + 1;
    if needed <= capacity { unsafe { ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()); out.add(text.len()).write(0); } }
    Ok(needed)
}
/// A variable the boundary can carry: its name has no '=' (std reads a leading one as part of the
/// name, which is how Windows keeps its per-drive directories, "=C:"), and on Windows both halves are Unicode.
fn carried(name: &OsStr, value: &OsStr) -> bool {
    matches!((bytes(name), bytes(value)), (Some(name), Some(_)) if !name.is_empty() && !name.contains(&b'='))
}



fn os_text(what: u32) -> Result<Vec<u8>> { if what == TEXT_OS_NAME { Ok(b"Windows".to_vec()) } else { Err(Error::Unsupported) } }


fn user_text(what: u32) -> Result<Vec<u8>> {
    let value = std::env::var_os(if what == TEXT_USER_NAME { "USERNAME" } else { "USERPROFILE" }).ok_or(Error::Unsupported)?;
    bytes(&value).map(<[u8]>::to_vec).ok_or(Error::Os)
}

impl port::SystemInfo for Std {
    unsafe fn environment_entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
        // Every call reads the environment as it is then. A variable that is not carried is left out on every call alike, so the indices stay dense.
        let (name, value) = std::env::vars_os().filter(|(name, value)| carried(name, value)).nth(index).ok_or(Error::NotFound)?;
        let text = [bytes(&name).ok_or(Error::Os)?, b"=", bytes(&value).ok_or(Error::Os)?].concat();
        unsafe { deliver(&text, out, capacity) }
    }
    unsafe fn text(what: u32, out: *mut u8, capacity: usize) -> Result<usize> {
        let text = match what {
            TEXT_EXECUTABLE_PATH => {
                let path = std::env::current_exe().map_err(|e| if e.kind() == io::ErrorKind::Unsupported { Error::Unsupported } else { Error::Os })?;
                // macOS names the path the program was started by; Linux the file itself. Resolving gives both the same meaning.
                // Windows is left alone: resolving there yields a verbatim (`\\?\`) path.

                bytes(path.as_os_str()).ok_or(Error::Os)?.to_vec()
            }
            TEXT_OS_NAME | TEXT_OS_RELEASE | TEXT_OS_VERSION => os_text(what)?,
            TEXT_USER_NAME | TEXT_HOME_DIRECTORY => user_text(what)?,
            _ => return Err(Error::InvalidArgument),
        };
        unsafe { deliver(&text, out, capacity) }
    }
    fn process_times() -> Result<(u64, u64)> {


        {
            use windows_sys::Win32::{Foundation::FILETIME, System::Threading::{GetCurrentProcess, GetProcessTimes}};
            // Creation, exit, kernel and user time, the last two as durations in 100 ns units.
            let mut times = [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 4];
            let [created, exited, kernel, user] = &mut times;
            if unsafe { GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user) } == 0 { return Err(Error::Os); }
            let ns = |time: &FILETIME| (((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64).saturating_mul(100);
            Ok((ns(&times[3]), ns(&times[2])))
        }
    }
    fn uptime_ns() -> Result<u64> {



        { Ok(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }.saturating_mul(1_000_000)) }

    }
    fn user_ids() -> Result<(u32, u32)> {


        { Err(Error::Unsupported) }
    }
}
