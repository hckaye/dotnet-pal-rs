//! Requests from outside the process for the desktop port.
//!
//! Unix maps the nine kinds to POSIX signals. The signal handler only writes the
//! kind as one byte into a self-pipe; a `std::thread` of the port's own reads the
//! pipe and calls the consumer from ordinary thread context. Only these nine
//! signals are touched, and no signal mask but the dispatcher's and, for the
//! length of a `raise`, the caller's. The default action of a kind is what the
//! action in place before `enable` does with the signal: the kernel's default
//! unless the process inherited or installed something else (a process started
//! under nohup keeps ignoring SIGHUP).
//!
//! Windows has three of the kinds, as console control events. The system calls
//! the control routine on a thread it creates for the event, so the routine
//! reports directly. That branch compiles but has not been executed here.
use super::Std;
use dotnet_pal_rs::notifications::deliver;
use dotnet_pal_rs::port::{self, Error, Result};


mod platform {
    use super::*;
    use dotnet_pal_rs::notifications::{CONTINUE, WINDOW_CHANGE};
    use std::{mem, ptr, sync::{atomic::{AtomicI32, Ordering}, Mutex}};


    use libc::__error as errno_location;


    /// Indexed by kind; index 0 is no kind.
    const SIGNALS: [i32; 10] = [0, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP, libc::SIGCONT, libc::SIGWINCH, libc::SIGTTIN, libc::SIGTTOU, libc::SIGTSTP];
    /// The action each signal had before `enable`, `Some` while `on_signal` is its action. The lock serializes every
    /// change of a signal action; the signal handler never takes it, and the dispatcher does not hold it while the
    /// consumer's handler runs.
    static PREVIOUS: Mutex<[Option<libc::sigaction>; 10]> = Mutex::new([None; 10]);
    static WRITE_END: AtomicI32 = AtomicI32::new(-1);
    static OWNER: AtomicI32 = AtomicI32::new(0);

    fn os(result: i32) -> Result<()> { if result == 0 { Ok(()) } else { Err(Error::Os) } }
    fn signal(kind: u32) -> Result<i32> {
        match SIGNALS.get(kind as usize) { Some(&number) if number != 0 => Ok(number), _ => Err(Error::InvalidArgument) }
    }
    /// Invoked by the kernel on whatever thread took the signal: async-signal-safe
    /// calls only, no lock, no counter, errno preserved.
    extern "C" fn on_signal(code: i32) {
        let errno = unsafe { errno_location() };
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
    fn dispatch(read_end: i32) {
        // The nine are taken on other threads, so they never interrupt the consumer's handler.
        let mut set: libc::sigset_t = unsafe { mem::zeroed() };
        unsafe { libc::sigemptyset(&mut set) };
        for number in &SIGNALS[1..] { unsafe { libc::sigaddset(&mut set, *number) }; }
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, ptr::null_mut()) };
        let mut kinds = [0u8; 64];
        loop {
            let count = unsafe { libc::read(read_end, kinds.as_mut_ptr().cast(), kinds.len()) };
            if count < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; }
            if count <= 0 { return; }
            for kind in &kinds[..count as usize] {
                // Nobody wanted it: the kind was disabled after the signal was caught, or `enable` has not returned
                // yet. The consumer never saw the report, so the signal counts as having arrived while the kind was
                // not enabled and gets the action it would have got then; dropping it would lose a terminate request.
                if !deliver(*kind as u32) { let _ = <Std as port::Notifications>::default_action(*kind as u32); }
            }
        }
    }

    impl port::Notifications for Std {
        fn start() -> Result<()> {
            let mut ends = [-1; 2];


            os(unsafe { libc::pipe(ends.as_mut_ptr()) })?;
            // Only the write end is non-blocking: the signal handler never waits, the dispatcher always does.
            let flags = unsafe { libc::fcntl(ends[1], libc::F_GETFL) };
            let mut ready = flags >= 0 && unsafe { libc::fcntl(ends[1], libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0;
            // Without pipe2 a program another thread starts in between inherits the two descriptors.

            for end in ends { ready &= unsafe { libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC) } == 0; }
            if ready {
                OWNER.store(unsafe { libc::getpid() }, Ordering::Release);
                WRITE_END.store(ends[1], Ordering::Release);
                let read_end = ends[0];
                // Default stack size: the consumer's handler runs on this thread.
                ready = std::thread::Builder::new().name("pal-notify".into()).spawn(move || dispatch(read_end)).is_ok();
            }
            if !ready {
                WRITE_END.store(-1, Ordering::Release);
                unsafe { libc::close(ends[0]); libc::close(ends[1]); }
                return Err(Error::Os);
            }
            Ok(())
        }
        fn enable(kind: u32) -> Result<()> {
            let number = signal(kind)?;
            if WRITE_END.load(Ordering::Acquire) < 0 { return Err(Error::InvalidArgument); }
            let mut previous = PREVIOUS.lock().map_err(|_| Error::Os)?;
            // The previous action is remembered once: enabling an enabled kind must not record `on_signal` as previous.
            if previous[kind as usize].is_some() { return Ok(()); }
            let mut old: libc::sigaction = unsafe { mem::zeroed() };
            os(unsafe { libc::sigaction(number, &action(), &mut old) })?;
            previous[kind as usize] = Some(old);
            Ok(())
        }
        fn disable(kind: u32) -> Result<()> {
            let number = signal(kind)?;
            let mut previous = PREVIOUS.lock().map_err(|_| Error::Os)?;
            let Some(old) = previous[kind as usize] else { return Ok(()); };
            os(unsafe { libc::sigaction(number, &old, ptr::null_mut()) })?;
            previous[kind as usize] = None;
            Ok(())
        }
        fn default_action(kind: u32) -> Result<()> {
            let number = signal(kind)?;
            // The kernel ignores these two unless somebody handles them; SIGCONT has resumed the process before anyone is told.
            if kind == CONTINUE || kind == WINDOW_CHANGE { return Ok(()); }
            let previous = PREVIOUS.lock().map_err(|_| Error::Os)?;
            let ours = previous[kind as usize];
            if let Some(old) = &ours {
                if old.sa_sigaction == libc::SIG_IGN { return Ok(()); }
                os(unsafe { libc::sigaction(number, old, ptr::null_mut()) })?;
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
            if ours.is_some() { os(unsafe { libc::sigaction(number, &action(), ptr::null_mut()) })?; }
            os(rc)
        }
    }
}
