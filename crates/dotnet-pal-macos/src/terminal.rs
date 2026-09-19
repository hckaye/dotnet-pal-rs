//! The interactive terminal of the desktop port.
//!
//! Unix asks termios and the window-size ioctl. The input mode, readiness and
//! the editing characters belong to the terminal behind descriptor 0; the window
//! size is asked of the descriptor of the stream named. Every mode is derived
//! from the settings the terminal had before this process first changed them.
//! Windows asks the console API: a console read has no byte count or timeout to
//! wait for, readiness counts every pending input record (a key release as
//! well), and a console has no editing characters. The Windows branches compile
//! but have not been executed here.
use super::Std;
use dotnet_pal_rs::port::{self, Error, Result};
use std::sync::Mutex;


mod unix {
    use super::*;
    use dotnet_pal_rs::terminal::{CONTROL_END_OF_FILE, CONTROL_END_OF_LINE, CONTROL_END_OF_LINE_2, CONTROL_ERASE};
    use std::mem::MaybeUninit;

    static ORIGINAL: Mutex<Option<libc::termios>> = Mutex::new(None);
    fn errno() -> i32 { std::io::Error::last_os_error().raw_os_error().unwrap_or(0) }
    /// A descriptor that is no terminal answers ENOTTY; macOS answers ENODEV for
    /// /dev/null and EOPNOTSUPP for a socket. A closed one answers EBADF.
    fn failure() -> Error { match errno() { libc::ENOTTY | libc::EINVAL | libc::ENODEV | libc::EOPNOTSUPP => Error::NotFound, _ => Error::Os } }
    fn attributes() -> Result<libc::termios> {
        let mut settings = MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, settings.as_mut_ptr()) } != 0 { return Err(failure()); }
        Ok(unsafe { settings.assume_init() })
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

    impl port::Terminal for Std {
        fn window_size(stream: u32) -> Result<(u32, u32)> {
            let mut size = MaybeUninit::<libc::winsize>::uninit();
            // The request is an unsigned long for glibc and macOS and an int for musl.
            if unsafe { libc::ioctl(stream as i32, libc::TIOCGWINSZ as _, size.as_mut_ptr()) } != 0 { return Err(failure()); }
            let size = unsafe { size.assume_init() };
            Ok((size.ws_col as u32, size.ws_row as u32))
        }
        fn set_input_mode(raw: bool, min_bytes: u8, timeout_ds: u8, interrupt_as_input: bool) -> Result<()> {
            // Held across the change: two switches never interleave. A failure to read the
            // settings is not remembered: descriptor 0 may be a terminal the next time.
            let mut original = ORIGINAL.lock().map_err(|_| Error::Os)?;
            let mut settings = match *original { Some(settings) => settings, None => *original.insert(attributes()?) };
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
}
