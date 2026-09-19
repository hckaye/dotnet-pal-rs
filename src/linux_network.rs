//! Linux provider for the network group: getifaddrs(3) for the interfaces and their
//! addresses, the MTU ioctl, sysfs for the link speed and the wireless marker,
//! getnameinfo(3) for the reverse lookup and the membership socket options on the
//! descriptors of the sockets provider. No Rust heap; the lists are CRT allocations.
//!
//! Both enumerations answer from one snapshot. Index 0 of either takes a new one,
//! as does the first call of all, and every call reads its entry under the lock
//! that guards the exchange: two threads that enumerate at once each get whole
//! entries and an end, from the newest snapshot. An interface is a distinct name
//! (an alias label such as "eth0:1" names its interface), and one whose index the
//! kernel no longer knows is left out, so the indices stay dense.
use crate::linux::Linux;
use crate::linux_sockets::{descriptor, native, shape};
use crate::network::{Interface, InterfaceAddress, INTERFACE_ETHERNET, INTERFACE_LOOPBACK, INTERFACE_POINT_TO_POINT, INTERFACE_TUNNEL, INTERFACE_UNKNOWN,
    INTERFACE_WIRELESS, LINK_DOWN, LINK_UNKNOWN, LINK_UP, MULTICAST};
use crate::port::{self, Error, Result};
use crate::sockets::{Address, IPV4, IPV6};
use core::{cell::UnsafeCell, ffi::{c_void, CStr}, mem, ptr};

/// ARPHRD_IP6GRE of <linux/if_arp.h>; `libc` does not carry it.
const ARPHRD_IP6GRE: u16 = 823;
struct Shared<T>(UnsafeCell<T>);
// SAFETY: the snapshot is only touched between `pthread_mutex_lock` and `pthread_mutex_unlock` on LOCK.
unsafe impl<T> Sync for Shared<T> {}
struct Snapshot { taken: bool, interfaces: *mut Interface, interface_count: usize, addresses: *mut InterfaceAddress, address_count: usize }
static LOCK: Shared<libc::pthread_mutex_t> = Shared(UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER));
static SNAPSHOT: Shared<Snapshot> = Shared(UnsafeCell::new(Snapshot { taken: false, interfaces: ptr::null_mut(), interface_count: 0, addresses: ptr::null_mut(), address_count: 0 }));

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// `/sys/class/net/<name>/<leaf>` with its terminator; `None` for a name sysfs cannot hold.
fn sysfs(name: &[u8], leaf: &[u8]) -> Option<[u8; 96]> {
    const ROOT: &[u8] = b"/sys/class/net/";
    let mut path = [0u8; 96];
    if name.contains(&b'/') || ROOT.len() + name.len() + 1 + leaf.len() >= path.len() { return None; }
    let mut at = 0;
    for part in [ROOT, name, b"/", leaf] { path[at..at + part.len()].copy_from_slice(part); at += part.len(); }
    Some(path)
}
/// Bits per second from sysfs, which counts megabits: 0 when the file is missing, unreadable (loopback is EINVAL) or says -1.
fn speed(name: &[u8]) -> u64 {
    let Some(path) = sysfs(name, b"speed") else { return 0; };
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 { return 0; }
    let mut text = [0u8; 32];
    let length = unsafe { libc::read(fd, text.as_mut_ptr().cast(), text.len()) };
    unsafe { libc::close(fd) };
    let mut megabits = 0u64;
    for byte in &text[..length.max(0) as usize] {
        match byte { b'0'..=b'9' => megabits = megabits.saturating_mul(10).saturating_add((byte - b'0') as u64), b'\n' => break, _ => return 0 }
    }
    megabits.saturating_mul(1_000_000)
}
fn wireless(name: &[u8]) -> bool { sysfs(name, b"wireless").is_some_and(|path| unsafe { libc::access(path.as_ptr().cast(), libc::F_OK) } == 0) }
fn mtu(probe: i32, name: &[u8]) -> u32 {
    let mut request: libc::ifreq = unsafe { mem::zeroed() };
    if probe < 0 || name.len() >= request.ifr_name.len() { return 0; }
    for (to, from) in request.ifr_name.iter_mut().zip(name) { *to = *from as _; }
    if unsafe { libc::ioctl(probe, libc::SIOCGIFMTU as _, ptr::addr_of_mut!(request)) } != 0 { return 0; }
    // SAFETY: SIOCGIFMTU fills the integer member of the union.
    unsafe { request.ifr_ifru.ifru_mtu }.max(0) as u32
}
fn state(flags: u32) -> u32 {
    let (up, running) = (flags & libc::IFF_UP as u32 != 0, flags & libc::IFF_RUNNING as u32 != 0);
    if up && running { LINK_UP } else if !up { LINK_DOWN } else { LINK_UNKNOWN }
}
/// The ones of a netmask; an address without one is a host of its own.
unsafe fn prefix(mask: *const libc::sockaddr, family: i32, host: u32) -> u32 {
    if mask.is_null() || unsafe { (*mask).sa_family } as i32 != family { return host; }
    if family == libc::AF_INET { return unsafe { mask.cast::<libc::sockaddr_in>().read_unaligned() }.sin_addr.s_addr.count_ones(); }
    unsafe { mask.cast::<libc::sockaddr_in6>().read_unaligned() }.sin6_addr.s6_addr.iter().map(|byte| byte.count_ones()).sum()
}
/// Fills both lists from one getifaddrs answer. `interfaces` and `addresses` each hold as many entries as the answer has.
unsafe fn fill(list: *const libc::ifaddrs, probe: i32, interfaces: *mut Interface, addresses: *mut InterfaceAddress) -> (usize, usize) {
    let (mut interface_count, mut address_count, mut next) = (0, 0, list);
    while let Some(entry) = unsafe { next.as_ref() } {
        next = entry.ifa_next;
        if entry.ifa_name.is_null() { continue; }
        let label = unsafe { CStr::from_ptr(entry.ifa_name) }.to_bytes();
        let name = &label[..label.iter().position(|b| *b == b':').unwrap_or(label.len())];
        let mut text = [0u8; 64];
        if name.is_empty() || name.len() >= text.len() { continue; }
        text[..name.len()].copy_from_slice(name);
        // SAFETY: the entries below `interface_count` were written by earlier rounds of this loop.
        let known = unsafe { core::slice::from_raw_parts_mut(interfaces, interface_count) };
        let slot = match known.iter().position(|i| i.name == text) {
            Some(slot) => slot,
            None => {
                let index = unsafe { libc::if_nametoindex(text.as_ptr().cast()) };
                if index == 0 { continue; }
                let kind = if entry.ifa_flags & libc::IFF_POINTOPOINT as u32 != 0 { INTERFACE_POINT_TO_POINT } else { INTERFACE_UNKNOWN };
                let flags = if entry.ifa_flags & libc::IFF_MULTICAST as u32 != 0 { MULTICAST } else { 0 };
                unsafe { interfaces.add(interface_count).write(Interface { index, kind, state: state(entry.ifa_flags), flags, mtu: mtu(probe, name), speed_bps: speed(name), name: text, ..Interface::EMPTY }) };
                interface_count += 1;
                interface_count - 1
            }
        };
        if entry.ifa_addr.is_null() { continue; }
        let interface = unsafe { &mut *interfaces.add(slot) };
        match unsafe { (*entry.ifa_addr).sa_family } as i32 {
            libc::AF_PACKET => {
                let link = unsafe { entry.ifa_addr.cast::<libc::sockaddr_ll>().read_unaligned() };
                interface.kind = match link.sll_hatype {
                    libc::ARPHRD_LOOPBACK => INTERFACE_LOOPBACK,
                    libc::ARPHRD_ETHER => if wireless(name) { INTERFACE_WIRELESS } else { INTERFACE_ETHERNET },
                    libc::ARPHRD_PPP => INTERFACE_POINT_TO_POINT,
                    libc::ARPHRD_TUNNEL | libc::ARPHRD_TUNNEL6 | libc::ARPHRD_SIT | libc::ARPHRD_IPGRE | ARPHRD_IP6GRE => INTERFACE_TUNNEL,
                    _ => interface.kind,
                };
                // An address longer than the boundary carries (InfiniBand has 20 bytes) is none rather than a part of one.
                let length = link.sll_halen as usize;
                if length <= interface.hardware_address.len() {
                    interface.hardware_address_length = length as u32;
                    interface.hardware_address[..length].copy_from_slice(&link.sll_addr[..length]);
                }
            }
            libc::AF_INET => {
                let v4 = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in>().read_unaligned() };
                let found = InterfaceAddress { interface_index: interface.index, prefix_length: unsafe { prefix(entry.ifa_netmask, libc::AF_INET, 32) }, address: Address::v4(v4.sin_addr.s_addr.to_ne_bytes(), 0) };
                unsafe { addresses.add(address_count).write(found) };
                address_count += 1;
            }
            libc::AF_INET6 => {
                let v6 = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in6>().read_unaligned() }.sin6_addr.s6_addr;
                // fe80::/10 is only an address together with its link.
                let scope = if v6[0] == 0xfe && v6[1] & 0xc0 == 0x80 { interface.index } else { 0 };
                let found = InterfaceAddress { interface_index: interface.index, prefix_length: unsafe { prefix(entry.ifa_netmask, libc::AF_INET6, 128) }, address: Address::v6(v6, 0, scope) };
                unsafe { addresses.add(address_count).write(found) };
                address_count += 1;
            }
            _ => {}
        }
    }
    (interface_count, address_count)
}
/// Replaces the snapshot; a failure leaves the one before it in place.
unsafe fn take(snapshot: &mut Snapshot) -> Result<()> {
    let mut list: *mut libc::ifaddrs = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 { return Err(if errno() == libc::ENOMEM { Error::OutOfMemory } else { Error::Os }); }
    let (mut entries, mut next) = (1, list as *const libc::ifaddrs);
    while let Some(entry) = unsafe { next.as_ref() } { entries += 1; next = entry.ifa_next; }
    let interfaces = unsafe { libc::calloc(entries, mem::size_of::<Interface>()) }.cast::<Interface>();
    let addresses = unsafe { libc::calloc(entries, mem::size_of::<InterfaceAddress>()) }.cast::<InterfaceAddress>();
    if interfaces.is_null() || addresses.is_null() {
        unsafe { libc::free(interfaces.cast()); libc::free(addresses.cast()); libc::freeifaddrs(list); }
        return Err(Error::OutOfMemory);
    }
    // The MTU ioctl wants a socket of any kind; a machine without IPv4 has the other family.
    let mut probe = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if probe < 0 { probe = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) }; }
    let (interface_count, address_count) = unsafe { fill(list, probe, interfaces, addresses) };
    unsafe {
        if probe >= 0 { libc::close(probe); }
        libc::freeifaddrs(list);
        libc::free(snapshot.interfaces.cast());
        libc::free(snapshot.addresses.cast());
    }
    *snapshot = Snapshot { taken: true, interfaces, interface_count, addresses, address_count };
    Ok(())
}
fn entry<T: Copy>(index: usize, list: impl FnOnce(&Snapshot) -> (*const T, usize)) -> Result<T> {
    if unsafe { libc::pthread_mutex_lock(LOCK.0.get()) } != 0 { return Err(Error::Os); }
    // SAFETY: the lock is held until the entry has been copied out.
    let snapshot = unsafe { &mut *SNAPSHOT.0.get() };
    let result = if index == 0 || !snapshot.taken { unsafe { take(snapshot) } } else { Ok(()) }.and_then(|()| {
        let (items, count) = list(snapshot);
        if index < count { Ok(unsafe { items.add(index).read() }) } else { Err(Error::NotFound) }
    });
    unsafe { libc::pthread_mutex_unlock(LOCK.0.get()) };
    result
}

impl port::Network for Linux {
    fn interface_entry(index: usize) -> Result<Interface> { entry(index, |s| (s.interfaces.cast_const(), s.interface_count)) }
    fn address_entry(index: usize) -> Result<InterfaceAddress> { entry(index, |s| (s.addresses.cast_const(), s.address_count)) }
    unsafe fn reverse_lookup(address: &Address, out: *mut u8, capacity: usize) -> Result<usize> {
        let (storage, length) = native(address)?;
        let mut host = [0u8; libc::NI_MAXHOST as usize];
        let code = loop {
            let code = unsafe { libc::getnameinfo(ptr::addr_of!(storage).cast(), length, host.as_mut_ptr().cast(), host.len() as libc::socklen_t, ptr::null_mut(), 0, libc::NI_NAMEREQD) };
            if code != libc::EAI_SYSTEM || errno() != libc::EINTR { break code; }
        };
        match code {
            0 => {}
            libc::EAI_NONAME => return Err(Error::NotFound),
            libc::EAI_AGAIN => return Err(Error::Timeout),
            libc::EAI_MEMORY => return Err(Error::OutOfMemory),
            _ => return Err(Error::Os),
        }
        let length = host.iter().position(|b| *b == 0).unwrap_or(host.len() - 1);
        // The boundary has no empty text, and an address whose name is empty has none.
        if length == 0 { return Err(Error::NotFound); }
        if length < capacity { unsafe { ptr::copy_nonoverlapping(host.as_ptr(), out, length); out.add(length).write(0); } }
        Ok(length + 1)
    }
    unsafe fn membership(socket: *mut c_void, group: &Address, interface_index: u32, join: bool) -> Result<()> {
        let fd = unsafe { descriptor(socket) }?;
        let (stream, v6) = shape(fd)?;
        // The kernel calls a stream socket EPROTO and lets an IPv6 socket join an IPv4 group (measured); both are the caller's mistake here.
        if stream || v6 != (group.family as u32 == IPV6) { return Err(Error::InvalidArgument); }
        let Ok(interface) = i32::try_from(interface_index) else { return Err(Error::NotFound); };
        let a = group.address;
        let rc = match group.family as u32 {
            IPV4 => {
                let request = libc::ip_mreqn { imr_multiaddr: libc::in_addr { s_addr: u32::from_ne_bytes([a[0], a[1], a[2], a[3]]) }, imr_address: libc::in_addr { s_addr: 0 }, imr_ifindex: interface };
                unsafe { libc::setsockopt(fd, libc::IPPROTO_IP, if join { libc::IP_ADD_MEMBERSHIP } else { libc::IP_DROP_MEMBERSHIP }, ptr::addr_of!(request).cast(), mem::size_of::<libc::ip_mreqn>() as libc::socklen_t) }
            }
            _ => {
                let request = libc::ipv6_mreq { ipv6mr_multiaddr: libc::in6_addr { s6_addr: a }, ipv6mr_interface: interface_index };
                unsafe { libc::setsockopt(fd, libc::IPPROTO_IPV6, if join { libc::IPV6_ADD_MEMBERSHIP } else { libc::IPV6_DROP_MEMBERSHIP }, ptr::addr_of!(request).cast(), mem::size_of::<libc::ipv6_mreq>() as libc::socklen_t) }
            }
        };
        if rc == 0 { return Ok(()); }
        // EADDRINUSE is a group the socket is in already, EADDRNOTAVAIL one it is not in, ENODEV an interface that does not exist.
        Err(match errno() {
            libc::EADDRINUSE => Error::AddressInUse, libc::EADDRNOTAVAIL => Error::AddressNotAvailable, libc::ENODEV => Error::NotFound,
            libc::EACCES | libc::EPERM => Error::AccessDenied, libc::ENOPROTOOPT => Error::Unsupported, libc::EINVAL => Error::InvalidArgument,
            libc::ENOMEM | libc::ENOBUFS => Error::OutOfMemory,
            _ => Error::Os,
        })
    }
}
