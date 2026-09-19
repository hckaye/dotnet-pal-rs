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
    use dotnet_pal_rs::notifications::{INTERRUPT, QUIT, TERMINATE};
    use windows_sys::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT};
    use windows_sys::Win32::System::Threading::ExitProcess;
    /// FALSE hands the event to the next routine, in the end the system's own, which ends the process:
    /// the default action of a kind nobody wanted, including one disabled while the event was on its way.
    /// After TRUE for a close, logoff or shutdown event the system ends the process all the same, so the
    /// consumer's handler has to finish what it wants done before it returns.
    unsafe extern "system" fn on_control(event: u32) -> i32 {
        let kind = match event { CTRL_C_EVENT => INTERRUPT, CTRL_BREAK_EVENT => QUIT, CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => TERMINATE, _ => return 0 };
        deliver(kind) as i32
    }
    fn known(kind: u32) -> Result<()> { if matches!(kind, INTERRUPT | QUIT | TERMINATE) { Ok(()) } else { Err(Error::Unsupported) } }
    impl port::Notifications for Std {
        fn start() -> Result<()> { if unsafe { SetConsoleCtrlHandler(Some(on_control), 1) } == 0 { Err(Error::Os) } else { Ok(()) } }
        // The one routine stays registered; the front end's record of the enabled kinds decides per event.
        fn enable(kind: u32) -> Result<()> { known(kind) }
        fn disable(kind: u32) -> Result<()> { known(kind) }
        /// The system's own routine ends the process with ExitProcess; 0xC000013A is STATUS_CONTROL_C_EXIT,
        /// the status of an application ended by CTRL+C.
        fn default_action(kind: u32) -> Result<()> { known(kind)?; unsafe { ExitProcess(0xC000013A) } }
    }
}
