//! Linux provider for the notifications group: the nine POSIX signals behind the
//! kinds, reported through a self-pipe. The signal handler only writes the kind
//! as one byte; a dispatcher thread of the provider's own reads the pipe and
//! calls the consumer from ordinary thread context. No Rust heap.
//!
//! The provider touches these nine signals only, and no signal mask but the
//! dispatcher's and, for the length of a `raise`, the caller's: the runtime keeps
//! SIGSEGV, SIGBUS, SIGFPE and a real-time signal for itself.
//!
//! The default action of a kind is what the action in place before `enable` does
//! with the signal. That is the kernel's default unless the process inherited or
//! installed something else: a process started under nohup keeps ignoring SIGHUP.
use crate::linux::Linux;
use crate::notifications::{deliver, CONTINUE, WINDOW_CHANGE};
use crate::port::{self, Error, Result};
use core::{cell::UnsafeCell, ffi::c_void, mem, ptr, sync::atomic::{AtomicI32, AtomicU32, Ordering}};

/// Indexed by kind; index 0 is no kind.
const SIGNALS: [i32; 10] = [0, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP, libc::SIGCONT, libc::SIGWINCH, libc::SIGTTIN, libc::SIGTTOU, libc::SIGTSTP];
struct Shared<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Shared<T> {}
/// Serializes every change of a signal action. The signal handler never takes it,
/// and the dispatcher does not hold it while the consumer's handler runs.
static LOCK: Shared<libc::pthread_mutex_t> = Shared(UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER));
/// The action each signal had before `enable`; accessed under `LOCK`.
static PREVIOUS: Shared<[libc::sigaction; 10]> = Shared(UnsafeCell::new(unsafe { mem::zeroed() }));
/// Bit `kind` is set while `on_signal` is the action of that signal; changed under `LOCK`.
static INSTALLED: AtomicU32 = AtomicU32::new(0);
static WRITE_END: AtomicI32 = AtomicI32::new(-1);
static OWNER: AtomicI32 = AtomicI32::new(0);

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn os(result: i32) -> Result<()> { if result == 0 { Ok(()) } else { Err(Error::Os) } }
fn signal(kind: u32) -> Result<i32> {
    match SIGNALS.get(kind as usize) { Some(&number) if number != 0 => Ok(number), _ => Err(Error::InvalidArgument) }
}
fn locked<T>(body: impl FnOnce() -> Result<T>) -> Result<T> {
    os(unsafe { libc::pthread_mutex_lock(LOCK.0.get()) })?;
    let result = body();
    unsafe { libc::pthread_mutex_unlock(LOCK.0.get()) };
    result
}
/// Invoked by the kernel on whatever thread took the signal: async-signal-safe
/// calls only, no lock, no counter, errno preserved.
extern "C" fn on_signal(code: i32) {
    let errno = unsafe { libc::__errno_location() };
    let saved = unsafe { errno.read() };
    // A child made by fork shares the pipe but has no dispatcher: what reaches it is not a request to the owner.
    if unsafe { libc::getpid() } == OWNER.load(Ordering::Acquire) {
        if let Some(kind) = SIGNALS.iter().position(|number| *number == code) {
            let byte = kind as u8;
            // The write end does not block: with a full pipe the report is dropped instead of stalling this thread.
            while unsafe { libc::write(WRITE_END.load(Ordering::Acquire), ptr::addr_of!(byte).cast(), 1) } < 0 && unsafe { errno.read() } == libc::EINTR {}
        }
    }
    unsafe { errno.write(saved) };
}
fn action() -> libc::sigaction {
    let mut action: libc::sigaction = unsafe { mem::zeroed() };
    action.sa_sigaction = on_signal as *const () as usize;
    // The signal lands on an arbitrary thread of the runtime: its blocking call is restarted, not failed with EINTR.
    action.sa_flags = libc::SA_RESTART;
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    action
}
extern "C" fn dispatcher(argument: *mut c_void) -> *mut c_void {
    // The nine are taken on other threads, so they never interrupt the consumer's handler.
    let mut set: libc::sigset_t = unsafe { mem::zeroed() };
    unsafe { libc::sigemptyset(&mut set) };
    for number in &SIGNALS[1..] { unsafe { libc::sigaddset(&mut set, *number) }; }
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, ptr::null_mut()) };
    let mut kinds = [0u8; 64];
    loop {
        let count = unsafe { libc::read(argument as usize as i32, kinds.as_mut_ptr().cast(), kinds.len()) };
        if count < 0 && errno() == libc::EINTR { continue; }
        if count <= 0 { return ptr::null_mut(); }
        for kind in &kinds[..count as usize] {
            // Nobody wanted it: the kind was disabled after the signal was caught, or `enable` has not returned
            // yet. The consumer never saw the report, so the signal counts as having arrived while the kind was
            // not enabled and gets the action it would have got then; dropping it would lose a terminate request.
            if !deliver(*kind as u32) { let _ = <Linux as port::Notifications>::default_action(*kind as u32); }
        }
    }
}

impl port::Notifications for Linux {
    fn start() -> Result<()> {
        let mut ends = [-1; 2];
        os(unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) })?;
        // Only the write end is non-blocking: the signal handler never waits, the dispatcher always does.
        let flags = unsafe { libc::fcntl(ends[1], libc::F_GETFL) };
        let mut thread: libc::pthread_t = 0;
        let mut rc = if flags < 0 { -1 } else { unsafe { libc::fcntl(ends[1], libc::F_SETFL, flags | libc::O_NONBLOCK) } };
        if rc == 0 {
            // Also warms the libc entry points of the handler before a first asynchronous call.
            unsafe { libc::__errno_location(); libc::write(ends[1], ptr::null(), 0); }
            OWNER.store(unsafe { libc::getpid() }, Ordering::Release);
            WRITE_END.store(ends[1], Ordering::Release);
            // Default stack size: the consumer's handler runs on this thread.
            rc = unsafe { libc::pthread_create(&mut thread, ptr::null(), dispatcher, ends[0] as usize as *mut c_void) };
        }
        if rc != 0 {
            WRITE_END.store(-1, Ordering::Release);
            unsafe { libc::close(ends[0]); libc::close(ends[1]); }
            return Err(Error::Os);
        }
        unsafe { libc::pthread_detach(thread) };
        Ok(())
    }
    fn enable(kind: u32) -> Result<()> {
        let number = signal(kind)?;
        if WRITE_END.load(Ordering::Acquire) < 0 { return Err(Error::InvalidArgument); }
        locked(|| {
            // The previous action is remembered once: enabling an enabled kind must not record `on_signal` as previous.
            if INSTALLED.load(Ordering::Acquire) & (1 << kind) != 0 { return Ok(()); }
            os(unsafe { libc::sigaction(number, &action(), ptr::addr_of_mut!((*PREVIOUS.0.get())[kind as usize])) })?;
            INSTALLED.fetch_or(1 << kind, Ordering::Release);
            Ok(())
        })
    }
    fn disable(kind: u32) -> Result<()> {
        let number = signal(kind)?;
        locked(|| {
            if INSTALLED.load(Ordering::Acquire) & (1 << kind) == 0 { return Ok(()); }
            os(unsafe { libc::sigaction(number, ptr::addr_of!((*PREVIOUS.0.get())[kind as usize]), ptr::null_mut()) })?;
            INSTALLED.fetch_and(!(1 << kind), Ordering::Release);
            Ok(())
        })
    }
    fn default_action(kind: u32) -> Result<()> {
        let number = signal(kind)?;
        // The kernel ignores these two unless somebody handles them; SIGCONT has resumed the process before anyone is told.
        if kind == CONTINUE || kind == WINDOW_CHANGE { return Ok(()); }
        locked(|| {
            let ours = INSTALLED.load(Ordering::Acquire) & (1 << kind) != 0;
            if ours {
                let previous = unsafe { ptr::addr_of!((*PREVIOUS.0.get())[kind as usize]) };
                if unsafe { (*previous).sa_sigaction } == libc::SIG_IGN { return Ok(()); }
                os(unsafe { libc::sigaction(number, previous, ptr::null_mut()) })?;
            }
            // The dispatcher blocks the signal and any caller may: unblocked on this thread only, for the raise only.
            let mut set: libc::sigset_t = unsafe { mem::zeroed() };
            let mut old: libc::sigset_t = unsafe { mem::zeroed() };
            unsafe { libc::sigemptyset(&mut set); libc::sigaddset(&mut set, number); }
            let mut rc = unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, &mut old) };
            if rc == 0 {
                rc = unsafe { libc::raise(number) };
                unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, ptr::null_mut()) };
            }
            // Still here: the previous action handled the signal, the kernel discarded a stop of an orphaned process
            // group, or a stop has been continued. The lock was held throughout, so the kind is still enabled.
            if ours { os(unsafe { libc::sigaction(number, &action(), ptr::null_mut()) })?; }
            os(rc)
        })
    }
}
