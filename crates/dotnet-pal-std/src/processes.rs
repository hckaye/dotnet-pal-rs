//! Child processes of the desktop port, on `std::process`.
//!
//! The handle keeps the `std::process::Child`, and nothing reaps it behind the
//! provider's back: every `try_wait` and every signal happens under one mutex, so
//! a signal never goes to an identifier the OS is free to reuse. `Child` wants
//! `&mut self` to wait, so a wait never holds that mutex while it blocks. Instead
//! one thread at a time watches the child in the OS without reaping it (`waitid`
//! with `WNOWAIT` on Unix, the process handle on Windows; a Unix wait with a limit
//! looks at intervals) and the other waiters get its verdict through a condition.
//! That leaves `terminate` free to run during a wait that has no limit.
//!
//! The program path is used as given: a bare name gets `./` in front so that std
//! does not search `PATH` for it. Windows has no argument 0 apart from the
//! program, so the first argument is ignored there, and it has no polite way to
//! end a process; its branches compile but have not been executed here.
//!
//! A Unix child starts with no signal blocked and every disposition at its
//! default, so that `terminate` reaches it whatever the runtime arranged for
//! itself. std does that for SIGPIPE only and lets the child inherit the spawning
//! thread's signal mask and the signals the program ignores. When there is
//! anything of that kind to inherit, the provider resets it in a `pre_exec` step;
//! std then forks instead of using `posix_spawn`, which costs more in a large
//! process, and the usual case keeps the fast path.
//!
//! A pipe is a `std::fs::File` over the descriptor std created (close-on-exec on
//! Unix, not inheritable on Windows). This is a library: it cannot count on the
//! program ignoring SIGPIPE as Rust binaries do. Apple delivers a pipe's SIGPIPE
//! to the process rather than to the writing thread, so the parent's end of an
//! input pipe is marked `F_SETNOSIGPIPE` there; other Unix systems hold SIGPIPE
//! back for the writing thread and take the one a failed write raised off it again.
//!
//! A child under another identity (`CAP_SPAWN_AS`, Unix) is the same child with one
//! more `pre_exec` step. `CommandExt::groups` is not stable, and std changes the user
//! it is given before it runs a `pre_exec` step, when the privilege to set groups is
//! gone. It also enters the working directory before that step, as the parent's
//! user. So the step does all of it in the order the runtime's own System.Native
//! uses: groups, group, user, then the directory, which the new user has to be
//! allowed to enter. It runs in the forked child, the only thread of its own copy
//! of the memory: the calls are async-signal-safe there and nothing is allocated.
use super::{borrow, boxed, take, Std};
use dotnet_pal_rs::kernel::INFINITE;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::processes::{Spawned, PIPE_ERROR, PIPE_INPUT, PIPE_OUTPUT};
use std::{ffi::{c_void, CStr, OsStr}, fs, io::{self, Read, Write}, path::Path, process::{Child, Command, ExitStatus, Stdio}, ptr, sync::{Condvar, Mutex}, time::{Duration, Instant}};

/// A Unix wait with a limit looks again after the first interval, doubling up to the second.
const PAUSE: (Duration, Duration) = (Duration::from_millis(1), Duration::from_millis(16));

#[derive(Clone, Copy)]
enum Fate { Running, Ended(i32), Lost }
struct State { child: Child, fate: Fate, watched: bool }
struct Process { state: Mutex<State>, changed: Condvar, #[cfg(unix)] id: u32, #[cfg(windows)] handle: usize }
struct Pipe(fs::File);
#[cfg(unix)]
type Native = std::os::fd::OwnedFd;
#[cfg(windows)]
type Native = std::os::windows::io::OwnedHandle;

/// The boundary status of a failed spawn or signal: errno on Unix, `ErrorKind` elsewhere and
/// for the errors std raises without asking the OS. EAGAIN is the process limit here.
fn error(e: io::Error) -> Error {
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        return match code {
            libc::ENOENT => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, libc::EISDIR => Error::IsDirectory,
            libc::ENOTDIR => Error::NotDirectory, libc::ENAMETOOLONG => Error::NameTooLong, libc::EMFILE | libc::ENFILE => Error::TooManyHandles,
            libc::ENOMEM | libc::EAGAIN => Error::OutOfMemory, libc::EINVAL => Error::InvalidArgument, libc::ESRCH => Error::NotFound, _ => Error::Os,
        };
    }
    use io::ErrorKind as Kind;
    match e.kind() {
        Kind::NotFound => Error::NotFound, Kind::PermissionDenied => Error::AccessDenied, Kind::IsADirectory => Error::IsDirectory,
        Kind::NotADirectory => Error::NotDirectory, Kind::OutOfMemory => Error::OutOfMemory, Kind::InvalidInput => Error::InvalidArgument,
        // Windows folds malformed and over-long names into this kind; 206 is ERROR_FILENAME_EXCED_RANGE.
        Kind::InvalidFilename => if e.raw_os_error() == Some(206) { Error::NameTooLong } else { Error::InvalidArgument },
        _ => Error::Os,
    }
}
fn transfer(mut call: impl FnMut() -> io::Result<usize>) -> Result<usize> {
    loop {
        match call() {
            Ok(done) => return Ok(done),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(match e.kind() { io::ErrorKind::BrokenPipe => Error::BrokenPipe, io::ErrorKind::WouldBlock => Error::WouldBlock, _ => Error::Os }),
        }
    }
}
#[cfg(unix)]
fn text(bytes: &[u8]) -> Result<&OsStr> { Ok(std::os::unix::ffi::OsStrExt::from_bytes(bytes)) }
#[cfg(not(unix))]
fn text(bytes: &[u8]) -> Result<&OsStr> { std::str::from_utf8(bytes).map(OsStr::new).map_err(|_| Error::InvalidArgument) }
/// One text of an argument or environment vector: the caller's NUL-terminated borrow.
unsafe fn borrowed<'a>(text: *const u8) -> &'a [u8] { unsafe { CStr::from_ptr(text.cast()) }.to_bytes() }
/// `NAME=value` split at the first `=` after the first byte, as the C library and std read an
/// environment: a name is not empty, so it may begin with `=` (Windows keeps such variables).
fn variable(entry: &[u8]) -> Result<(&OsStr, &OsStr)> {
    let split = entry.iter().skip(1).position(|byte| *byte == b'=').ok_or(Error::InvalidArgument)? + 1;
    Ok((text(&entry[..split])?, text(&entry[split + 1..])?))
}
fn file(pipe: impl Into<Native>) -> fs::File { fs::File::from(pipe.into()) }
fn code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) { return 128 + signal; }
    status.code().unwrap_or(-1)
}
/// Looks once, with the mutex held. `try_wait` is what reaps the child; its answer is kept for every later question.
fn look(state: &mut State) -> Result<Option<i32>> {
    match state.fate { Fate::Ended(code) => return Ok(Some(code)), Fate::Lost => return Err(Error::Os), Fate::Running => {} }
    match state.child.try_wait() {
        Ok(None) => Ok(None),
        Ok(Some(status)) => { state.fate = Fate::Ended(code(status)); Ok(Some(code(status))) }
        // The embedding program reaped the child itself (a wait for any child, an ignored SIGCHLD) and the code went with it.
        Err(_) => { state.fate = Fate::Lost; Err(Error::Os) }
    }
}
/// Makes the child start with an empty signal mask and default dispositions when it would inherit anything else.
/// SIGPIPE does not count: std resets it on every path, and a runtime that ignores it would otherwise always fork.
#[cfg(unix)]
fn reset_signals(command: &mut Command) {
    const LAST: i32 = if cfg!(any(target_os = "linux", target_os = "android")) { 64 } else { 31 }; // Linux has real-time signals; the calls refuse the C library's own two
    let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, ptr::null(), &mut blocked) } != 0 { return; }
    let ignored = |number: i32| {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        number != libc::SIGPIPE && unsafe { libc::sigaction(number, ptr::null(), &mut action) } == 0 && action.sa_sigaction == libc::SIG_IGN
    };
    if !(1..=LAST).any(|number| unsafe { libc::sigismember(&blocked, number) } == 1 || ignored(number)) { return; }
    // Runs in the forked child: only async-signal-safe calls.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, || {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            for number in 1..=LAST { libc::sigaction(number, &action, ptr::null_mut()); }
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            if libc::sigprocmask(libc::SIG_SETMASK, &none, ptr::null_mut()) == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
        });
    }
}
/// Blocks, without the mutex, until the child may have ended or the deadline has passed.
#[cfg(unix)]
fn watch(process: &Process, deadline: Option<Instant>, pause: &mut Duration) {
    if deadline.is_none() {
        // WNOWAIT leaves the child for `look` to reap under the mutex.
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        loop {
            if unsafe { libc::waitid(libc::P_PID, process.id as libc::id_t, info.as_mut_ptr(), libc::WEXITED | libc::WNOWAIT) } == 0 { return; }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted { break; }
        }
    }
    std::thread::sleep(deadline.map_or(*pause, |deadline| deadline.saturating_duration_since(Instant::now()).min(*pause)));
    *pause = (*pause * 2).min(PAUSE.1);
}
#[cfg(windows)]
fn watch(process: &Process, deadline: Option<Instant>, _pause: &mut Duration) {
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE as FOREVER};
    // Rounded up, and below the value that means no limit.
    let limit = deadline.map_or(FOREVER, |deadline| deadline.saturating_duration_since(Instant::now()).as_millis().saturating_add(1).min(FOREVER as u128 - 1) as u32);
    unsafe { WaitForSingleObject(process.handle as _, limit) };
}
/// Runs a write that may find the other end closed. SIGPIPE is held back for this thread meanwhile and the one a
/// failed write raised is taken off it again, unless one was pending behind the caller's own block: that one is the
/// caller's to see. The program's disposition and the other threads are left alone.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn quietly(write: impl FnOnce() -> Result<usize>) -> Result<usize> {
    let (mut only, mut before, mut pending): (libc::sigset_t, libc::sigset_t, libc::sigset_t) = unsafe { (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed()) };
    unsafe { libc::sigemptyset(&mut only); libc::sigaddset(&mut only, libc::SIGPIPE) };
    if unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &only, &mut before) } != 0 { return Err(Error::Os); }
    let held = unsafe { libc::sigismember(&before, libc::SIGPIPE) } == 1;
    let foreign = held && unsafe { libc::sigpending(&mut pending) == 0 && libc::sigismember(&pending, libc::SIGPIPE) == 1 };
    let result = write();
    if result == Err(Error::BrokenPipe) && !foreign {
        let now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        while unsafe { libc::sigtimedwait(&only, ptr::null_mut(), &now) } < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {}
    }
    if !held { unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, ptr::null_mut()) }; }
    result
}
#[cfg(any(not(unix), target_vendor = "apple"))]
fn quietly(write: impl FnOnce() -> Result<usize>) -> Result<usize> { write() }

/// Makes the child take an identity and then enter its directory, in that order. Runs in the forked child.
#[cfg(unix)]
fn assume(command: &mut Command, user: u32, group: u32, groups: &[u32], directory: Option<&[u8]>) -> Result<()> {
    let directory = directory.map(std::ffi::CString::new).transpose().map_err(|_| Error::InvalidArgument)?;
    // Everything the step needs exists before the fork: the list, a sorted copy to search and room for the groups this process holds.
    let (groups, mut held) = (groups.to_vec(), vec![0 as libc::gid_t; groups.len()]);
    let mut wanted = groups.clone();
    wanted.sort_unstable();
    let limit = unsafe { libc::sysconf(libc::_SC_NGROUPS_MAX) };
    let step = move || {
        let failed = || Err(io::Error::last_os_error());
        if unsafe { libc::setgroups(groups.len() as _, groups.as_ptr()) } != 0 {
            // The runtime's own System.Native lets a process without the privilege start a child as its own user: the request
            // stands when this process holds no group the list lacks. macOS refuses a list of more than 16 before it looks at the privilege.
            let code = io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if code != libc::EPERM && !(code == libc::EINVAL && limit >= 0 && groups.len() as i64 > limit as i64) { return failed(); }
            let holds = unsafe { libc::getgroups(groups.len() as _, held.as_mut_ptr()) };
            if holds < 0 || (groups.is_empty() && holds > 0) || !held[..holds as usize].iter().all(|group| wanted.binary_search(group).is_ok()) {
                return Err(io::Error::from_raw_os_error(code));
            }
        }
        if unsafe { libc::setgid(group) != 0 || libc::setuid(user) != 0 } { return failed(); }
        if let Some(directory) = &directory { if unsafe { libc::chdir(directory.as_ptr()) } != 0 { return failed(); } }
        Ok(())
    };
    unsafe { std::os::unix::process::CommandExt::pre_exec(command, step) };
    Ok(())
}
/// A child of either group: `identity` is the user, the group and the groups of a child of `spawn_as`.
unsafe fn start(program: &[u8], arguments: &[*const u8], environment: Option<&[*const u8]>, directory: Option<&[u8]>, pipes: u32,
    identity: Option<(u32, u32, &[u32])>) -> Result<Spawned> {
    let program = Path::new(text(program)?);
    // std searches PATH for a bare name; the boundary starts the file it was given.
    let mut command = if program.parent().is_some_and(|parent| parent.as_os_str().is_empty()) { Command::new(Path::new(".").join(program)) } else { Command::new(program) };
    let mut arguments = arguments.iter().map(|argument| text(unsafe { borrowed(*argument) }));
    let name = arguments.next().ok_or(Error::InvalidArgument)??;
    #[cfg(unix)]
    std::os::unix::process::CommandExt::arg0(&mut command, name);
    #[cfg(not(unix))]
    let _ = name;
    for argument in arguments { command.arg(argument?); }
    if let Some(environment) = environment {
        command.env_clear();
        for entry in environment {
            let (name, value) = variable(unsafe { borrowed(*entry) })?;
            command.env(name, value);
        }
    }
    match identity {
        None => if let Some(directory) = directory { command.current_dir(text(directory)?); },
        #[cfg(unix)]
        Some((user, group, groups)) => assume(&mut command, user, group, groups, directory)?,
        #[cfg(not(unix))]
        Some(_) => return Err(Error::Unsupported),
    }
    let stream = |bit: u32| if pipes & bit != 0 { Stdio::piped() } else { Stdio::inherit() };
    command.stdin(stream(PIPE_INPUT)).stdout(stream(PIPE_OUTPUT)).stderr(stream(PIPE_ERROR));
    #[cfg(unix)]
    reset_signals(&mut command);
    let mut child = command.spawn().map_err(error)?;
    let (input, output, errors) = (child.stdin.take().map(file), child.stdout.take().map(file), child.stderr.take().map(file));
    #[cfg(target_vendor = "apple")]
    if let Some(input) = &input {
        const F_SETNOSIGPIPE: libc::c_int = 73; // <sys/fcntl.h>; this libc does not name it
        if unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(input), F_SETNOSIGPIPE, 1) } != 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Os);
        }
    }
    let id = child.id();
    #[cfg(windows)]
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&child) as usize;
    let process = Process { state: Mutex::new(State { child, fate: Fate::Running, watched: false }), changed: Condvar::new(), #[cfg(unix)] id, #[cfg(windows)] handle };
    let pipe = |file: Option<fs::File>| file.map_or(ptr::null_mut(), |file| boxed(Pipe(file)));
    Ok(Spawned { process: boxed(process), id: id as u64, input: pipe(input), output: pipe(output), error: pipe(errors) })
}

impl port::SpawnAs for Std {
    const PROVIDED: bool = cfg!(unix);
    unsafe fn spawn_as(program: &[u8], arguments: &[*const u8], environment: Option<&[*const u8]>, directory: Option<&[u8]>, pipes: u32,
        user_id: u32, group_id: u32, groups: &[u32]) -> Result<Spawned> {
        unsafe { start(program, arguments, environment, directory, pipes, Some((user_id, group_id, groups))) }
    }
}
impl port::Processes for Std {
    unsafe fn spawn(program: &[u8], arguments: &[*const u8], environment: Option<&[*const u8]>, directory: Option<&[u8]>, pipes: u32) -> Result<Spawned> {
        unsafe { start(program, arguments, environment, directory, pipes, None) }
    }
    unsafe fn wait(process: *mut c_void, timeout_ns: u64) -> Result<i32> {
        let process = unsafe { borrow::<Process>(process) }?;
        // A limit the clock cannot represent is no limit.
        let deadline = if timeout_ns == INFINITE { None } else { Instant::now().checked_add(Duration::from_nanos(timeout_ns)) };
        let mut pause = PAUSE.0;
        let mut state = process.state.lock().map_err(|_| Error::Os)?;
        loop {
            if state.watched {
                // Another thread watches the child and has not reaped it; its verdict arrives through the condition.
                state = match deadline {
                    None => process.changed.wait(state).map_err(|_| Error::Os)?,
                    Some(deadline) => {
                        let now = Instant::now();
                        if now >= deadline { return Err(Error::Timeout); }
                        process.changed.wait_timeout(state, deadline - now).map_err(|_| Error::Os)?.0
                    }
                };
                continue;
            }
            if let Some(code) = look(&mut state)? { return Ok(code); }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) { return Err(Error::Timeout); }
            state.watched = true;
            drop(state);
            watch(process, deadline, &mut pause);
            state = process.state.lock().map_err(|_| Error::Os)?;
            state.watched = false;
            process.changed.notify_all();
        }
    }
    unsafe fn terminate(process: *mut c_void, forceful: bool) -> Result<()> {
        let process = unsafe { borrow::<Process>(process) }?;
        let mut state = process.state.lock().map_err(|_| Error::Os)?;
        // Looking first makes a child that has ended "gone" even when nobody has waited for it. While another thread
        // watches the child it has not been reaped, which is all a signal needs: the identifier is still this child's.
        if !state.watched && !matches!(look(&mut state), Ok(None)) { return Err(Error::NotFound); }
        if forceful { return state.child.kill().map_err(error); }
        #[cfg(unix)]
        { if unsafe { libc::kill(process.id as libc::pid_t, libc::SIGTERM) } == 0 { Ok(()) } else { Err(error(io::Error::last_os_error())) } }
        #[cfg(not(unix))]
        { Err(Error::Unsupported) }
    }
    unsafe fn release(process: *mut c_void) -> Result<()> {
        let process = unsafe { take::<Process>(process) }?;
        // One look, so that a child that has already ended leaves no zombie. Dropping a `Child` neither ends it nor waits for it.
        if let Ok(mut state) = process.state.lock() { let _ = look(&mut state); }
        Ok(())
    }
    unsafe fn pipe_read(pipe: *mut c_void, out: *mut u8, capacity: usize) -> Result<usize> {
        let mut file = &unsafe { borrow::<Pipe>(pipe) }?.0;
        let buffer = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
        transfer(|| file.read(buffer))
    }
    unsafe fn pipe_write(pipe: *mut c_void, data: *const u8, size: usize) -> Result<usize> {
        let mut file = &unsafe { borrow::<Pipe>(pipe) }?.0;
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        quietly(|| transfer(|| file.write(bytes)))
    }
    unsafe fn pipe_close(pipe: *mut c_void) -> Result<()> { drop(unsafe { take::<Pipe>(pipe) }?); Ok(()) }
}
