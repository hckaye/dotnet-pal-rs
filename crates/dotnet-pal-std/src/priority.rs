//! Scheduling priority of a process on the desktop port.
//!
//! Unix asks getpriority and setpriority with PRIO_PROCESS. macOS keeps one nice
//! value a process; it goes up to 20, which is reported as 19, the least favoured
//! value the boundary has. Linux keeps one a thread, and a process id names the
//! thread the process started with: `get` answers with the value of that thread,
//! also for process 0, and `set` changes that thread first, whose answer is the
//! answer of the call, and then every other thread listed in /proc/<id>/task, until
//! a pass over the list finds nothing left to change. A thread the kernel refuses
//! leaves the others changed and the call ACCESS_DENIED.
//! Windows has six priority classes: Idle is 19, BelowNormal 10, Normal 0,
//! AboveNormal -5, High -10 and Realtime -20, and `set` picks the class whose value
//! is nearest, the one nearer to Normal of two that are as near. Its branch compiles
//! but has not been executed here.
#[cfg(unix)]
mod unix {
    use super::super::Std;
    use dotnet_pal_rs::port::{self, Error, Result};
    use dotnet_pal_rs::priority::LEAST_FAVOURED;
    #[cfg(target_os = "linux")]
    use libc::__errno_location as errno_location;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    use libc::__error as errno_location;
    #[cfg(target_os = "android")]
    use libc::__errno as errno_location;

    fn errno() -> i32 { unsafe { *errno_location() } }
    fn failure() -> Error { match errno() { libc::ESRCH => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, _ => Error::Os } }
    /// The id the system knows the process by; one that is no process id names no process.
    fn target(process: u64) -> Result<libc::id_t> {
        if process == 0 { return Ok(unsafe { libc::getpid() } as libc::id_t); }
        match libc::pid_t::try_from(process) { Ok(id) => Ok(id as libc::id_t), Err(_) => Err(Error::NotFound) }
    }
    /// -1 is a value like any other: only errno tells it from a failure.
    fn read(id: libc::id_t) -> Result<i32> {
        unsafe { *errno_location() = 0 };
        let value = unsafe { libc::getpriority(libc::PRIO_PROCESS, id) };
        if value == -1 && errno() != 0 { return Err(failure()); }
        Ok(value.min(LEAST_FAVOURED))
    }
    fn write(id: libc::id_t, value: i32) -> Result<()> {
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, id, value) } == 0 { Ok(()) } else { Err(failure()) }
    }
    /// The other threads of a Linux process. A thread that ends between the list and the call is no failure, and a thread started
    /// meanwhile by one not yet changed has the old value, so the list is read until a pass finds nothing left to change.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn others(leader: libc::id_t, value: i32) -> Result<()> {
        for _ in 0..16 {
            // No procfs, or a process that has ended since its first thread was changed: there is no list to follow.
            let Ok(list) = std::fs::read_dir(format!("/proc/{leader}/task")) else { return Ok(()); };
            let (mut changed, mut refusal) = (false, Ok(()));
            for thread in list.flatten().filter_map(|entry| entry.file_name().to_str()?.parse::<libc::id_t>().ok()).filter(|thread| *thread != leader) {
                match read(thread) { Ok(found) if found == value => continue, Err(Error::NotFound) => continue, _ => {} }
                match write(thread, value) { Ok(()) => changed = true, Err(Error::NotFound) => {} Err(e) => if refusal.is_ok() { refusal = Err(e); } }
            }
            refusal?;
            if !changed { return Ok(()); }
        }
        Err(Error::Os)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn others(_: libc::id_t, _: i32) -> Result<()> { Ok(()) }

    impl port::Priority for Std {
        // A refusal is not an answer this question has.
        fn get(process: u64) -> Result<i32> { read(target(process)?).map_err(|e| if e == Error::NotFound { e } else { Error::Os }) }
        fn set(process: u64, value: i32) -> Result<()> {
            let leader = target(process)?;
            // No such process, or no permission for it, is said before anything has changed.
            write(leader, value)?;
            others(leader, value)
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::super::Std;
    use dotnet_pal_rs::port::{self, Error, Result};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetPriorityClass, OpenProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS,
        HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, REALTIME_PRIORITY_CLASS};

    /// The classes and their values, the nearer to Normal first: of two classes as near to a value, the first one is chosen.
    const CLASSES: [(u32, i32); 6] = [(NORMAL_PRIORITY_CLASS, 0), (ABOVE_NORMAL_PRIORITY_CLASS, -5), (BELOW_NORMAL_PRIORITY_CLASS, 10), (HIGH_PRIORITY_CLASS, -10),
        (IDLE_PRIORITY_CLASS, 19), (REALTIME_PRIORITY_CLASS, -20)];
    fn failure() -> Error { match unsafe { GetLastError() } { ERROR_ACCESS_DENIED => Error::AccessDenied, ERROR_INVALID_PARAMETER => Error::NotFound, _ => Error::Os } }
    /// Runs `body` on a handle of the process with the `access` it needs. Process 0 is the pseudo handle, which has every access.
    fn with<T>(process: u64, access: u32, body: impl FnOnce(HANDLE) -> Result<T>) -> Result<T> {
        if process == 0 { return body(unsafe { GetCurrentProcess() }); }
        let id = u32::try_from(process).map_err(|_| Error::NotFound)?;
        let handle = unsafe { OpenProcess(access, 0, id) };
        if handle.is_null() { return Err(failure()); }
        let result = body(handle);
        unsafe { CloseHandle(handle) };
        result
    }
    impl port::Priority for Std {
        fn get(process: u64) -> Result<i32> {
            with(process, PROCESS_QUERY_LIMITED_INFORMATION, |handle| {
                let class = unsafe { GetPriorityClass(handle) };
                CLASSES.iter().find(|(known, _)| *known == class).map(|(_, value)| *value).ok_or(Error::Os)
            }).map_err(|e| if e == Error::NotFound { e } else { Error::Os })
        }
        fn set(process: u64, value: i32) -> Result<()> {
            let (class, _) = CLASSES.iter().min_by_key(|(_, known)| (known - value).abs()).ok_or(Error::Os)?;
            with(process, PROCESS_SET_INFORMATION, |handle| if unsafe { SetPriorityClass(handle, *class) } != 0 { Ok(()) } else { Err(failure()) })
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod absent {
    use super::super::Std;
    use dotnet_pal_rs::port::{self, Error, Result};
    impl port::Priority for Std {
        const PROVIDED: bool = false;
        fn get(_: u64) -> Result<i32> { Err(Error::Unsupported) }
        fn set(_: u64, _: i32) -> Result<()> { Err(Error::Unsupported) }
    }
}
