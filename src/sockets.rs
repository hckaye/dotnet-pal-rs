//! Internet sockets (`CAP_SOCKETS`): TCP streams and UDP datagrams over IPv4 and
//! IPv6. Addresses use the boundary's own layout, never a platform `sockaddr`.
//! Readiness is a level-triggered `poll` plus a `wake` that interrupts the poll on
//! one channel; a consumer that wants edge-triggered notification derives it from
//! the two. Channels exist so that one waiter's wake is never consumed by another.
use crate::io::{self, ACCESS_DENIED, ADDRESS_IN_USE, ADDRESS_NOT_AVAILABLE, ALREADY_CONNECTED, BROKEN_PIPE, CONNECTION_ABORTED, CONNECTION_REFUSED,
    CONNECTION_RESET, HOST_UNREACHABLE, IN_PROGRESS, MESSAGE_TOO_LARGE, NETWORK_UNREACHABLE, NOT_CONNECTED, TOO_MANY_HANDLES, WOULD_BLOCK};
use crate::kernel::TIMEOUT;
use crate::port::{Port, Sockets};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP: u64 = 268435456;
pub const IPV4: u32 = 1;
pub const IPV6: u32 = 2;
pub const STREAM: u32 = 1;
pub const DATAGRAM: u32 = 2;
pub const SHUTDOWN_READ: u32 = 1;
pub const SHUTDOWN_WRITE: u32 = 2;
pub const SHUTDOWN_BOTH: u32 = 3;
pub const RECEIVE_PEEK: u32 = 1;
pub const POLL_READ: u32 = 1;
pub const POLL_WRITE: u32 = 2;
pub const POLL_ERROR: u32 = 4;
pub const POLL_HANGUP: u32 = 8;
pub const MAX_POLL: usize = 4096;
pub const POLL_CHANNELS: u32 = 64;
/// A poll nobody can wake.
pub const NO_CHANNEL: u32 = u32::MAX;
pub const REUSE_ADDRESS: u32 = 1;
pub const NO_DELAY: u32 = 2;
pub const KEEP_ALIVE: u32 = 3;
pub const BROADCAST: u32 = 4;
pub const RECEIVE_BUFFER: u32 = 5;
pub const SEND_BUFFER: u32 = 6;
pub const IPV6_ONLY: u32 = 7;
pub const LINGER: u32 = 8;
pub const RECEIVE_TIMEOUT: u32 = 9;
pub const SEND_TIMEOUT: u32 = 10;
/// Read-only: the pending error as a status code, cleared by the read.
pub const ERROR: u32 = 11;
/// Read-only: bytes that can be received without waiting.
pub const AVAILABLE: u32 = 12;
/// Seconds a connection stays idle before the first keep-alive probe, seconds between probes, probes before it is given up.
pub const KEEP_ALIVE_IDLE: u32 = 13;
pub const KEEP_ALIVE_INTERVAL: u32 = 14;
pub const KEEP_ALIVE_COUNT: u32 = 15;
/// Hop limits of unicast and multicast traffic (0..=255).
pub const HOPS: u32 = 16;
pub const MULTICAST_HOPS: u32 = 17;
/// Whether the socket receives its own multicast traffic.
pub const MULTICAST_LOOPBACK: u32 = 18;
/// Index of the interface multicast traffic leaves through (`network` group; 0: the target's choice).
pub const MULTICAST_INTERFACE: u32 = 19;
/// Longest host name `resolve` accepts.
pub const MAX_HOST_NAME: usize = 255;
/// Most addresses one `resolve` call returns.
pub const MAX_RESOLVED: usize = 64;

/// An endpoint. `port` is in host order; an IPv4 address occupies `address[..4]`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Address { pub family: u16, pub port: u16, pub scope: u32, pub address: [u8; 16] }
impl Address {
    pub const fn v4(octets: [u8; 4], port: u16) -> Self {
        Self { family: IPV4 as u16, port, scope: 0, address: [octets[0], octets[1], octets[2], octets[3], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] }
    }
    pub const fn v6(octets: [u8; 16], port: u16, scope: u32) -> Self { Self { family: IPV6 as u16, port, scope, address: octets } }
    /// The address with the bytes its family does not use cleared; `None` for an unknown family.
    pub fn normalized(mut self) -> Option<Self> {
        match self.family as u32 {
            IPV4 => { self.scope = 0; self.address[4..].fill(0); Some(self) }
            IPV6 => Some(self),
            _ => None,
        }
    }
}
/// One `poll` entry: `requested` is `POLL_READ | POLL_WRITE`, `triggered` the answer.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PollEntry { pub socket: *mut c_void, pub requested: u32, pub triggered: u32 }

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub create: Option<unsafe extern "C" fn(u32, u32, *mut *mut c_void) -> u32>,
    pub close: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub bind: Option<unsafe extern "C" fn(*mut c_void, *const Address) -> u32>,
    pub listen: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub accept: Option<unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut Address) -> u32>,
    pub connect: Option<unsafe extern "C" fn(*mut c_void, *const Address) -> u32>,
    pub send: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize, *const Address, *mut usize) -> u32>,
    pub receive: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize, u32, *mut Address, *mut usize) -> u32>,
    pub shutdown: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub local_address: Option<unsafe extern "C" fn(*mut c_void, *mut Address) -> u32>,
    pub peer_address: Option<unsafe extern "C" fn(*mut c_void, *mut Address) -> u32>,
    pub set_blocking: Option<unsafe extern "C" fn(*mut c_void, u32) -> u32>,
    pub get_option: Option<unsafe extern "C" fn(*mut c_void, u32, *mut u64) -> u32>,
    pub set_option: Option<unsafe extern "C" fn(*mut c_void, u32, u64) -> u32>,
    pub poll: Option<unsafe extern "C" fn(*mut PollEntry, usize, u64, u32, *mut usize) -> u32>,
    pub wake: Option<unsafe extern "C" fn(u32) -> u32>,
    pub resolve: Option<unsafe extern "C" fn(*const u8, usize, u32, *mut Address, usize, *mut usize) -> u32>,
    pub host_name: Option<unsafe extern "C" fn(*mut u8, usize, *mut usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub create_ok: u64, pub close_ok: u64, pub bind_ok: u64, pub listen_ok: u64, pub accept_ok: u64, pub connect_ok: u64,
    pub send_ok: u64, pub receive_ok: u64, pub shutdown_ok: u64, pub address_ok: u64, pub option_ok: u64, pub poll_ok: u64,
    pub wake_ok: u64, pub resolve_ok: u64, pub rejected_or_failed: u64,
}
const FAILED: usize = 14;
static COUNTERS: [Counter; 15] = [const { Counter::new() }; 15];

/// The statuses a socket operation may report; anything else is a broken provider.
pub const fn sanitize(status: u32) -> u32 {
    match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | TIMEOUT | ACCESS_DENIED | WOULD_BLOCK | BROKEN_PIPE
        | CONNECTION_REFUSED | CONNECTION_RESET | CONNECTION_ABORTED | NOT_CONNECTED | ALREADY_CONNECTED | ADDRESS_IN_USE
        | ADDRESS_NOT_AVAILABLE | NETWORK_UNREACHABLE | HOST_UNREACHABLE | IN_PROGRESS | TOO_MANY_HANDLES | MESSAGE_TOO_LARGE => status,
        _ => OS_ERROR,
    }
}
fn record(status: u32, index: usize) -> u32 {
    let status = sanitize(status);
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
fn valid_buffer(p: *const u8, size: usize) -> bool { size <= isize::MAX as usize && (size == 0 || (!p.is_null() && (p as usize).checked_add(size).is_some())) }
unsafe fn input(address: *const Address) -> Option<Address> {
    if address.is_null() || !(address as usize).is_multiple_of(mem::align_of::<Address>()) { return None; }
    unsafe { address.read() }.normalized()
}
unsafe extern "C" fn create<S: Sockets>(family: u32, kind: u32, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(ptr::null_mut()) };
    if !(IPV4..=IPV6).contains(&family) || !(STREAM..=DATAGRAM).contains(&kind) { return record(INVALID_ARGUMENT, 0); }
    let status = match unsafe { S::create(family, kind) } {
        Ok(socket) if socket.is_null() => OS_ERROR,
        Ok(socket) => { unsafe { out.write(socket) }; OK }
        Err(e) => e.status(),
    };
    record(status, 0)
}
unsafe extern "C" fn close<S: Sockets>(socket: *mut c_void) -> u32 {
    if socket.is_null() { return record(INVALID_ARGUMENT, 1); }
    record(crate::port::status(unsafe { S::close(socket) }), 1)
}
unsafe extern "C" fn bind<S: Sockets>(socket: *mut c_void, address: *const Address) -> u32 {
    let Some(address) = (unsafe { input(address) }) else { return record(INVALID_ARGUMENT, 2); };
    if socket.is_null() { return record(INVALID_ARGUMENT, 2); }
    record(crate::port::status(unsafe { S::bind(socket, &address) }), 2)
}
unsafe extern "C" fn listen<S: Sockets>(socket: *mut c_void, backlog: u32) -> u32 {
    if socket.is_null() { return record(INVALID_ARGUMENT, 3); }
    record(crate::port::status(unsafe { S::listen(socket, backlog) }), 3)
}
unsafe extern "C" fn accept<S: Sockets>(socket: *mut c_void, out: *mut *mut c_void, peer: *mut Address) -> u32 {
    if !aligned_output(out) || !aligned_output(peer) { return record(INVALID_ARGUMENT, 4); }
    unsafe { out.write(ptr::null_mut()); peer.write(Address::default()); }
    if socket.is_null() { return record(INVALID_ARGUMENT, 4); }
    let status = match unsafe { S::accept(socket) } {
        Ok((accepted, _)) if accepted.is_null() => OS_ERROR,
        Ok((accepted, address)) => {
            // A peer the target cannot name stays the zero address; the connection is still good.
            unsafe { out.write(accepted); peer.write(address.normalized().unwrap_or_default()); }
            OK
        }
        Err(e) => e.status(),
    };
    record(status, 4)
}
unsafe extern "C" fn connect<S: Sockets>(socket: *mut c_void, address: *const Address) -> u32 {
    let Some(address) = (unsafe { input(address) }) else { return record(INVALID_ARGUMENT, 5); };
    if socket.is_null() { return record(INVALID_ARGUMENT, 5); }
    record(crate::port::status(unsafe { S::connect(socket, &address) }), 5)
}
unsafe extern "C" fn send<S: Sockets>(socket: *mut c_void, data: *const u8, size: usize, to: *const Address, sent: *mut usize) -> u32 {
    if !aligned_output(sent) { return record(INVALID_ARGUMENT, 6); }
    unsafe { sent.write(0) };
    if socket.is_null() || !valid_buffer(data, size) { return record(INVALID_ARGUMENT, 6); }
    let to = if to.is_null() { None } else { match unsafe { input(to) } { Some(a) => Some(a), None => return record(INVALID_ARGUMENT, 6) } };
    // An empty datagram is a real message; an empty stream send has nothing to do.
    let status = match unsafe { S::send(socket, if size == 0 { ptr::NonNull::dangling().as_ptr() } else { data }, size, to.as_ref()) } {
        Ok(done) if done > size => OS_ERROR,
        Ok(done) => { unsafe { sent.write(done) }; OK }
        Err(e) => e.status(),
    };
    record(status, 6)
}
unsafe extern "C" fn receive<S: Sockets>(socket: *mut c_void, data: *mut u8, capacity: usize, flags: u32, from: *mut Address, received: *mut usize) -> u32 {
    if !aligned_output(received) || (!from.is_null() && !aligned_output(from)) { return record(INVALID_ARGUMENT, 7); }
    unsafe { received.write(0); if !from.is_null() { from.write(Address::default()); } }
    if socket.is_null() || !valid_buffer(data, capacity) || flags & !RECEIVE_PEEK != 0 { return record(INVALID_ARGUMENT, 7); }
    let status = match unsafe { S::receive(socket, if capacity == 0 { ptr::NonNull::dangling().as_ptr() } else { data }, capacity, flags) } {
        Ok((done, _)) if done > capacity => OS_ERROR,
        Ok((done, sender)) => {
            unsafe { received.write(done) };
            if let (false, Some(sender)) = (from.is_null(), sender.and_then(Address::normalized)) { unsafe { from.write(sender) }; }
            OK
        }
        Err(e) => e.status(),
    };
    record(status, 7)
}
unsafe extern "C" fn shutdown<S: Sockets>(socket: *mut c_void, how: u32) -> u32 {
    if socket.is_null() || !(SHUTDOWN_READ..=SHUTDOWN_BOTH).contains(&how) { return record(INVALID_ARGUMENT, 8); }
    record(crate::port::status(unsafe { S::shutdown(socket, how) }), 8)
}
unsafe fn endpoint(socket: *mut c_void, out: *mut Address, query: impl FnOnce() -> crate::port::Result<Address>) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 9); }
    unsafe { out.write(Address::default()) };
    if socket.is_null() { return record(INVALID_ARGUMENT, 9); }
    let status = match query().map(Address::normalized) {
        Ok(Some(address)) => { unsafe { out.write(address) }; OK }
        Ok(None) => OS_ERROR,
        Err(e) => e.status(),
    };
    record(status, 9)
}
unsafe extern "C" fn local_address<S: Sockets>(socket: *mut c_void, out: *mut Address) -> u32 { unsafe { endpoint(socket, out, || S::local_address(socket)) } }
unsafe extern "C" fn peer_address<S: Sockets>(socket: *mut c_void, out: *mut Address) -> u32 { unsafe { endpoint(socket, out, || S::peer_address(socket)) } }
unsafe extern "C" fn set_blocking<S: Sockets>(socket: *mut c_void, blocking: u32) -> u32 {
    if socket.is_null() || blocking > 1 { return record(INVALID_ARGUMENT, 10); }
    record(crate::port::status(unsafe { S::set_blocking(socket, blocking != 0) }), 10)
}
unsafe extern "C" fn get_option<S: Sockets>(socket: *mut c_void, option: u32, value: *mut u64) -> u32 {
    if !aligned_output(value) { return record(INVALID_ARGUMENT, 10); }
    unsafe { value.write(0) };
    if socket.is_null() || !(REUSE_ADDRESS..=MULTICAST_INTERFACE).contains(&option) { return record(INVALID_ARGUMENT, 10); }
    let status = match unsafe { S::get_option(socket, option) } {
        // The pending error travels as a status code and is held to the same set.
        Ok(result) if option == ERROR => { unsafe { value.write(if result > u32::MAX as u64 { OS_ERROR } else { sanitize(result as u32) } as u64) }; OK }
        Ok(result) => { unsafe { value.write(result) }; OK }
        Err(e) => e.status(),
    };
    record(status, 10)
}
unsafe extern "C" fn set_option<S: Sockets>(socket: *mut c_void, option: u32, value: u64) -> u32 {
    if socket.is_null() || !(REUSE_ADDRESS..=MULTICAST_INTERFACE).contains(&option) || matches!(option, ERROR | AVAILABLE) { return record(INVALID_ARGUMENT, 10); }
    let flag = matches!(option, REUSE_ADDRESS | NO_DELAY | KEEP_ALIVE | BROADCAST | IPV6_ONLY | MULTICAST_LOOPBACK);
    let hops = matches!(option, HOPS | MULTICAST_HOPS);
    // Multicast traffic with no hops stays on the host. Unicast traffic, a keep-alive time and a probe count need at least one.
    let positive = matches!(option, HOPS | KEEP_ALIVE_IDLE | KEEP_ALIVE_INTERVAL | KEEP_ALIVE_COUNT);
    if (flag && value > 1) || (hops && value > 255) || (positive && value == 0) || value > u32::MAX as u64 { return record(INVALID_ARGUMENT, 10); }
    record(crate::port::status(unsafe { S::set_option(socket, option, value) }), 10)
}
unsafe extern "C" fn poll<S: Sockets>(entries: *mut PollEntry, count: usize, timeout_ns: u64, channel: u32, ready: *mut usize) -> u32 {
    if !aligned_output(ready) { return record(INVALID_ARGUMENT, 11); }
    unsafe { ready.write(0) };
    if count > MAX_POLL || (channel >= POLL_CHANNELS && channel != NO_CHANNEL) || (count != 0 && !aligned_output(entries)) { return record(INVALID_ARGUMENT, 11); }
    // SAFETY: a writable array of `count` entries is the caller's contract; an empty poll is a wait for `wake`.
    let entries = if count == 0 { &mut [][..] } else { unsafe { core::slice::from_raw_parts_mut(entries, count) } };
    for entry in entries.iter_mut() {
        entry.triggered = 0;
        if entry.socket.is_null() || entry.requested & !(POLL_READ | POLL_WRITE) != 0 { return record(INVALID_ARGUMENT, 11); }
    }
    if let Err(e) = unsafe { S::poll(entries, timeout_ns, (channel != NO_CHANNEL).then_some(channel)) } {
        for entry in entries.iter_mut() { entry.triggered = 0; }
        return record(e.status(), 11);
    }
    let mut count = 0;
    for entry in entries.iter_mut() {
        entry.triggered &= (entry.requested & (POLL_READ | POLL_WRITE)) | POLL_ERROR | POLL_HANGUP;
        if entry.triggered != 0 { count += 1; }
    }
    unsafe { ready.write(count) };
    record(OK, 11)
}
unsafe extern "C" fn wake<S: Sockets>(channel: u32) -> u32 {
    if channel >= POLL_CHANNELS { return record(INVALID_ARGUMENT, 12); }
    record(crate::port::status(S::wake(channel)), 12)
}
unsafe extern "C" fn resolve<S: Sockets>(name: *const u8, length: usize, family: u32, out: *mut Address, capacity: usize, count: *mut usize) -> u32 {
    if !aligned_output(count) { return record(INVALID_ARGUMENT, 13); }
    unsafe { count.write(0) };
    if capacity == 0 || capacity > MAX_RESOLVED || !aligned_output(out) || family > IPV6 || length > MAX_HOST_NAME { return record(INVALID_ARGUMENT, 13); }
    let Some(name) = (unsafe { io::path(name, length) }) else { return record(INVALID_ARGUMENT, 13); };
    for i in 0..capacity { unsafe { out.add(i).write(Address::default()) }; }
    let status = match unsafe { S::resolve(name, family, out, capacity) } {
        Ok(0) => NOT_FOUND,
        Ok(found) if found > capacity => OS_ERROR,
        Ok(found) => {
            let mut status = OK;
            for i in 0..found {
                match unsafe { out.add(i).read() }.normalized() {
                    Some(address) if family == 0 || address.family as u32 == family => unsafe { out.add(i).write(address) },
                    _ => status = OS_ERROR,
                }
            }
            if status == OK { unsafe { count.write(found) }; }
            status
        }
        Err(e) => e.status(),
    };
    if status != OK { for i in 0..capacity { unsafe { out.add(i).write(Address::default()) }; } }
    // NOT_FOUND belongs to resolve alone, so it bypasses the shared set.
    if status == NOT_FOUND { COUNTERS[FAILED].increment(); return status; }
    record(status, 13)
}
unsafe extern "C" fn host_name<S: Sockets>(out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    if !aligned_output(needed) || !valid_buffer(out, capacity) { return record(INVALID_ARGUMENT, 13); }
    unsafe { needed.write(0) };
    let status = unsafe { io::text(out, capacity, needed, crate::runtime::MAX_NAME + 1, || S::host_name(out, capacity)) };
    if status == BUFFER_TOO_SMALL { COUNTERS[FAILED].increment(); return status; }
    record(status, 13)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { create_ok: c(0), close_ok: c(1), bind_ok: c(2), listen_ok: c(3), accept_ok: c(4), connect_ok: c(5), send_ok: c(6),
        receive_ok: c(7), shutdown_ok: c(8), address_ok: c(9), option_ok: c(10), poll_ok: c(11), wake_ok: c(12), resolve_ok: c(13),
        rejected_or_failed: c(FAILED) }) };
    OK
}
pub const EMPTY: Ops = Ops { create: None, close: None, bind: None, listen: None, accept: None, connect: None, send: None, receive: None,
    shutdown: None, local_address: None, peer_address: None, set_blocking: None, get_option: None, set_option: None, poll: None, wake: None,
    resolve: None, host_name: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's socket provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Sockets;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { create: Some(create::<T<P>>), close: Some(close::<T<P>>), bind: Some(bind::<T<P>>), listen: Some(listen::<T<P>>),
        accept: Some(accept::<T<P>>), connect: Some(connect::<T<P>>), send: Some(send::<T<P>>), receive: Some(receive::<T<P>>),
        shutdown: Some(shutdown::<T<P>>), local_address: Some(local_address::<T<P>>), peer_address: Some(peer_address::<T<P>>),
        set_blocking: Some(set_blocking::<T<P>>), get_option: Some(get_option::<T<P>>), set_option: Some(set_option::<T<P>>),
        poll: Some(poll::<T<P>>), wake: Some(wake::<T<P>>), resolve: Some(resolve::<T<P>>), host_name: Some(host_name::<T<P>>),
        read_stats: Some(read_stats) })
}
