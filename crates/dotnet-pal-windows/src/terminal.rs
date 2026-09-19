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




mod windows {
    use super::*;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents, GetStdHandle, SetConsoleMode,
        CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

    static ORIGINAL: Mutex<Option<u32>> = Mutex::new(None);
    fn handle(stream: u32) -> HANDLE { unsafe { GetStdHandle(match stream { 0 => STD_INPUT_HANDLE, 1 => STD_OUTPUT_HANDLE, _ => STD_ERROR_HANDLE }) } }

    impl port::Terminal for Std {
        fn window_size(stream: u32) -> Result<(u32, u32)> {
            let mut mode = 0;
            if unsafe { GetConsoleMode(handle(stream), &mut mode) } == 0 { return Err(Error::NotFound); }
            // Only a screen buffer knows the window: the console behind the input handle is asked through an output stream attached to it.
            for candidate in [stream, 1, 2] {
                let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
                if unsafe { GetConsoleScreenBufferInfo(handle(candidate), &mut info) } == 0 { continue; }
                let window = info.srWindow;
                return Ok(((window.Right as i32 - window.Left as i32 + 1).max(0) as u32, (window.Bottom as i32 - window.Top as i32 + 1).max(0) as u32));
            }
            Err(Error::Unsupported)
        }
        fn set_input_mode(raw: bool, min_bytes: u8, timeout_ds: u8, interrupt_as_input: bool) -> Result<()> {
            if raw && (min_bytes != 1 || timeout_ds != 0) { return Err(Error::Unsupported); }
            let mut original = ORIGINAL.lock().map_err(|_| Error::Os)?;
            let mut current = 0;
            if unsafe { GetConsoleMode(handle(0), &mut current) } == 0 { return Err(Error::NotFound); }
            let mut mode = *original.get_or_insert(current);
            if raw { mode &= !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT); }
            if interrupt_as_input { mode &= !ENABLE_PROCESSED_INPUT; } else { mode |= ENABLE_PROCESSED_INPUT; }
            if unsafe { SetConsoleMode(handle(0), mode) } == 0 { Err(Error::Os) } else { Ok(()) }
        }
        fn input_ready() -> Result<bool> {
            let mut pending = 0;
            if unsafe { GetNumberOfConsoleInputEvents(handle(0), &mut pending) } == 0 { return Err(Error::NotFound); }
            Ok(pending != 0)
        }
        fn control_character(_which: u32) -> Result<u8> { Err(Error::NotFound) }
    }
}
