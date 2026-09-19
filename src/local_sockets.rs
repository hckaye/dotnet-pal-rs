//! Unix domain sockets (`CAP_LOCAL_SOCKETS`). A local socket is a socket of the
//! sockets group created with [`crate::sockets::LOCAL`]; this group binds it to a
//! path, connects it to one and answers for the paths and the peer's user. The type
//! that provides [`crate::port::Sockets`] provides this one.
use crate::io::{self, ACCESS_DENIED, ADDRESS_IN_USE, ALREADY_CONNECTED, CONNECTION_REFUSED, IN_PROGRESS, NAME_TOO_LONG, NOT_CONNECTED, NOT_DIRECTORY,
    READ_ONLY, WOULD_BLOCK};
use crate::kernel::TIMEOUT;
use crate::port::{LocalSockets, Port};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem};

pub const CAP: u64 = 274877906944;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub bind: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> u32>,
    pub connect: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> u32>,
    pub address: Option<unsafe extern "C" fn(*mut c_void, u32, *mut u8, usize, *mut usize) -> u32>,
    pub peer_user: Option<unsafe extern "C" fn(*mut c_void, *mut u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub bind_ok: u64, pub connect_ok: u64, pub address_ok: u64, pub peer_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 4;
static COUNTERS: [Counter; 5] = [const { Counter::new() }; 5];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY => status,
        NOT_FOUND | ACCESS_DENIED | NAME_TOO_LONG | NOT_DIRECTORY | READ_ONLY if index <= 1 => status,
        ADDRESS_IN_USE if index == 0 => status,
        CONNECTION_REFUSED | IN_PROGRESS | WOULD_BLOCK | ALREADY_CONNECTED | TIMEOUT if index == 1 => status,
        NOT_FOUND | BUFFER_TOO_SMALL | NOT_CONNECTED if index == 2 => status,
        NOT_CONNECTED if index == 3 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
unsafe extern "C" fn bind<L: LocalSockets>(socket: *mut c_void, path: *const u8, path_length: usize) -> u32 {
    let Some(path) = (unsafe { io::path(path, path_length) }) else { return record(INVALID_ARGUMENT, 0); };
    if socket.is_null() { return record(INVALID_ARGUMENT, 0); }
    record(crate::port::status(unsafe { L::bind(socket, path) }), 0)
}
unsafe extern "C" fn connect<L: LocalSockets>(socket: *mut c_void, path: *const u8, path_length: usize) -> u32 {
    let Some(path) = (unsafe { io::path(path, path_length) }) else { return record(INVALID_ARGUMENT, 1); };
    if socket.is_null() { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(unsafe { L::connect(socket, path) }), 1)
}
unsafe extern "C" fn address<L: LocalSockets>(socket: *mut c_void, peer: u32, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    let buffer = capacity <= isize::MAX as usize && (capacity == 0 || (!out.is_null() && (out as usize).checked_add(capacity).is_some()));
    if !aligned_output(needed) || !buffer { return record(INVALID_ARGUMENT, 2); }
    unsafe { needed.write(0) };
    if socket.is_null() || peer > 1 { return record(INVALID_ARGUMENT, 2); }
    record(unsafe { io::text(out, capacity, needed, crate::runtime::MAX_NAME + 1, || L::address(socket, peer != 0, out, capacity)) }, 2)
}
unsafe extern "C" fn peer_user<L: LocalSockets>(socket: *mut c_void, user_id: *mut u32) -> u32 {
    if !aligned_output(user_id) { return record(INVALID_ARGUMENT, 3); }
    // No user is the safe answer to leave behind: a consumer compares it with its own and must not find a match by accident.
    unsafe { user_id.write(u32::MAX) };
    if socket.is_null() { return record(INVALID_ARGUMENT, 3); }
    match unsafe { L::peer_user(socket) } {
        Ok(id) => { unsafe { user_id.write(id) }; record(OK, 3) }
        Err(e) => record(e.status(), 3),
    }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { bind_ok: c(0), connect_ok: c(1), address_ok: c(2), peer_ok: c(3), rejected_or_failed: c(FAILED) }) };
    OK
}
pub const EMPTY: Ops = Ops { bind: None, connect: None, address: None, peer_user: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's local socket provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::LocalSockets;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { bind: Some(bind::<T<P>>), connect: Some(connect::<T<P>>), address: Some(address::<T<P>>), peer_user: Some(peer_user::<T<P>>),
        read_stats: Some(read_stats) })
}
