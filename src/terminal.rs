//! The interactive terminal behind the standard streams (`CAP_TERMINAL`): its
//! size, the input mode, whether input is waiting, and its editing characters.
use crate::port::{Port, Terminal};
use crate::runtime::NOT_FOUND;
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 8589934592;
pub const CONTROL_ERASE: u32 = 1;
pub const CONTROL_END_OF_LINE: u32 = 2;
pub const CONTROL_END_OF_LINE_2: u32 = 3;
pub const CONTROL_END_OF_FILE: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub window_size: Option<unsafe extern "C" fn(u32, *mut u32, *mut u32) -> u32>,
    pub set_input_mode: Option<unsafe extern "C" fn(u32, u32, u32, u32) -> u32>,
    pub input_ready: Option<unsafe extern "C" fn(*mut u32) -> u32>,
    pub control_character: Option<unsafe extern "C" fn(u32, *mut u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub size_ok: u64, pub mode_ok: u64, pub ready_ok: u64, pub control_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 4;
static COUNTERS: [Counter; 5] = [const { Counter::new() }; 5];

fn record(status: u32, index: usize) -> u32 {
    let status = match status { OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | NOT_FOUND => status, _ => OS_ERROR };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
unsafe extern "C" fn window_size<T: Terminal>(stream: u32, columns: *mut u32, rows: *mut u32) -> u32 {
    if !aligned_output(columns) || !aligned_output(rows) { return record(INVALID_ARGUMENT, 0); }
    unsafe { columns.write(0); rows.write(0); }
    if stream > crate::streams::ERROR { return record(INVALID_ARGUMENT, 0); }
    match T::window_size(stream) {
        // A terminal nobody gave a size reports none. That is no failure, and no answer a caller can lay text out in either.
        Ok((0, _)) | Ok((_, 0)) => record(UNSUPPORTED, 0),
        Ok((c, r)) => { unsafe { columns.write(c); rows.write(r); } record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn set_input_mode<T: Terminal>(raw: u32, min_bytes: u32, timeout_ds: u32, interrupt_as_input: u32) -> u32 {
    if raw > 1 || interrupt_as_input > 1 || min_bytes > u8::MAX as u32 || timeout_ds > u8::MAX as u32 { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(T::set_input_mode(raw != 0, min_bytes as u8, timeout_ds as u8, interrupt_as_input != 0)), 1)
}
unsafe extern "C" fn input_ready<T: Terminal>(ready: *mut u32) -> u32 {
    if !aligned_output(ready) { return record(INVALID_ARGUMENT, 2); }
    unsafe { ready.write(0) };
    match T::input_ready() {
        Ok(value) => { unsafe { ready.write(value as u32) }; record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe extern "C" fn control_character<T: Terminal>(which: u32, value: *mut u32) -> u32 {
    if !aligned_output(value) { return record(INVALID_ARGUMENT, 3); }
    unsafe { value.write(0) };
    if !(CONTROL_ERASE..=CONTROL_END_OF_FILE).contains(&which) { return record(INVALID_ARGUMENT, 3); }
    match T::control_character(which) {
        Ok(byte) => { unsafe { value.write(byte as u32) }; record(OK, 3) }
        Err(e) => record(e.status(), 3),
    }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { size_ok: COUNTERS[0].load(), mode_ok: COUNTERS[1].load(), ready_ok: COUNTERS[2].load(), control_ok: COUNTERS[3].load(),
        rejected_or_failed: COUNTERS[FAILED].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { window_size: None, set_input_mode: None, input_ready: None, control_character: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's terminal provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Terminal;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { window_size: Some(window_size::<T<P>>), set_input_mode: Some(set_input_mode::<T<P>>), input_ready: Some(input_ready::<T<P>>),
        control_character: Some(control_character::<T<P>>), read_stats: Some(read_stats) })
}
