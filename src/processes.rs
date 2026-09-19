//! Child processes (`CAP_PROCESSES`): starting a program, the pipes to its
//! standard streams, waiting for its end and ending it. Handles are opaque
//! provider objects; texts are trusted NUL-terminated borrows.
use crate::io::{self, ACCESS_DENIED, BROKEN_PIPE, IS_DIRECTORY, NAME_TOO_LONG, NOT_DIRECTORY, TOO_MANY_HANDLES};
use crate::kernel::TIMEOUT;
use crate::port::{Port, Processes};
use crate::runtime::NOT_FOUND;
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP: u64 = 4294967296;
pub const PIPE_INPUT: u32 = 1;
pub const PIPE_OUTPUT: u32 = 2;
pub const PIPE_ERROR: u32 = 4;
pub const MAX_ARGUMENTS: usize = 65536;

/// A started child: its handle, its identifier and the parent's end of each pipe
/// that was asked for (null otherwise).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Spawned { pub process: *mut c_void, pub id: u64, pub input: *mut c_void, pub output: *mut c_void, pub error: *mut c_void }
impl Spawned {
    pub const EMPTY: Self = Self { process: ptr::null_mut(), id: 0, input: ptr::null_mut(), output: ptr::null_mut(), error: ptr::null_mut() };
    /// A result a consumer can act on: a handle, an identifier, and exactly the pipes that were asked for.
    pub(crate) fn consistent(&self, pipes: u32) -> bool {
        !self.process.is_null() && self.id != 0 && self.input.is_null() == (pipes & PIPE_INPUT == 0)
            && self.output.is_null() == (pipes & PIPE_OUTPUT == 0) && self.error.is_null() == (pipes & PIPE_ERROR == 0)
    }
}
pub(crate) type Texts = *const *const u8;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub spawn: Option<unsafe extern "C" fn(*const u8, usize, Texts, usize, Texts, usize, *const u8, usize, u32, *mut Spawned, usize) -> u32>,
    pub wait: Option<unsafe extern "C" fn(*mut c_void, u64, *mut i32) -> u32>,
    pub terminate: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub release: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub pipe_read: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize, *mut usize) -> u32>,
    pub pipe_write: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut usize) -> u32>,
    pub pipe_close: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub spawn_ok: u64, pub wait_ok: u64, pub terminate_ok: u64, pub release_ok: u64, pub pipe_read_ok: u64, pub pipe_write_ok: u64,
    pub pipe_close_ok: u64, pub rejected_or_failed: u64,
}
const FAILED: usize = 7;
static COUNTERS: [Counter; 8] = [const { Counter::new() }; 8];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | ACCESS_DENIED | TOO_MANY_HANDLES => status,
        NOT_FOUND | IS_DIRECTORY | NOT_DIRECTORY | NAME_TOO_LONG if index == 0 => status,
        TIMEOUT if index == 1 => status,
        NOT_FOUND if index == 2 => status, // the child is already gone
        BROKEN_PIPE if index == 5 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
fn valid_buffer(p: *const u8, size: usize) -> bool { size <= isize::MAX as usize && (size == 0 || (!p.is_null() && (p as usize).checked_add(size).is_some())) }
/// An array of non-null texts; the texts themselves are the caller's NUL-terminated borrows.
unsafe fn texts<'a>(list: Texts, count: usize) -> Option<&'a [*const u8]> {
    if count > MAX_ARGUMENTS || list.is_null() || !(list as usize).is_multiple_of(mem::align_of::<*const u8>()) { return None; }
    let list = unsafe { core::slice::from_raw_parts(list, count) };
    list.iter().all(|text| !text.is_null()).then_some(list)
}
/// What a spawn request names, borrowed from the caller; `None` for a request no provider should see.
pub(crate) type Request<'a> = (&'a [u8], &'a [*const u8], Option<&'a [*const u8]>, Option<&'a [u8]>);
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn request<'a>(program: *const u8, program_length: usize, arguments: Texts, argument_count: usize, environment: Texts, environment_count: usize,
    directory: *const u8, directory_length: usize, pipes: u32) -> Option<Request<'a>> {
    let program = unsafe { io::path(program, program_length) }?;
    // Argument 0 is the program's own name: an empty vector is not a vector a program can start with.
    let arguments = unsafe { texts(arguments, argument_count) }.filter(|list| !list.is_empty())?;
    let environment = if environment.is_null() && environment_count == 0 { None } else { Some(unsafe { texts(environment, environment_count) }?) };
    // NAME=value with a name: what every target can hand to a child, and what the child can read back.
    if let Some(list) = environment {
        for entry in list {
            // SAFETY: NUL-terminated texts are the caller's contract; the scan ends at the first `=` or NUL.
            let mut at = 0;
            while !matches!(unsafe { entry.add(at).read() }, 0 | b'=') { at += 1; }
            if at == 0 || unsafe { entry.add(at).read() } == 0 { return None; }
        }
    }
    let directory = if directory.is_null() && directory_length == 0 { None } else { Some(unsafe { io::path(directory, directory_length) }?) };
    if pipes & !(PIPE_INPUT | PIPE_OUTPUT | PIPE_ERROR) != 0 { return None; }
    Some((program, arguments, environment, directory))
}
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn spawn<S: Processes>(program: *const u8, program_length: usize, arguments: Texts, argument_count: usize, environment: Texts,
    environment_count: usize, directory: *const u8, directory_length: usize, pipes: u32, out: *mut Spawned, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Spawned>() { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(Spawned::EMPTY) };
    let Some((program, arguments, environment, directory)) = (unsafe { request(program, program_length, arguments, argument_count, environment, environment_count,
        directory, directory_length, pipes) }) else { return record(INVALID_ARGUMENT, 0); };
    let status = match unsafe { S::spawn(program, arguments, environment, directory, pipes) } {
        Ok(spawned) if spawned.consistent(pipes) => { unsafe { out.write(spawned) }; OK }
        Ok(_) => OS_ERROR, // a child the consumer cannot hold or pipes it did not ask for: a broken provider
        Err(e) => e.status(),
    };
    record(status, 0)
}
unsafe extern "C" fn wait<S: Processes>(process: *mut c_void, timeout_ns: u64, exit_code: *mut i32) -> u32 {
    if !aligned_output(exit_code) { return record(INVALID_ARGUMENT, 1); }
    unsafe { exit_code.write(0) };
    if process.is_null() { return record(INVALID_ARGUMENT, 1); }
    match unsafe { S::wait(process, timeout_ns) } {
        Ok(code) => { unsafe { exit_code.write(code) }; record(OK, 1) }
        Err(e) => record(e.status(), 1),
    }
}
unsafe extern "C" fn terminate<S: Processes>(process: *mut c_void, forceful: u32) -> u32 {
    if process.is_null() || forceful > 1 { return record(INVALID_ARGUMENT, 2); }
    record(crate::port::status(unsafe { S::terminate(process, forceful != 0) }), 2)
}
unsafe extern "C" fn release<S: Processes>(process: *mut c_void) -> u32 {
    if process.is_null() { return record(INVALID_ARGUMENT, 3); }
    record(crate::port::status(unsafe { S::release(process) }), 3)
}
unsafe extern "C" fn pipe_read<S: Processes>(pipe: *mut c_void, data: *mut u8, capacity: usize, got: *mut usize) -> u32 {
    if !aligned_output(got) { return record(INVALID_ARGUMENT, 4); }
    unsafe { got.write(0) };
    if pipe.is_null() || !valid_buffer(data, capacity) { return record(INVALID_ARGUMENT, 4); }
    if capacity == 0 { return record(OK, 4); }
    let status = match unsafe { S::pipe_read(pipe, data, capacity) } {
        Ok(done) if done > capacity => OS_ERROR,
        Ok(done) => { unsafe { got.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 4)
}
unsafe extern "C" fn pipe_write<S: Processes>(pipe: *mut c_void, data: *const u8, size: usize, written: *mut usize) -> u32 {
    if !aligned_output(written) { return record(INVALID_ARGUMENT, 5); }
    unsafe { written.write(0) };
    if pipe.is_null() || !valid_buffer(data, size) { return record(INVALID_ARGUMENT, 5); }
    if size == 0 { return record(OK, 5); }
    let status = match unsafe { S::pipe_write(pipe, data, size) } {
        Ok(0) => OS_ERROR, // no progress reported as success would loop callers forever
        Ok(done) if done > size => OS_ERROR,
        Ok(done) => { unsafe { written.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 5)
}
unsafe extern "C" fn pipe_close<S: Processes>(pipe: *mut c_void) -> u32 {
    if pipe.is_null() { return record(INVALID_ARGUMENT, 6); }
    record(crate::port::status(unsafe { S::pipe_close(pipe) }), 6)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { spawn_ok: c(0), wait_ok: c(1), terminate_ok: c(2), release_ok: c(3), pipe_read_ok: c(4), pipe_write_ok: c(5),
        pipe_close_ok: c(6), rejected_or_failed: c(FAILED) }) };
    OK
}
pub const EMPTY: Ops = Ops { spawn: None, wait: None, terminate: None, release: None, pipe_read: None, pipe_write: None, pipe_close: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's child process provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Processes;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { spawn: Some(spawn::<T<P>>), wait: Some(wait::<T<P>>), terminate: Some(terminate::<T<P>>), release: Some(release::<T<P>>),
        pipe_read: Some(pipe_read::<T<P>>), pipe_write: Some(pipe_write::<T<P>>), pipe_close: Some(pipe_close::<T<P>>), read_stats: Some(read_stats) })
}
