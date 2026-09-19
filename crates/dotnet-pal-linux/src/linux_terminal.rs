//! Linux provider for the terminal group: termios and the window-size ioctl of
//! the C library. The input mode, readiness and the editing characters belong to
//! the terminal behind descriptor 0; the window size is asked of the descriptor
//! of the stream named. No Rust heap.
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::terminal::{CONTROL_END_OF_FILE, CONTROL_END_OF_LINE, CONTROL_END_OF_LINE_2, CONTROL_ERASE};
use core::{cell::UnsafeCell, mem, sync::atomic::{AtomicU8, Ordering}};

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// A descriptor that is no terminal answers ENOTTY (EINVAL on some older kernels); a closed one EBADF.
fn failure() -> Error { match errno() { libc::ENOTTY | libc::EINVAL => Error::NotFound, _ => Error::Os } }
fn attributes() -> Result<libc::termios> {
    let mut settings = mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, settings.as_mut_ptr()) } != 0 { return Err(failure()); }
    Ok(unsafe { settings.assume_init() })
}

/// The settings the terminal had before this process first changed them. Every
/// mode is derived from them, so switching back and forth never accumulates.
struct Original(UnsafeCell<mem::MaybeUninit<libc::termios>>, AtomicU8);
unsafe impl Sync for Original {}
static ORIGINAL: Original = Original(UnsafeCell::new(mem::MaybeUninit::uninit()), AtomicU8::new(0));
fn original() -> Result<libc::termios> {
    loop {
        match ORIGINAL.1.load(Ordering::Acquire) {
            2 => return Ok(unsafe { (*ORIGINAL.0.get()).assume_init() }),
            1 => core::hint::spin_loop(),
            _ => {
                if ORIGINAL.1.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() { continue; }
                // A failure is not remembered: descriptor 0 may be a terminal the next time.
                let settings = attributes();
                if let Ok(value) = settings { unsafe { (*ORIGINAL.0.get()).write(value) }; }
                ORIGINAL.1.store(if settings.is_ok() { 2 } else { 0 }, Ordering::Release);
                return settings;
            }
        }
    }
}
/// A process group in the background of its terminal is stopped by SIGTTOU here until
/// it is in the foreground again, as job control wants it: the terminal belongs to
/// the foreground job, and changing its mode under that job would break its input.
fn apply(settings: &libc::termios) -> Result<()> {
    loop {
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, settings) } == 0 { return Ok(()); }
        if errno() != libc::EINTR { return Err(failure()); }
    }
}

impl port::Terminal for Linux {
    fn window_size(stream: u32) -> Result<(u32, u32)> {
        let mut size = mem::MaybeUninit::<libc::winsize>::uninit();
        if unsafe { libc::ioctl(stream as i32, libc::TIOCGWINSZ, size.as_mut_ptr()) } != 0 { return Err(failure()); }
        let size = unsafe { size.assume_init() };
        Ok((size.ws_col as u32, size.ws_row as u32))
    }
    fn set_input_mode(raw: bool, min_bytes: u8, timeout_ds: u8, interrupt_as_input: bool) -> Result<()> {
        let mut settings = original()?;
        if raw {
            // What System.Native clears for a console read: line assembly, echo and the
            // extended editing keys, and the flow-control and CR/NL rewriting of input bytes.
            settings.c_iflag &= !(libc::IXON | libc::IXOFF | libc::ICRNL | libc::INLCR | libc::IGNCR);
            settings.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN);
            settings.c_cc[libc::VMIN] = min_bytes;
            settings.c_cc[libc::VTIME] = timeout_ds;
        }
        if interrupt_as_input { settings.c_lflag &= !libc::ISIG; } else { settings.c_lflag |= libc::ISIG; }
        apply(&settings)
    }
    fn input_ready() -> Result<bool> {
        let mut entry = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
        loop {
            let count = unsafe { libc::poll(&mut entry, 1, 0) };
            if count < 0 { if errno() == libc::EINTR { continue; } return Err(Error::Os); }
            // Bytes, end of input and a hang-up all make a read return at once.
            return Ok(count > 0);
        }
    }
    fn control_character(which: u32) -> Result<u8> {
        let index = match which {
            CONTROL_ERASE => libc::VERASE, CONTROL_END_OF_LINE => libc::VEOL, CONTROL_END_OF_LINE_2 => libc::VEOL2, CONTROL_END_OF_FILE => libc::VEOF,
            _ => return Err(Error::InvalidArgument),
        };
        match attributes()?.c_cc[index] { libc::_POSIX_VDISABLE => Err(Error::NotFound), value => Ok(value) }
    }
}
