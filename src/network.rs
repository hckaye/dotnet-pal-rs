//! Network interfaces, reverse lookup and multicast membership (`CAP_NETWORK`).
//! Membership takes the handles of the sockets group, so the type that provides
//! [`crate::port::Sockets`] provides this one.
use crate::io::{ACCESS_DENIED, ADDRESS_IN_USE, ADDRESS_NOT_AVAILABLE};
use crate::kernel::TIMEOUT;
use crate::port::{Network, Port};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::sockets::{Address, IPV4, IPV6};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::{ffi::c_void, mem};

pub const CAP: u64 = 137438953472;
pub const INTERFACE_UNKNOWN: u32 = 0;
pub const INTERFACE_ETHERNET: u32 = 1;
pub const INTERFACE_LOOPBACK: u32 = 2;
pub const INTERFACE_WIRELESS: u32 = 3;
pub const INTERFACE_POINT_TO_POINT: u32 = 4;
pub const INTERFACE_TUNNEL: u32 = 5;
pub const LINK_UNKNOWN: u32 = 0;
pub const LINK_UP: u32 = 1;
pub const LINK_DOWN: u32 = 2;
/// Interface flag: the interface carries multicast traffic.
pub const MULTICAST: u32 = 1;
/// Longest host name a reverse lookup answers with, NUL included.
pub const MAX_HOST_NAME: usize = 256;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Interface {
    pub index: u32, pub kind: u32, pub state: u32, pub flags: u32, pub mtu: u32, pub hardware_address_length: u32,
    pub speed_bps: u64, pub hardware_address: [u8; 8], pub name: [u8; 64],
}
impl Interface {
    pub const EMPTY: Self = Self { index: 0, kind: 0, state: 0, flags: 0, mtu: 0, hardware_address_length: 0, speed_bps: 0, hardware_address: [0; 8], name: [0; 64] };
    /// Stores `name`, cut to the 63 bytes the field holds.
    pub fn set_name(&mut self, name: &[u8]) {
        let length = name.iter().position(|b| *b == 0).unwrap_or(name.len()).min(63);
        self.name = [0; 64];
        self.name[..length].copy_from_slice(&name[..length]);
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterfaceAddress { pub interface_index: u32, pub prefix_length: u32, pub address: Address }
impl InterfaceAddress {
    pub const EMPTY: Self = Self { interface_index: 0, prefix_length: 0, address: Address { family: 0, port: 0, scope: 0, address: [0; 16] } };
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub interface_entry: Option<unsafe extern "C" fn(usize, *mut Interface, usize) -> u32>,
    pub address_entry: Option<unsafe extern "C" fn(usize, *mut InterfaceAddress, usize) -> u32>,
    pub reverse_lookup: Option<unsafe extern "C" fn(*const Address, *mut u8, usize, *mut usize) -> u32>,
    pub membership: Option<unsafe extern "C" fn(*mut c_void, *const Address, u32, u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub interface_ok: u64, pub address_ok: u64, pub lookup_ok: u64, pub membership_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 4;
static COUNTERS: [Counter; 5] = [const { Counter::new() }; 5];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY => status,
        NOT_FOUND if index <= 2 => status,
        BUFFER_TOO_SMALL | TIMEOUT if index == 2 => status,
        ADDRESS_IN_USE | ADDRESS_NOT_AVAILABLE | ACCESS_DENIED | NOT_FOUND if index == 3 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
fn family(address: &Address) -> bool { matches!(address.family as u32, IPV4 | IPV6) }
unsafe extern "C" fn interface_entry<N: Network>(index: usize, out: *mut Interface, out_size: usize) -> u32 {
    if !aligned_output(out) || out_size < mem::size_of::<Interface>() { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(Interface::EMPTY) };
    match N::interface_entry(index) {
        // Index 0 means "the target's choice" elsewhere, a name is how managed code tells interfaces apart.
        Ok(i) if i.index == 0 || i.name[0] == 0 || i.name[63] != 0 || i.kind > INTERFACE_TUNNEL || i.state > LINK_DOWN || i.flags & !MULTICAST != 0
            || i.hardware_address_length > 8 => record(OS_ERROR, 0),
        Ok(i) => { unsafe { out.write(i) }; record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn address_entry<N: Network>(index: usize, out: *mut InterfaceAddress, out_size: usize) -> u32 {
    if !aligned_output(out) || out_size < mem::size_of::<InterfaceAddress>() { return record(INVALID_ARGUMENT, 1); }
    unsafe { out.write(InterfaceAddress::EMPTY) };
    match N::address_entry(index) {
        Ok(a) if a.interface_index == 0 || !family(&a.address) || a.address.port != 0
            || a.prefix_length > if a.address.family as u32 == IPV4 { 32 } else { 128 } => record(OS_ERROR, 1),
        Ok(a) => { unsafe { out.write(a) }; record(OK, 1) }
        Err(e) => record(e.status(), 1),
    }
}
unsafe extern "C" fn reverse_lookup<N: Network>(address: *const Address, out: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    let buffer = capacity <= isize::MAX as usize && (capacity == 0 || (!out.is_null() && (out as usize).checked_add(capacity).is_some()));
    if !aligned_output(needed) || !buffer { return record(INVALID_ARGUMENT, 2); }
    unsafe { needed.write(0) };
    if address.is_null() || !(address as usize).is_multiple_of(mem::align_of::<Address>()) { return record(INVALID_ARGUMENT, 2); }
    // SAFETY: a readable address is the caller's contract.
    let address = unsafe { address.read() };
    if !family(&address) { return record(INVALID_ARGUMENT, 2); }
    record(unsafe { crate::io::text(out, capacity, needed, MAX_HOST_NAME, || N::reverse_lookup(&address, out, capacity)) }, 2)
}
unsafe extern "C" fn membership<N: Network>(socket: *mut c_void, group: *const Address, interface_index: u32, join: u32) -> u32 {
    if socket.is_null() || join > 1 || group.is_null() || !(group as usize).is_multiple_of(mem::align_of::<Address>()) { return record(INVALID_ARGUMENT, 3); }
    // SAFETY: a readable address is the caller's contract.
    let group = unsafe { group.read() };
    // 224.0.0.0/4 and ff00::/8 are the multicast ranges; anything else is no group.
    let multicast = match group.family as u32 { IPV4 => group.address[0] & 0xf0 == 0xe0, IPV6 => group.address[0] == 0xff, _ => false };
    if !multicast { return record(INVALID_ARGUMENT, 3); }
    // A group is a family and an address. The interface travels as its own argument, so whatever else the caller left in the structure goes.
    let mut address = [0u8; 16];
    let used = if group.family as u32 == IPV4 { 4 } else { 16 };
    address[..used].copy_from_slice(&group.address[..used]);
    let group = Address { family: group.family, port: 0, scope: 0, address };
    record(crate::port::status(unsafe { N::membership(socket, &group, interface_index, join != 0) }), 3)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { interface_ok: c(0), address_ok: c(1), lookup_ok: c(2), membership_ok: c(3), rejected_or_failed: c(FAILED) }) };
    OK
}
pub const EMPTY: Ops = Ops { interface_entry: None, address_entry: None, reverse_lookup: None, membership: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's network information provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Network;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { interface_entry: Some(interface_entry::<T<P>>), address_entry: Some(address_entry::<T<P>>), reverse_lookup: Some(reverse_lookup::<T<P>>),
        membership: Some(membership::<T<P>>), read_stats: Some(read_stats) })
}
