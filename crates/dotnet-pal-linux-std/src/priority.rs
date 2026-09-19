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

mod unix {
    use super::super::Std;
    use dotnet_pal_rs::port::{self, Error, Result};
    use dotnet_pal_rs::priority::LEAST_FAVOURED;

    use libc::__errno_location as errno_location;



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
