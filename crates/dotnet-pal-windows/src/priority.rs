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
