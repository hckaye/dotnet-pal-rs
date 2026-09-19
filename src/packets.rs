//! Where a datagram arrived (`CAP_PACKETS`): `receive` is the receive of the sockets
//! group for a datagram or a raw socket and also reports the interface and the
//! destination address of the datagram. The type that provides
//! [`crate::port::Sockets`] provides this one.
use crate::io::{ACCESS_DENIED, BROKEN_PIPE, CONNECTION_REFUSED, CONNECTION_RESET, HOST_UNREACHABLE, MESSAGE_TOO_LARGE, NETWORK_UNREACHABLE, NOT_CONNECTED, WOULD_BLOCK};
use crate::kernel::TIMEOUT;
use crate::port::{Packets, Port};
use crate::sockets::{Address, RECEIVE_PEEK};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};

pub const CAP: u64 = 2199023255552;

/// The interface a datagram arrived through (0 when the target does not say) and the address it was sent to.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Info { pub interface_index: u32, pub reserved: u32, pub destination: Address }
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    #[allow(clippy::type_complexity)]
    pub receive: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize, u32, *mut Address, *mut Info, usize, *mut usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub receive_ok: u64, pub rejected_or_failed: u64 }
static COUNTERS: [Counter; 2] = [const { Counter::new() }; 2];

fn record(status: u32) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | WOULD_BLOCK | TIMEOUT | CONNECTION_REFUSED | CONNECTION_RESET | NOT_CONNECTED | BROKEN_PIPE => status,
        // What the network reported about an earlier send, for a socket that asked for it with RECEIVE_ERRORS.
        HOST_UNREACHABLE | NETWORK_UNREACHABLE | MESSAGE_TOO_LARGE | ACCESS_DENIED => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { 0 } else { 1 }].increment();
    status
}
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn receive<K: Packets>(socket: *mut c_void, data: *mut u8, capacity: usize, flags: u32, from: *mut Address, info: *mut Info, info_size: usize,
    received: *mut usize) -> u32 {
    if !aligned_output(received) || !aligned_output(info) || info_size < mem::size_of::<Info>() || (!from.is_null() && !aligned_output(from)) { return record(INVALID_ARGUMENT); }
    unsafe { received.write(0); info.write(Info::default()); if !from.is_null() { from.write(Address::default()); } }
    let buffer = capacity <= isize::MAX as usize && (capacity == 0 || (!data.is_null() && (data as usize).checked_add(capacity).is_some()));
    if socket.is_null() || !buffer || flags & !RECEIVE_PEEK != 0 { return record(INVALID_ARGUMENT); }
    let status = match unsafe { K::receive(socket, if capacity == 0 { ptr::NonNull::dangling().as_ptr() } else { data }, capacity, flags) } {
        Ok((done, _, _)) if done > capacity => OS_ERROR,
        Ok((done, sender, arrived)) => {
            // A destination is an IP address without a port, or nothing: the option may be off, and a target may not know.
            let destination = arrived.destination.normalized().filter(|a| a.port == 0).unwrap_or_default();
            unsafe {
                received.write(done);
                info.write(Info { interface_index: arrived.interface_index, reserved: 0, destination });
                if let (false, Some(sender)) = (from.is_null(), sender.and_then(Address::reported)) { from.write(sender); }
            }
            OK
        }
        Err(e) => e.status(),
    };
    record(status)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { receive_ok: COUNTERS[0].load(), rejected_or_failed: COUNTERS[1].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { receive: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's packet information provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Packets;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { receive: Some(receive::<T<P>>), read_stats: Some(read_stats) })
}
