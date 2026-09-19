//! Network interfaces, reverse lookup and multicast membership of the desktop port.
//!
//! Unix: getifaddrs(3). An interface is a distinct name of that list (an alias label
//! such as "eth0:1" names its interface); its index is if_nametoindex, its MTU the
//! SIOCGIFMTU ioctl. Linux takes the type and the hardware address from the
//! AF_PACKET entry, the speed from sysfs and calls an Ethernet device with a
//! `wireless` directory there wireless. macOS takes them from the AF_LINK entry and
//! the `if_data` behind it, whose 32-bit line speed cannot tell its largest value
//! from a faster link, so that value is reported as unknown; Wi-Fi is IFT_ETHER
//! there and reads as Ethernet. macOS cuts a netmask's `sa_len` to its last set
//! byte (5 for 255.0.0.0, measured), so only the bytes inside it are counted.
//!
//! Both enumerations answer from one snapshot. Index 0 of either takes a new one,
//! as does the first call of all, and every call copies its entry out under the
//! lock: threads that enumerate at once each get whole entries and an end.
//!
//! Membership is `socket2` on the handles of the sockets provider. macOS joins a
//! group on an interface of its own choice when the index names none (measured), so
//! an index nobody has is answered here. An IPv6 socket that carries both families
//! joins an IPv4 group at the IPv4 level on Linux, which macOS refuses (EINVAL); macOS
//! takes the group mapped into IPv6 (::ffff:a.b.c.d) at the IPv6 level instead. Either
//! OS lets an IPv6-only socket join that way and delivers nothing to it (all measured,
//! Linux 6.12 and Darwin 25), so the socket is asked first. Reverse lookup is getnameinfo(3).
//! Windows offers none of this: the capability is absent there.

mod unix {
    use super::super::sockets::{native, tuning, Socket};
    use super::super::{borrow, Std};
    use dotnet_pal_rs::network::{self, Interface, InterfaceAddress, INTERFACE_POINT_TO_POINT, INTERFACE_UNKNOWN, LINK_DOWN, LINK_UNKNOWN, LINK_UP, MULTICAST};
    use dotnet_pal_rs::port::{self, Error, Result};
    use dotnet_pal_rs::sockets::{Address, IPV4, IPV6};
    use socket2::InterfaceIndexOrAddress;
    use std::{ffi::{c_void, CStr}, io, net::{Ipv4Addr, Ipv6Addr}, ptr, sync::Mutex};

    type Snapshot = (Vec<Interface>, Vec<InterfaceAddress>);
    static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
    /// _IOWR('i', 51, struct ifreq) of <sys/sockio.h>; `libc` 0.2.169 does not carry it for Apple targets.


    const SIOCGIFMTU: libc::c_ulong = libc::SIOCGIFMTU as libc::c_ulong;

    fn mtu(probe: i32, name: &[u8]) -> u32 {
        let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
        if probe < 0 || name.len() >= request.ifr_name.len() { return 0; }
        for (to, from) in request.ifr_name.iter_mut().zip(name) { *to = *from as _; }
        if unsafe { libc::ioctl(probe, SIOCGIFMTU as _, ptr::addr_of_mut!(request)) } != 0 { return 0; }
        // SAFETY: SIOCGIFMTU fills the integer member of the union.
        unsafe { request.ifr_ifru.ifru_mtu }.max(0) as u32
    }
    fn state(flags: u32) -> u32 {
        let (up, running) = (flags & libc::IFF_UP as u32 != 0, flags & libc::IFF_RUNNING as u32 != 0);
        if up && running { LINK_UP } else if !up { LINK_DOWN } else { LINK_UNKNOWN }
    }
    /// The ones of a netmask; an address without one is a host of its own.
    unsafe fn prefix(mask: *const libc::sockaddr, family: i32, host: u32) -> u32 {
        if mask.is_null() { return host; }
        let (start, size) = if family == libc::AF_INET { (std::mem::offset_of!(libc::sockaddr_in, sin_addr), 4) } else { (std::mem::offset_of!(libc::sockaddr_in6, sin6_addr), 16) };

        unsafe { std::slice::from_raw_parts(mask.cast::<u8>().add(start), size) }.iter().map(|byte| byte.count_ones()).sum()
    }
    /// Kind, hardware address and what else the link-layer entry of an interface says.

    unsafe fn link(entry: &libc::ifaddrs, name: &[u8], interface: &mut Interface) {
        if unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_PACKET { return; }
        let link = unsafe { entry.ifa_addr.cast::<libc::sockaddr_ll>().read_unaligned() };
        let sysfs = |leaf: &str| std::path::Path::new("/sys/class/net").join(String::from_utf8_lossy(name).as_ref()).join(leaf);
        interface.kind = match link.sll_hatype {
            libc::ARPHRD_LOOPBACK => network::INTERFACE_LOOPBACK,
            libc::ARPHRD_ETHER => if sysfs("wireless").exists() { network::INTERFACE_WIRELESS } else { network::INTERFACE_ETHERNET },
            libc::ARPHRD_PPP => INTERFACE_POINT_TO_POINT,
            // 823 is ARPHRD_IP6GRE of <linux/if_arp.h>, which `libc` does not carry.
            libc::ARPHRD_TUNNEL | libc::ARPHRD_TUNNEL6 | libc::ARPHRD_SIT | libc::ARPHRD_IPGRE | 823 => network::INTERFACE_TUNNEL,
            _ => interface.kind,
        };
        // Megabits per second; the file is missing or unreadable for a link without a speed, and -1 for one that does not know it.
        interface.speed_bps = std::fs::read_to_string(sysfs("speed")).ok().and_then(|text| text.trim().parse::<u64>().ok()).map_or(0, |megabits| megabits.saturating_mul(1_000_000));
        // An address longer than the boundary carries (InfiniBand has 20 bytes) is none rather than a part of one.
        let length = link.sll_halen as usize;
        if length <= interface.hardware_address.len() {
            interface.hardware_address_length = length as u32;
            interface.hardware_address[..length].copy_from_slice(&link.sll_addr[..length]);
        }
    }



    fn take() -> Result<Snapshot> {
        let mut list: *mut libc::ifaddrs = ptr::null_mut();
        if unsafe { libc::getifaddrs(&mut list) } != 0 { return Err(if io::Error::last_os_error().raw_os_error() == Some(libc::ENOMEM) { Error::OutOfMemory } else { Error::Os }); }
        // The MTU ioctl wants a socket of any kind; a machine without IPv4 has the other family.
        let probe = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).or_else(|_| socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::DGRAM, None)).ok();
        let probe = probe.as_ref().map_or(-1, std::os::fd::AsRawFd::as_raw_fd);
        let (mut interfaces, mut addresses, mut next) = (Vec::<Interface>::new(), Vec::new(), list.cast_const());
        while let Some(entry) = unsafe { next.as_ref() } {
            next = entry.ifa_next;
            if entry.ifa_name.is_null() { continue; }
            let label = unsafe { CStr::from_ptr(entry.ifa_name) }.to_bytes();
            let name = &label[..label.iter().position(|b| *b == b':').unwrap_or(label.len())];
            let mut text = [0u8; 64];
            if name.is_empty() || name.len() >= text.len() { continue; }
            text[..name.len()].copy_from_slice(name);
            let slot = match interfaces.iter().position(|i| i.name == text) {
                Some(slot) => slot,
                None => {
                    // An interface the OS no longer knows is left out, so the indices stay dense.
                    let index = unsafe { libc::if_nametoindex(text.as_ptr().cast()) };
                    if index == 0 { continue; }
                    let kind = if entry.ifa_flags & libc::IFF_POINTOPOINT as u32 != 0 { INTERFACE_POINT_TO_POINT } else { INTERFACE_UNKNOWN };
                    let flags = if entry.ifa_flags & libc::IFF_MULTICAST as u32 != 0 { MULTICAST } else { 0 };
                    interfaces.push(Interface { index, kind, state: state(entry.ifa_flags), flags, mtu: mtu(probe, name), name: text, ..Interface::EMPTY });
                    interfaces.len() - 1
                }
            };
            if entry.ifa_addr.is_null() { continue; }
            let index = interfaces[slot].index;
            match unsafe { (*entry.ifa_addr).sa_family } as i32 {
                libc::AF_INET => {
                    let v4 = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in>().read_unaligned() };
                    addresses.push(InterfaceAddress { interface_index: index, prefix_length: unsafe { prefix(entry.ifa_netmask, libc::AF_INET, 32) }, address: Address::v4(v4.sin_addr.s_addr.to_ne_bytes(), 0) });
                }
                libc::AF_INET6 => {
                    let v6 = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in6>().read_unaligned() }.sin6_addr.s6_addr;
                    // fe80::/10 is only an address together with its link.
                    let scope = if v6[0] == 0xfe && v6[1] & 0xc0 == 0x80 { index } else { 0 };
                    addresses.push(InterfaceAddress { interface_index: index, prefix_length: unsafe { prefix(entry.ifa_netmask, libc::AF_INET6, 128) }, address: Address::v6(v6, 0, scope) });
                }
                _ => unsafe { link(entry, name, &mut interfaces[slot]) },
            }
        }
        unsafe { libc::freeifaddrs(list) };
        Ok((interfaces, addresses))
    }
    fn entry<T: Copy>(index: usize, list: impl FnOnce(&Snapshot) -> &Vec<T>) -> Result<T> {
        let mut snapshot = SNAPSHOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if index == 0 || snapshot.is_none() { *snapshot = Some(take()?); }
        snapshot.as_ref().and_then(|lists| list(lists).get(index).copied()).ok_or(Error::NotFound)
    }
    /// Whether an IPv6 socket that carries both families can join an IPv4 group here: only where it has been measured.
    const DUAL: bool = cfg!(any(target_os = "linux", target_os = "android", target_vendor = "apple"));
    fn group_error(e: io::Error) -> Error {
        // EADDRINUSE is a group the socket is in already, EADDRNOTAVAIL one it is not in, ENODEV or ENXIO an interface that does not exist.
        match e.raw_os_error() {
            Some(libc::EADDRINUSE) => Error::AddressInUse, Some(libc::EADDRNOTAVAIL) => Error::AddressNotAvailable, Some(libc::ENODEV | libc::ENXIO) => Error::NotFound,
            Some(libc::EACCES | libc::EPERM) => Error::AccessDenied, Some(libc::ENOPROTOOPT) => Error::Unsupported, Some(libc::EINVAL) => Error::InvalidArgument,
            Some(libc::ENOMEM | libc::ENOBUFS) => Error::OutOfMemory,
            _ => Error::Os,
        }
    }

    impl port::Network for Std {
        fn interface_entry(index: usize) -> Result<Interface> { entry(index, |lists| &lists.0) }
        fn address_entry(index: usize) -> Result<InterfaceAddress> { entry(index, |lists| &lists.1) }
        unsafe fn reverse_lookup(address: &Address, out: *mut u8, capacity: usize) -> Result<usize> {
            let native = native(address)?;
            let mut host = [0u8; libc::NI_MAXHOST as usize];
            let code = loop {
                let code = unsafe { libc::getnameinfo(native.as_ptr().cast(), native.len(), host.as_mut_ptr().cast(), host.len() as libc::socklen_t, ptr::null_mut(), 0, libc::NI_NAMEREQD) };
                if code != libc::EAI_SYSTEM || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted { break code; }
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
            let socket = unsafe { borrow::<Socket>(socket) }?;
            // A stream socket has no groups, a local socket neither, and a group of a family the socket does not carry is not this socket's.
            let mapped = socket.v6 && group.family as u32 == IPV4;
            if socket.stream || socket.local || (socket.v6 != (group.family as u32 == IPV6) && !mapped) { return Err(Error::InvalidArgument); }
            if mapped && (!DUAL || socket.inner.only_v6().map_err(group_error)?) { return Err(Error::InvalidArgument); }
            if interface_index != 0 && !tuning::exists(interface_index) { return Err(Error::NotFound); }
            let (s, a) = (&socket.inner, group.address);
            match (group.family as u32, join) {


                (IPV4, true) => s.join_multicast_v4_n(&Ipv4Addr::new(a[0], a[1], a[2], a[3]), &InterfaceIndexOrAddress::Index(interface_index)),
                (IPV4, false) => s.leave_multicast_v4_n(&Ipv4Addr::new(a[0], a[1], a[2], a[3]), &InterfaceIndexOrAddress::Index(interface_index)),
                (_, true) => s.join_multicast_v6(&Ipv6Addr::from(a), interface_index),
                (_, false) => s.leave_multicast_v6(&Ipv6Addr::from(a), interface_index),
            }.map_err(group_error)
        }
    }
}
