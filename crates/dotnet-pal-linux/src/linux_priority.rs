//! Linux provider for the priority group: getpriority and setpriority on a process id.
//!
//! The kernel keeps one nice value a thread, and PRIO_PROCESS with an id names the one
//! thread that carries it: for a process id that is the thread the process started
//! with. So `get` answers with the value of that thread, also for process 0, which to
//! the kernel would be the calling thread. `set` changes that thread first, whose
//! answer is the answer of the call, and then every other thread listed in
//! /proc/<id>/task, until a pass over the list finds nothing left to change: a
//! thread started meanwhile by one not yet changed has the old value. A thread the
//! kernel refuses leaves the others changed and the call ACCESS_DENIED. Without
//! procfs the first thread is all that changes. No Rust heap and no state.
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use core::ffi::CStr;
use core::ptr;

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn failure() -> Error { match errno() { libc::ESRCH => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, _ => Error::Os } }
/// Passes over the thread list before a process that keeps starting threads with the old value is given up on.
const PASSES: usize = 16;

/// The id the kernel knows the process by; one that is no process id names no process.
fn target(process: u64) -> Result<libc::id_t> {
    if process == 0 { return Ok(unsafe { libc::getpid() } as libc::id_t); }
    match libc::pid_t::try_from(process) { Ok(id) => Ok(id as libc::id_t), Err(_) => Err(Error::NotFound) }
}
/// The nice value of one thread. -1 is a value like any other: only errno tells it from a failure.
fn read(thread: libc::id_t) -> Result<i32> {
    unsafe { *libc::__errno_location() = 0 };
    let value = unsafe { libc::getpriority(libc::PRIO_PROCESS, thread) };
    if value == -1 && errno() != 0 { return Err(failure()); }
    Ok(value)
}
fn write(thread: libc::id_t, value: i32) -> Result<()> {
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, thread, value) } == 0 { Ok(()) } else { Err(failure()) }
}
/// "/proc/<id>/task", terminated.
fn task_list(id: libc::id_t) -> [u8; 32] {
    let (mut path, mut digits, mut rest, mut count) = ([0u8; 32], [0u8; 10], id, 0);
    loop { digits[count] = b'0' + (rest % 10) as u8; count += 1; rest /= 10; if rest == 0 { break; } }
    path[..6].copy_from_slice(b"/proc/");
    for index in 0..count { path[6 + index] = digits[count - 1 - index]; }
    path[6 + count..11 + count].copy_from_slice(b"/task");
    path
}
/// One pass over the threads of the process: whether any had another value, and the first refusal.
fn pass(path: &[u8; 32], leader: libc::id_t, value: i32) -> Result<Option<(bool, Result<()>)>> {
    let stream = loop {
        let stream = unsafe { libc::opendir(path.as_ptr().cast()) };
        if !stream.is_null() { break stream; }
        // No procfs, or a process that has ended since its first thread was changed: there is no list to follow.
        match errno() { libc::EINTR => continue, libc::ENOENT | libc::ENOTDIR | libc::ESRCH => return Ok(None), libc::ENOMEM => return Err(Error::OutOfMemory), _ => return Err(Error::Os) }
    };
    let (mut changed, mut refusal) = (false, Ok(()));
    loop {
        unsafe { *libc::__errno_location() = 0 };
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() { if errno() == libc::EINTR { continue; } break; }
        // SAFETY: a record keeps its terminated name; it may be shorter than the declared array, so never a reference to the whole field.
        let name = unsafe { CStr::from_ptr(ptr::addr_of!((*entry).d_name).cast()) }.to_bytes();
        let Some(thread) = name.iter().try_fold(0u32, |id, digit| if digit.is_ascii_digit() { id.checked_mul(10)?.checked_add(u32::from(digit - b'0')) } else { None }) else { continue; };
        if name.is_empty() || thread == leader { continue; }
        // A thread that ends between the list and the call is no failure.
        match read(thread) { Ok(found) if found == value => continue, Err(Error::NotFound) => continue, _ => {} }
        match write(thread, value) { Ok(()) => changed = true, Err(Error::NotFound) => {} Err(e) => if refusal.is_ok() { refusal = Err(e); } }
    }
    unsafe { libc::closedir(stream) };
    Ok(Some((changed, refusal)))
}

impl port::Priority for Linux {
    fn get(process: u64) -> Result<i32> {
        // A refusal is not an answer this question has.
        read(target(process)?).map_err(|e| if e == Error::NotFound { e } else { Error::Os })
    }
    fn set(process: u64, value: i32) -> Result<()> {
        let leader = target(process)?;
        // No such process, or no permission for it, is said before anything has changed.
        write(leader, value)?;
        let path = task_list(leader);
        for _ in 0..PASSES {
            match pass(&path, leader, value)? {
                None | Some((false, Ok(()))) => return Ok(()),
                Some((_, Err(e))) => return Err(e),
                Some((true, Ok(()))) => {}
            }
        }
        Err(Error::Os)
    }
}
