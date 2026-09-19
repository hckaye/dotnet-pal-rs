//! Process lifetime services (`CAP_PROCESS`): orderly exit, debugger presence
//! and launching a crash-dump utility against the current process.
use crate::port::{self, Port, Process};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 16777216;
/// Bound on crash-dump argument vectors; the runtime builds at most a few dozen.
pub const MAX_ARGUMENTS: usize = 64;
/// Bound on one crash-dump argument or the whole error message.
pub const MAX_ARGUMENT: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub exit: Option<unsafe extern "C" fn(i32)>,
    pub debugger_present: Option<unsafe extern "C" fn(*mut u32) -> u32>,
    pub crash_dump: Option<unsafe extern "C" fn(*const *const u8, usize, *mut u8, usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub debugger_ok: u64, pub dump_ok: u64, pub rejected_or_failed: u64 }
static COUNTERS: [Counter; 3] = [const { Counter::new() }; 3];

fn record(status: u32, index: usize) -> u32 {
    let status = match status { OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR => status, _ => OS_ERROR };
    COUNTERS[if status == OK { index } else { 2 }].increment();
    status
}
unsafe extern "C" fn exit<P: Process>(code: i32) { P::exit(code) }
unsafe extern "C" fn debugger_present<P: Process>(out: *mut u32) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(0) };
    match P::debugger_present() {
        Ok(present) => { unsafe { out.write(present as u32) }; record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
/// Validates a NUL-terminated argument vector of bounded size before the provider sees it.
unsafe fn valid_arguments<'a>(argv: *const *const u8, argc: usize) -> Option<&'a [*const u8]> {
    if argv.is_null() || argc == 0 || argc > MAX_ARGUMENTS || (argv as usize) % mem::align_of::<*const u8>() != 0 { return None; }
    // SAFETY: the caller passes argc readable pointers, each to a NUL-terminated string.
    let arguments = unsafe { core::slice::from_raw_parts(argv, argc) };
    for &argument in arguments {
        if argument.is_null() { return None; }
        let mut length = 0;
        while unsafe { argument.add(length).read() } != 0 { length += 1; if length > MAX_ARGUMENT { return None; } }
    }
    Some(arguments)
}
unsafe extern "C" fn crash_dump<P: Process>(argv: *const *const u8, argc: usize, error: *mut u8, capacity: usize) -> u32 {
    if capacity > MAX_ARGUMENT || (capacity != 0 && error.is_null()) { return record(INVALID_ARGUMENT, 1); }
    if capacity != 0 { unsafe { error.write(0) }; }
    let Some(arguments) = (unsafe { valid_arguments(argv, argc) }) else { return record(INVALID_ARGUMENT, 1); };
    let status = port::status(unsafe { P::crash_dump(arguments, error, capacity) });
    // The message is informational; it is always terminated whatever the provider did.
    if capacity != 0 { unsafe { error.add(capacity - 1).write(0) }; }
    record(status, 1)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { debugger_ok: COUNTERS[0].load(), dump_ok: COUNTERS[1].load(), rejected_or_failed: COUNTERS[2].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { exit: None, debugger_present: None, crash_dump: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's process provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Process;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { exit: Some(exit::<T<P>>), debugger_present: Some(debugger_present::<T<P>>), crash_dump: Some(crash_dump::<T<P>>), read_stats: Some(read_stats) })
}
