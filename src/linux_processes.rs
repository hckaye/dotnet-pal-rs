//! Linux provider for the processes group: posix_spawn(3) over close-on-exec
//! pipes, a pidfd to wait with a timeout and exactly one waitpid per child. No
//! Rust heap; handles are CRT allocations that never move once their mutex
//! exists. `posix_spawn_file_actions_addchdir_np` needs glibc 2.29 or musl 1.1.24.
use crate::kernel::INFINITE;
use crate::linux::Linux;
use crate::port::{self, Error, Result};
use crate::processes::Spawned;
use core::{ffi::c_void, mem, ptr, sync::atomic::{AtomicI32, AtomicU8, Ordering}};

extern "C" { static mut environ: *mut *mut libc::c_char; }

const RUNNING: u8 = 0;
const ENDED: u8 = 1;
const LOST: u8 = 2;
/// Without a pidfd a wait looks again after the first interval, doubling up to the second.
const PAUSE_NS: (u64, u64) = (1_000_000, 16_000_000);

#[repr(C)]
struct Child {
    mutex: libc::pthread_mutex_t, // held around the reaping waitpid and around kill: a signal never goes to an identifier the kernel may reuse
    pid: libc::pid_t,
    pidfd: i32, // -1 before Linux 5.3 or with no descriptor to spare
    state: AtomicU8,
    code: AtomicI32,
}
#[repr(C)]
struct Pipe { fd: i32 }

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// The boundary's name for the errno of a spawn. EAGAIN is the process limit here, not "try again".
fn error(code: i32) -> Error {
    match code {
        libc::ENOENT => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, libc::EISDIR => Error::IsDirectory,
        libc::ENOTDIR => Error::NotDirectory, libc::ENAMETOOLONG => Error::NameTooLong, libc::EMFILE | libc::ENFILE => Error::TooManyHandles,
        libc::ENOMEM | libc::EAGAIN => Error::OutOfMemory, libc::EINVAL => Error::InvalidArgument, _ => Error::Os,
    }
}
fn transfer(code: i32) -> Error {
    match code { libc::EPIPE => Error::BrokenPipe, libc::EAGAIN => Error::WouldBlock, _ => Error::Os }
}
fn check(code: i32) -> core::result::Result<(), i32> { if code == 0 { Ok(()) } else { Err(code) } }
unsafe fn child(process: *mut c_void) -> Result<*mut Child> {
    if process.is_null() || !(process as usize).is_multiple_of(mem::align_of::<Child>()) { return Err(Error::InvalidArgument); }
    Ok(process.cast())
}
unsafe fn descriptor(pipe: *mut c_void) -> Result<i32> {
    if pipe.is_null() || !(pipe as usize).is_multiple_of(mem::align_of::<Pipe>()) { return Err(Error::InvalidArgument); }
    Ok(unsafe { (*pipe.cast::<Pipe>()).fd })
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

/// The verdict once it is in: the exit code, or the failure to learn it.
unsafe fn verdict(child: *mut Child) -> Option<Result<i32>> {
    match unsafe { (*child).state.load(Ordering::Acquire) } {
        RUNNING => None,
        ENDED => Some(Ok(unsafe { (*child).code.load(Ordering::Acquire) })),
        _ => Some(Err(Error::Os)),
    }
}
/// Looks once, with the mutex held. This is the only waitpid for the child: it
/// reaps exactly once and keeps the code for every later question.
unsafe fn look(child: *mut Child) -> Result<Option<i32>> {
    loop {
        if let Some(verdict) = unsafe { verdict(child) } { return verdict.map(Some); }
        let mut status = 0;
        let pid = unsafe { libc::waitpid((*child).pid, &mut status, libc::WNOHANG) };
        if pid == 0 { return Ok(None); }
        if pid < 0 {
            if errno() == libc::EINTR { continue; }
            // ECHILD: the embedding program reaped the child itself (a wait for any child, an ignored SIGCHLD) and the code went with it.
            unsafe { (*child).state.store(LOST, Ordering::Release) };
            return Err(Error::Os);
        }
        let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else if libc::WIFSIGNALED(status) { 128 + libc::WTERMSIG(status) } else { return Ok(None); };
        unsafe { (*child).code.store(code, Ordering::Release); (*child).state.store(ENDED, Ordering::Release) };
        return Ok(Some(code));
    }
}
unsafe fn locked<T>(child: *mut Child, call: impl FnOnce() -> T) -> Result<T> {
    let mutex = unsafe { ptr::addr_of_mut!((*child).mutex) };
    if unsafe { libc::pthread_mutex_lock(mutex) } != 0 { return Err(Error::Os); }
    let value = call();
    unsafe { libc::pthread_mutex_unlock(mutex) };
    Ok(value)
}
unsafe fn reap(child: *mut Child) -> Result<Option<i32>> {
    if let Some(verdict) = unsafe { verdict(child) } { return verdict.map(Some); }
    unsafe { locked(child, || look(child)) }?
}

/// Moves a descriptor out of the range of the standard streams. A program started with one of them closed gets
/// pipe descriptors below 3, which the dup2 for another stream would overwrite in the child.
fn lift(fd: &mut i32) -> Result<()> {
    if *fd > 2 { return Ok(()); }
    let moved = unsafe { libc::fcntl(*fd, libc::F_DUPFD_CLOEXEC, 3) };
    if moved < 0 { return Err(error(errno())); }
    unsafe { libc::close(*fd) };
    *fd = moved;
    Ok(())
}
/// What a spawn holds until the child or the caller has it; whatever is still here at the end is given back.
/// `ends[2 * stream]` is the child's end of a stream's pipe, `ends[2 * stream + 1]` the parent's.
struct Pending { ends: [i32; 6], child: *mut Child, pipes: [*mut Pipe; 3], vectors: *mut c_void }
impl Drop for Pending {
    fn drop(&mut self) {
        for fd in self.ends { if fd >= 0 { unsafe { libc::close(fd) }; } }
        for pipe in self.pipes { unsafe { libc::free(pipe.cast()) }; }
        if !self.child.is_null() { unsafe { libc::pthread_mutex_destroy(ptr::addr_of_mut!((*self.child).mutex)); libc::free(self.child.cast()) }; }
        unsafe { libc::free(self.vectors) };
    }
}
/// posix_spawn reports a failed file action (the working directory) and a failed exec as its return value.
unsafe fn start(program: &CPath, argv: *const *mut libc::c_char, envp: *const *mut libc::c_char, directory: Option<&CPath>, ends: &[i32; 6]) -> core::result::Result<libc::pid_t, i32> {
    let mut actions = mem::MaybeUninit::<libc::posix_spawn_file_actions_t>::uninit();
    let mut attributes = mem::MaybeUninit::<libc::posix_spawnattr_t>::uninit();
    check(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) })?;
    if let Err(code) = check(unsafe { libc::posix_spawnattr_init(attributes.as_mut_ptr()) }) {
        unsafe { libc::posix_spawn_file_actions_destroy(actions.as_mut_ptr()) };
        return Err(code);
    }
    let result = (|| {
        for stream in 0..3 {
            if ends[2 * stream] >= 0 { check(unsafe { libc::posix_spawn_file_actions_adddup2(actions.as_mut_ptr(), ends[2 * stream], stream as i32) })?; }
        }
        if let Some(directory) = directory { check(unsafe { libc::posix_spawn_file_actions_addchdir_np(actions.as_mut_ptr(), directory.as_ptr()) })?; }
        // The child starts with no signal blocked and every disposition at its default, not with what the runtime arranged for itself.
        let (mut none, mut all): (libc::sigset_t, libc::sigset_t) = unsafe { (mem::zeroed(), mem::zeroed()) };
        unsafe { libc::sigemptyset(&mut none); libc::sigfillset(&mut all) };
        check(unsafe { libc::posix_spawnattr_setsigmask(attributes.as_mut_ptr(), &none) })?;
        check(unsafe { libc::posix_spawnattr_setsigdefault(attributes.as_mut_ptr(), &all) })?;
        check(unsafe { libc::posix_spawnattr_setflags(attributes.as_mut_ptr(), (libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF) as libc::c_short) })?;
        let mut pid = 0;
        // The path is used as given: posix_spawn, unlike posix_spawnp, never searches PATH.
        check(unsafe { libc::posix_spawn(&mut pid, program.as_ptr(), actions.as_ptr(), attributes.as_ptr(), argv, envp) })?;
        Ok(pid)
    })();
    unsafe { libc::posix_spawnattr_destroy(attributes.as_mut_ptr()); libc::posix_spawn_file_actions_destroy(actions.as_mut_ptr()) };
    result
}

impl port::Processes for Linux {
    unsafe fn spawn(program: &[u8], arguments: &[*const u8], environment: Option<&[*const u8]>, directory: Option<&[u8]>, pipes: u32) -> Result<Spawned> {
        let program = CPath::new(program)?;
        let directory = match directory { Some(path) => Some(CPath::new(path)?), None => None };
        // Every allocation happens before the child exists: once it runs, nothing is left that could fail and orphan it.
        let mut pending = Pending { ends: [-1; 6], child: ptr::null_mut(), pipes: [ptr::null_mut(); 3], vectors: ptr::null_mut() };
        let handle = unsafe { libc::calloc(1, mem::size_of::<Child>()) }.cast::<Child>();
        if handle.is_null() { return Err(Error::OutOfMemory); }
        let code = unsafe { libc::pthread_mutex_init(ptr::addr_of_mut!((*handle).mutex), ptr::null()) };
        if code != 0 { unsafe { libc::free(handle.cast()) }; return Err(error(code)); }
        pending.child = handle;
        for stream in 0..3 {
            if pipes & (1 << stream) == 0 { continue; }
            pending.pipes[stream] = unsafe { libc::calloc(1, mem::size_of::<Pipe>()) }.cast();
            if pending.pipes[stream].is_null() { return Err(Error::OutOfMemory); }
            let mut pair = [-1i32; 2];
            if unsafe { libc::pipe2(pair.as_mut_ptr(), libc::O_CLOEXEC) } != 0 { return Err(error(errno())); }
            // The child reads its input and writes the other two.
            (pending.ends[2 * stream], pending.ends[2 * stream + 1]) = if stream == 0 { (pair[0], pair[1]) } else { (pair[1], pair[0]) };
            lift(&mut pending.ends[2 * stream])?;
            lift(&mut pending.ends[2 * stream + 1])?;
        }
        // The vectors gain the terminator the C library needs; calloc supplies it.
        let width = mem::size_of::<*mut libc::c_char>();
        pending.vectors = unsafe { libc::calloc(arguments.len() + 1 + environment.map_or(0, |list| list.len() + 1), width) };
        if pending.vectors.is_null() { return Err(Error::OutOfMemory); }
        let argv = pending.vectors.cast::<*mut libc::c_char>();
        unsafe { ptr::copy_nonoverlapping(arguments.as_ptr().cast(), argv, arguments.len()) };
        let envp = match environment {
            Some(list) => {
                let envp = unsafe { argv.add(arguments.len() + 1) };
                unsafe { ptr::copy_nonoverlapping(list.as_ptr().cast(), envp, list.len()) };
                envp
            }
            None => unsafe { ptr::addr_of!(environ).read() },
        };
        let pid = unsafe { start(&program, argv, envp, directory.as_ref(), &pending.ends) }.map_err(error)?;
        let handle = mem::replace(&mut pending.child, ptr::null_mut());
        // A pidfd is close-on-exec by definition. Without one the waits look at intervals instead.
        unsafe { (*handle).pid = pid; (*handle).pidfd = libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
        let mut handles = [ptr::null_mut::<c_void>(); 3];
        for (stream, slot) in handles.iter_mut().enumerate() {
            let pipe = mem::replace(&mut pending.pipes[stream], ptr::null_mut());
            if pipe.is_null() { continue; }
            unsafe { (*pipe).fd = mem::replace(&mut pending.ends[2 * stream + 1], -1) };
            *slot = pipe.cast();
        }
        Ok(Spawned { process: handle.cast(), id: pid as u64, input: handles[0], output: handles[1], error: handles[2] })
    }
    unsafe fn wait(process: *mut c_void, timeout_ns: u64) -> Result<i32> {
        let child = unsafe { child(process) }?;
        if let Some(code) = unsafe { reap(child) }? { return Ok(code); }
        if timeout_ns == 0 { return Err(Error::Timeout); }
        let deadline = if timeout_ns == INFINITE { None } else { Some(<Linux as port::Clock>::monotonic_ns()?.saturating_add(timeout_ns)) };
        let (mut pidfd, mut pause) = (unsafe { (*child).pidfd }, PAUSE_NS.0);
        loop {
            let remaining = match deadline { Some(deadline) => deadline.saturating_sub(<Linux as port::Clock>::monotonic_ns()?), None => INFINITE };
            if remaining == 0 { return Err(Error::Timeout); }
            if pidfd >= 0 {
                // Seconds are held to what every time_t can carry; a wait is never that long.
                let limit = libc::timespec { tv_sec: (remaining / 1_000_000_000).min(i32::MAX as u64) as _, tv_nsec: (remaining % 1_000_000_000) as _ };
                let mut slot = libc::pollfd { fd: pidfd, events: libc::POLLIN, revents: 0 };
                let ready = unsafe { libc::ppoll(&mut slot, 1, if deadline.is_some() { &limit } else { ptr::null() }, ptr::null()) };
                if ready < 0 && errno() != libc::EINTR { return Err(Error::Os); }
                // A pidfd turns readable when the child ends. One that reported and still left nothing to reap is not asked again.
                if ready > 0 { pidfd = -1; }
            } else {
                <Linux as port::Scheduler>::sleep_ns(remaining.min(pause))?;
                pause = (pause * 2).min(PAUSE_NS.1);
            }
            if let Some(code) = unsafe { reap(child) }? { return Ok(code); }
        }
    }
    unsafe fn terminate(process: *mut c_void, forceful: bool) -> Result<()> {
        let child = unsafe { child(process) }?;
        let signal = if forceful { libc::SIGKILL } else { libc::SIGTERM };
        // Looking first makes a child that has ended "gone" even when nobody has waited for it; the mutex keeps the
        // identifier this child's between the look and the signal.
        unsafe { locked(child, || match look(child) {
            Ok(None) => if libc::kill((*child).pid, signal) == 0 { Ok(()) } else { Err(if errno() == libc::ESRCH { Error::NotFound } else { error(errno()) }) },
            _ => Err(Error::NotFound),
        }) }?
    }
    unsafe fn release(process: *mut c_void) -> Result<()> {
        let child = unsafe { child(process) }?;
        // One look, so that a child that has already ended leaves no zombie. One that still runs is left to run.
        let _ = unsafe { reap(child) };
        unsafe {
            if (*child).pidfd >= 0 { libc::close((*child).pidfd); }
            libc::pthread_mutex_destroy(ptr::addr_of_mut!((*child).mutex));
            libc::free(process);
        }
        Ok(())
    }
    unsafe fn pipe_read(pipe: *mut c_void, out: *mut u8, capacity: usize) -> Result<usize> {
        let fd = unsafe { descriptor(pipe) }?;
        loop {
            let n = unsafe { libc::read(fd, out.cast(), capacity) };
            if n >= 0 { return Ok(n as usize); }
            if errno() != libc::EINTR { return Err(transfer(errno())); }
        }
    }
    unsafe fn pipe_write(pipe: *mut c_void, data: *const u8, size: usize) -> Result<usize> {
        let fd = unsafe { descriptor(pipe) }?;
        // A pipe has no MSG_NOSIGNAL. SIGPIPE is held back for this thread during the write and the one a failed write
        // raised is taken off the thread again; the process's disposition and the other threads are left alone.
        let (mut set, mut before): (libc::sigset_t, libc::sigset_t) = unsafe { (mem::zeroed(), mem::zeroed()) };
        unsafe { libc::sigemptyset(&mut set); libc::sigaddset(&mut set, libc::SIGPIPE) };
        if unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut before) } != 0 { return Err(Error::Os); }
        let held = unsafe { libc::sigismember(&before, libc::SIGPIPE) } == 1;
        // One that was already pending behind the caller's own block is the caller's to see.
        let foreign = held && {
            let mut pending: libc::sigset_t = unsafe { mem::zeroed() };
            unsafe { libc::sigpending(&mut pending) == 0 && libc::sigismember(&pending, libc::SIGPIPE) == 1 }
        };
        let (n, code) = loop {
            let n = unsafe { libc::write(fd, data.cast(), size) };
            if n >= 0 { break (n, 0); }
            if errno() != libc::EINTR { break (n, errno()); }
        };
        if code == libc::EPIPE && !foreign {
            let now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            while unsafe { libc::sigtimedwait(&set, ptr::null_mut(), &now) } < 0 && errno() == libc::EINTR {}
        }
        if !held { unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, ptr::null_mut()) }; }
        if n >= 0 { Ok(n as usize) } else { Err(transfer(code)) }
    }
    unsafe fn pipe_close(pipe: *mut c_void) -> Result<()> {
        let fd = unsafe { descriptor(pipe) }?;
        // close releases the descriptor even when it reports EINTR, so it is never retried.
        let code = if unsafe { libc::close(fd) } == 0 { 0 } else { errno() };
        unsafe { libc::free(pipe) };
        if code == 0 || code == libc::EINTR { Ok(()) } else { Err(Error::Os) }
    }
}
