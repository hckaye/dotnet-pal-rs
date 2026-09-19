//! Exercises the network group of the std port through the negotiated C table, on
//! whatever desktop OS runs the test: the lists against if_nameindex, the interface
//! ioctls and getifaddrs asked by the test itself, reverse lookup against
//! getnameinfo, and membership with real multicast datagrams.
#[cfg(not(unix))]
#[test]
fn network_is_absent_without_a_provider() {
    let api = unsafe { &*dotnet_pal_std::api() };
    assert_eq!(api.header.capabilities & dotnet_pal_rs::network::CAP, 0);
    assert!(api.network.interface_entry.is_none() && api.network.address_entry.is_none() && api.network.reverse_lookup.is_none() && api.network.membership.is_none());
}

#[cfg(unix)]
mod unix {
    use dotnet_pal_rs::io::{ADDRESS_IN_USE, ADDRESS_NOT_AVAILABLE};
    use dotnet_pal_rs::kernel::TIMEOUT;
    use dotnet_pal_rs::network::{self, Interface, InterfaceAddress, INTERFACE_ETHERNET, INTERFACE_LOOPBACK, INTERFACE_POINT_TO_POINT, INTERFACE_TUNNEL, INTERFACE_UNKNOWN, INTERFACE_WIRELESS,
        LINK_DOWN, LINK_UNKNOWN, LINK_UP, MULTICAST};
    use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
    use dotnet_pal_rs::sockets::{self, Address, PollEntry, DATAGRAM as UDP, IPV4 as V4, IPV6 as V6, NO_CHANNEL, POLL_READ, STREAM as TCP};
    use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
    use std::{ffi::{c_void, CStr}, mem::{size_of, zeroed}, ptr, sync::{Mutex, MutexGuard}, time::{Duration, Instant}};

    /// One test at a time: the counters and the snapshot are the process's.
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
    fn api() -> &'static dotnet_pal_rs::Api {
        let api = unsafe { &*dotnet_pal_std::api() };
        assert_eq!(api.header.capabilities & (network::CAP | sockets::CAP), network::CAP | sockets::CAP);
        api
    }
    fn ops() -> &'static network::Ops { &api().network }
    fn stats() -> network::Stats {
        let mut out = network::Stats::default();
        assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<network::Stats>()) }, OK);
        out
    }
    fn counts(s: &network::Stats) -> [u64; 5] { [s.interface_ok, s.address_ok, s.lookup_ok, s.membership_ok, s.rejected_or_failed] }
    fn interface_at(index: usize) -> (u32, Interface) {
        let mut out = Interface { index: 9, kind: 9, name: [b'x'; 64], ..Interface::EMPTY };
        (unsafe { ops().interface_entry.unwrap()(index, &mut out, size_of::<Interface>()) }, out)
    }
    fn address_at(index: usize) -> (u32, InterfaceAddress) {
        let mut out = InterfaceAddress { interface_index: 9, prefix_length: 9, address: Address { family: 9, port: 9, scope: 9, address: [9; 16] } };
        (unsafe { ops().address_entry.unwrap()(index, &mut out, size_of::<InterfaceAddress>()) }, out)
    }
    fn same(a: &Interface, b: &Interface) -> bool {
        (a.index, a.kind, a.state, a.flags, a.mtu, a.hardware_address_length, a.speed_bps, a.hardware_address, a.name) == (b.index, b.kind, b.state, b.flags, b.mtu, b.hardware_address_length, b.speed_bps, b.hardware_address, b.name)
    }
    fn key(a: &InterfaceAddress) -> (u32, u32, Address) { (a.interface_index, a.prefix_length, a.address) }
    fn name(interface: &Interface) -> &CStr { CStr::from_bytes_until_nul(&interface.name).expect("a terminated name") }
    type Lists = (Vec<Interface>, Vec<InterfaceAddress>);
    /// Both lists to their ends; past the last entry the answer is NOT_FOUND and the output is cleared.
    fn lists() -> Lists {
        let (mut interfaces, mut addresses) = (Vec::new(), Vec::new());
        loop {
            let (status, entry) = interface_at(interfaces.len());
            if status == NOT_FOUND { assert!(same(&entry, &Interface::EMPTY)); break; }
            assert_eq!(status, OK);
            interfaces.push(entry);
            assert!(interfaces.len() < 4096);
        }
        loop {
            let (status, entry) = address_at(addresses.len());
            if status == NOT_FOUND { assert_eq!(key(&entry), key(&InterfaceAddress::EMPTY)); break; }
            assert_eq!(status, OK);
            addresses.push(entry);
            assert!(addresses.len() < 4096);
        }
        (interfaces, addresses)
    }

    /// What the OS tells the test itself.
    mod os {
        use super::*;
        #[cfg(target_vendor = "apple")]
        pub const SIOCGIFFLAGS: libc::c_ulong = 0xc020_6911;
        #[cfg(target_vendor = "apple")]
        pub const SIOCGIFMTU: libc::c_ulong = 0xc020_6933;
        #[cfg(not(target_vendor = "apple"))]
        pub use libc::{SIOCGIFFLAGS, SIOCGIFMTU};
        pub struct Link { pub kind: u32, pub hardware: Vec<u8>, pub speed_bps: u64 }
        pub struct View { pub names: Vec<(String, u32)>, pub links: Vec<(String, Link)>, pub addresses: Vec<InterfaceAddress> }
        pub fn request(name: &CStr) -> libc::ifreq {
            let mut request: libc::ifreq = unsafe { zeroed() };
            for (to, from) in request.ifr_name.iter_mut().zip(name.to_bytes()) { *to = *from as _; }
            request
        }
        /// Flags and MTU of one interface by ioctl.
        pub fn flags_and_mtu(name: &CStr) -> (i32, u32) {
            let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let fd = std::os::fd::AsRawFd::as_raw_fd(&probe);
            let mut asked = request(name);
            assert_eq!(unsafe { libc::ioctl(fd, SIOCGIFFLAGS as _, ptr::addr_of_mut!(asked)) }, 0, "{name:?}");
            let flags = unsafe { asked.ifr_ifru.ifru_flags } as u16 as i32;
            let mut asked = request(name);
            assert_eq!(unsafe { libc::ioctl(fd, SIOCGIFMTU as _, ptr::addr_of_mut!(asked)) }, 0, "{name:?}");
            (flags, unsafe { asked.ifr_ifru.ifru_mtu } as u32)
        }
        fn ones(mask: *const libc::sockaddr, start: usize, size: usize) -> u32 {
            #[cfg(target_vendor = "apple")]
            let size = size.min((unsafe { (*mask).sa_len } as usize).saturating_sub(start));
            unsafe { std::slice::from_raw_parts(mask.cast::<u8>().add(start), size) }.iter().map(|b| b.count_ones()).sum()
        }
        #[cfg(not(target_vendor = "apple"))]
        fn link(entry: &libc::ifaddrs, name: &str, flags: u32) -> Option<Link> {
            if unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_PACKET { return None; }
            let link = unsafe { entry.ifa_addr.cast::<libc::sockaddr_ll>().read_unaligned() };
            let kind = match link.sll_hatype {
                772 => INTERFACE_LOOPBACK,
                1 => if std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists() { INTERFACE_WIRELESS } else { INTERFACE_ETHERNET },
                512 => INTERFACE_POINT_TO_POINT,
                768 | 769 | 776 | 778 | 823 => INTERFACE_TUNNEL,
                _ if flags & libc::IFF_POINTOPOINT as u32 != 0 => INTERFACE_POINT_TO_POINT,
                _ => INTERFACE_UNKNOWN,
            };
            let megabits = std::fs::read_to_string(format!("/sys/class/net/{name}/speed")).ok().and_then(|t| t.trim().parse::<i64>().ok()).filter(|m| *m > 0).unwrap_or(0);
            let hardware = if link.sll_halen <= 8 { link.sll_addr[..link.sll_halen as usize].to_vec() } else { Vec::new() };
            Some(Link { kind, hardware, speed_bps: megabits as u64 * 1_000_000 })
        }
        #[cfg(target_vendor = "apple")]
        fn link(entry: &libc::ifaddrs, _: &str, flags: u32) -> Option<Link> {
            if unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_LINK { return None; }
            let link = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_dl>() };
            let kind = match link.sdl_type {
                0x18 => INTERFACE_LOOPBACK, 0x06 => INTERFACE_ETHERNET, 0x47 => INTERFACE_WIRELESS, 0x17 => INTERFACE_POINT_TO_POINT, 0x37 | 0x39 => INTERFACE_TUNNEL,
                _ if flags & libc::IFF_POINTOPOINT as u32 != 0 => INTERFACE_POINT_TO_POINT,
                _ => INTERFACE_UNKNOWN,
            };
            let bytes = unsafe { std::slice::from_raw_parts(entry.ifa_addr.cast::<u8>(), link.sdl_len as usize) };
            let start = 8 + link.sdl_nlen as usize;
            let hardware = if link.sdl_alen <= 8 { bytes[start..start + link.sdl_alen as usize].to_vec() } else { Vec::new() };
            // ifi_baudrate sits behind eight bytes and two 32-bit fields of `struct if_data`.
            let baud = if entry.ifa_data.is_null() { 0 } else { unsafe { entry.ifa_data.cast::<u8>().add(16).cast::<u32>().read_unaligned() } };
            Some(Link { kind, hardware, speed_bps: if baud == u32::MAX { 0 } else { baud as u64 } })
        }
        pub fn view() -> View {
            let mut view = View { names: Vec::new(), links: Vec::new(), addresses: Vec::new() };
            let names = unsafe { libc::if_nameindex() };
            assert!(!names.is_null());
            let mut next = names;
            while unsafe { (*next).if_index } != 0 {
                view.names.push((unsafe { CStr::from_ptr((*next).if_name) }.to_str().unwrap().to_owned(), unsafe { (*next).if_index }));
                next = unsafe { next.add(1) };
            }
            unsafe { libc::if_freenameindex(names) };
            let mut list = ptr::null_mut();
            assert_eq!(unsafe { libc::getifaddrs(&mut list) }, 0);
            let mut next = list.cast_const();
            while let Some(entry) = unsafe { next.as_ref() } {
                next = entry.ifa_next;
                if entry.ifa_addr.is_null() { continue; }
                let label = unsafe { CStr::from_ptr(entry.ifa_name) }.to_str().unwrap();
                let name = label.split(':').next().unwrap().to_owned();
                let index = view.names.iter().find(|(known, _)| *known == name).map_or(0, |(_, index)| *index);
                match unsafe { (*entry.ifa_addr).sa_family } as i32 {
                    libc::AF_INET => {
                        let v4 = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in>().read_unaligned() };
                        view.addresses.push(InterfaceAddress { interface_index: index, prefix_length: ones(entry.ifa_netmask, 4, 4), address: Address::v4(v4.sin_addr.s_addr.to_ne_bytes(), 0) });
                    }
                    libc::AF_INET6 => {
                        let v6 = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in6>().read_unaligned() }.sin6_addr.s6_addr;
                        let scope = if v6[0] == 0xfe && v6[1] & 0xc0 == 0x80 { index } else { 0 };
                        view.addresses.push(InterfaceAddress { interface_index: index, prefix_length: ones(entry.ifa_netmask, 8, 16), address: Address::v6(v6, 0, scope) });
                    }
                    _ => if let Some(link) = link(entry, &name, entry.ifa_flags) { view.links.push((name, link)); },
                }
            }
            unsafe { libc::freeifaddrs(list) };
            view
        }
        /// getnameinfo's own answer: the name, or the status the group owes for the failure.
        pub fn reverse(address: &Address) -> std::result::Result<Vec<u8>, Option<u32>> {
            let mut host = [0u8; 1025];
            let (mut v4, mut v6): (libc::sockaddr_in, libc::sockaddr_in6) = unsafe { (zeroed(), zeroed()) };
            let a = address.address;
            #[cfg(target_vendor = "apple")]
            { v4.sin_len = size_of::<libc::sockaddr_in>() as u8; v6.sin6_len = size_of::<libc::sockaddr_in6>() as u8; }
            (v4.sin_family, v4.sin_addr.s_addr) = (libc::AF_INET as _, u32::from_ne_bytes([a[0], a[1], a[2], a[3]]));
            (v6.sin6_family, v6.sin6_addr.s6_addr, v6.sin6_scope_id) = (libc::AF_INET6 as _, a, address.scope);
            let (raw, length) = if address.family as u32 == V4 { (ptr::addr_of!(v4).cast::<libc::sockaddr>(), size_of::<libc::sockaddr_in>()) } else { (ptr::addr_of!(v6).cast(), size_of::<libc::sockaddr_in6>()) };
            match unsafe { libc::getnameinfo(raw, length as libc::socklen_t, host.as_mut_ptr().cast(), host.len() as libc::socklen_t, ptr::null_mut(), 0, libc::NI_NAMEREQD) } {
                0 => Ok(CStr::from_bytes_until_nul(&host).unwrap().to_bytes().to_vec()),
                libc::EAI_NONAME => Err(Some(NOT_FOUND)),
                libc::EAI_AGAIN => Err(Some(TIMEOUT)),
                _ => Err(None),
            }
        }
    }
    /// The group's lists and the test's own view of the same moment: taken again while the lists differ before and after the
    /// view (an interface of a desktop comes and goes, and its flags change, while a test runs).
    fn settled() -> (Lists, os::View, Vec<(i32, u32)>) {
        for _ in 0..20 {
            let before = lists();
            let view = os::view();
            let asked: Vec<(i32, u32)> = before.0.iter().map(|i| os::flags_and_mtu(name(i))).collect();
            let after = lists();
            let unchanged = before.0.len() == after.0.len() && before.0.iter().zip(&after.0).all(|(a, b)| same(a, b)) && before.1.iter().map(key).eq(after.1.iter().map(key));
            if unchanged { return (before, view, asked); }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the interface list did not hold still for one comparison in twenty");
    }

    #[test]
    fn interfaces_and_addresses_are_the_ones_the_os_lists() {
        let _serial = serial();
        let ((interfaces, addresses), view, asked) = settled();
        // The same names with the same indexes as if_nameindex, each once.
        let mut listed: Vec<(String, u32)> = interfaces.iter().map(|i| (name(i).to_str().unwrap().to_owned(), i.index)).collect();
        let mut known = view.names.clone();
        listed.sort();
        known.sort();
        assert_eq!(listed, known);
        for (interface, (flags, mtu)) in interfaces.iter().zip(&asked) {
            let text = name(interface).to_str().unwrap();
            assert!(interface.index != 0 && interface.name[text.len()..].iter().all(|b| *b == 0), "{text}");
            let (up, running) = (flags & libc::IFF_UP != 0, flags & libc::IFF_RUNNING != 0);
            assert_eq!(interface.state, if up && running { LINK_UP } else if !up { LINK_DOWN } else { LINK_UNKNOWN }, "{text}");
            assert_eq!((interface.flags, interface.mtu), (if flags & libc::IFF_MULTICAST != 0 { MULTICAST } else { 0 }, *mtu), "{text}");
            let (_, link) = view.links.iter().find(|(known, _)| known == text).unwrap_or_else(|| panic!("{text} has no link-layer entry"));
            assert_eq!((interface.kind, interface.speed_bps, &interface.hardware_address[..interface.hardware_address_length as usize]), (link.kind, link.speed_bps, &link.hardware[..]), "{text}");
            assert!(interface.hardware_address[interface.hardware_address_length as usize..].iter().all(|b| *b == 0), "{text}");
        }
        let loopback = interfaces.iter().find(|i| i.kind == INTERFACE_LOOPBACK).expect("a loopback interface");
        assert_eq!((loopback.state, loopback.hardware_address_length <= 6), (LINK_UP, true));
        // The address list as a set: every IPv4 and IPv6 entry of getifaddrs with its prefix length, interface and scope, none of them with a port.
        let (mut listed, mut known): (Vec<_>, Vec<_>) = (addresses.iter().map(key).map(|(i, p, a)| (i, p, a.family, a.port, a.scope, a.address)).collect(), view.addresses.iter().map(key).map(|(i, p, a)| (i, p, a.family, a.port, a.scope, a.address)).collect());
        listed.sort();
        known.sort();
        assert_eq!(listed, known);
        assert!(addresses.iter().all(|a| interfaces.iter().filter(|i| i.index == a.interface_index).count() == 1));
        assert!(addresses.iter().any(|a| key(a) == (loopback.index, 8, Address::v4([127, 0, 0, 1], 0))), "loopback has 127.0.0.1/8");
        let six = Address::v6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], 0, 0);
        assert_eq!(addresses.iter().any(|a| key(a) == (loopback.index, 128, six)), view.addresses.iter().any(|a| a.address == six), "::1/128 where the host has IPv6");
        // A link-local address carries its interface as the scope.
        for a in &addresses { assert_eq!(a.address.scope, if a.address.family as u32 == V6 && a.address.address[0] == 0xfe && a.address.address[1] & 0xc0 == 0x80 { a.interface_index } else { 0 }); }
        println!("NETWORK note: {} interfaces, {} addresses, loopback {:?}", interfaces.len(), addresses.len(), name(loopback));
    }

    #[test]
    fn enumerations_end_and_hold_under_threads() {
        let _serial = serial();
        let before = lists();
        let start = counts(&stats());
        assert_eq!((interface_at(before.0.len() + 1).0, interface_at(usize::MAX).0, address_at(before.1.len() + 1).0, address_at(usize::MAX).0), (NOT_FOUND, NOT_FOUND, NOT_FOUND, NOT_FOUND));
        assert_eq!(counts(&stats()), { let mut c = start; c[4] += 4; c });
        // Every thread restarts both enumerations at index 0, which takes a new snapshot under the others.
        let seen: Vec<Vec<Lists>> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4).map(|_| scope.spawn(|| (0..50).map(|_| lists()).collect::<Vec<_>>())).collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        let after = lists();
        let unchanged = |a: &Lists, b: &Lists| a.0.len() == b.0.len() && a.0.iter().zip(&b.0).all(|(x, y)| same(x, y)) && a.1.iter().map(key).eq(b.1.iter().map(key));
        for pass in seen.iter().flatten() {
            assert!(pass.0.iter().all(|i| i.index != 0 && i.name[0] != 0) && pass.1.iter().all(|a| a.interface_index != 0));
            if unchanged(&before, &after) { assert!(unchanged(pass, &before), "a pass differs from the list before and after it"); }
        }
        if !unchanged(&before, &after) { println!("NETWORK note: the interface list changed during the thread passes, their equality is unchecked"); }
    }

    fn lookup(address: &Address, capacity: usize) -> (u32, usize, Vec<u8>) {
        let (mut needed, mut buffer) = (usize::MAX, vec![0xAAu8; capacity]);
        let status = unsafe { ops().reverse_lookup.unwrap()(address, if capacity == 0 { ptr::null_mut() } else { buffer.as_mut_ptr() }, capacity, &mut needed) };
        (status, needed, buffer)
    }
    fn padded(text: &[u8], capacity: usize) -> Vec<u8> { let mut all = text.to_vec(); all.resize(capacity, 0); all }
    /// The group answers as getnameinfo does: the name under the text contract, or the status its failure maps to.
    fn reverse(address: &Address) -> &'static str {
        match os::reverse(address) {
            Ok(host) => {
                let needed = host.len() + 1;
                assert_eq!(lookup(address, 512), (OK, needed, padded(&host, 512)));
                assert_eq!(lookup(address, needed), (OK, needed, padded(&host, needed)));
                assert_eq!(lookup(address, needed - 1), (BUFFER_TOO_SMALL, needed, vec![0; needed - 1]));
                assert_eq!(lookup(address, 0), (BUFFER_TOO_SMALL, needed, Vec::new()));
                "a name"
            }
            Err(Some(status)) => { assert_eq!(lookup(address, 512), (status, 0, vec![0; 512])); if status == NOT_FOUND { "NOT_FOUND" } else { "TIMEOUT" } }
            Err(None) => "unchecked",
        }
    }
    #[test]
    fn reverse_lookup_answers_as_getnameinfo_does() {
        let _serial = serial();
        let start = counts(&stats());
        let loopback = reverse(&Address::v4([127, 0, 0, 1], 0));
        // A port and the bytes an IPv4 address does not use are not part of the question.
        let noisy = Address { family: V4 as u16, port: 4242, scope: 7, address: [127, 0, 0, 1, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9] };
        assert_eq!(lookup(&noisy, 512), lookup(&Address::v4([127, 0, 0, 1], 0), 512));
        // 192.0.2.1 is TEST-NET-1: nobody's address, so nobody's name.
        let unnamed = reverse(&Address::v4([192, 0, 2, 1], 0));
        let six = reverse(&Address::v6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], 0, 0));
        println!("NETWORK note: 127.0.0.1 has {loopback}, ::1 has {six}, 192.0.2.1 has {unnamed}");
        let before = counts(&stats());
        assert!(before[2] > start[2] || loopback != "a name");
        let r = ops().reverse_lookup.unwrap();
        let (good, bad, mut needed, mut buffer) = (Address::v4([127, 0, 0, 1], 0), Address { family: 9, ..Address::v4([127, 0, 0, 1], 0) }, 7usize, [0u8; 8]);
        assert_eq!(unsafe { r(&good, buffer.as_mut_ptr(), 8, ptr::null_mut()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { r(&good, ptr::null_mut(), 1, &mut needed) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { r(&good, buffer.as_mut_ptr(), usize::MAX, &mut needed) }, INVALID_ARGUMENT);
        assert_eq!((unsafe { r(ptr::null(), buffer.as_mut_ptr(), 8, &mut needed) }, needed), (INVALID_ARGUMENT, 0));
        assert_eq!((unsafe { r(&bad, buffer.as_mut_ptr(), 8, &mut needed) }, needed), (INVALID_ARGUMENT, 0));
        assert_eq!(counts(&stats()), { let mut c = before; c[4] += 5; c });
    }

    fn socket_ops() -> &'static sockets::Ops { &api().sockets }
    fn open(family: u32, kind: u32) -> *mut c_void {
        let mut socket = ptr::null_mut();
        assert_eq!(unsafe { socket_ops().create.unwrap()(family, kind, &mut socket) }, OK);
        socket
    }
    fn close(socket: *mut c_void) { assert_eq!(unsafe { socket_ops().close.unwrap()(socket) }, OK); }
    fn bind_any(socket: *mut c_void, family: u32) -> u16 {
        let (any, mut at) = (Address { family: family as u16, ..Address::default() }, Address::default());
        assert_eq!((unsafe { socket_ops().bind.unwrap()(socket, &any) }, unsafe { socket_ops().local_address.unwrap()(socket, &mut at) }), (OK, OK));
        at.port
    }
    fn set(socket: *mut c_void, name: u32, value: u64) { assert_eq!(unsafe { socket_ops().set_option.unwrap()(socket, name, value) }, OK, "option {name}"); }
    fn member(socket: *mut c_void, group: &Address, interface: u32, join: u32) -> u32 { unsafe { ops().membership.unwrap()(socket, group, interface, join) } }
    /// Whether a datagram with this text reaches the socket in time; anything else that arrives is read and dropped (a copy of
    /// an earlier datagram must not pass for the one sent after it).
    fn arrives(socket: *mut c_void, wanted: &[u8], within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { return false; }
            let (mut entry, mut ready) = ([PollEntry { socket, requested: POLL_READ, triggered: 0 }], 0usize);
            assert_eq!(unsafe { socket_ops().poll.unwrap()(entry.as_mut_ptr(), 1, left.as_nanos() as u64, NO_CHANNEL, &mut ready) }, OK);
            if ready == 0 { continue; }
            let (mut data, mut done) = ([0u8; 16], 0usize);
            assert_eq!(unsafe { socket_ops().receive.unwrap()(socket, data.as_mut_ptr(), data.len(), 0, ptr::null_mut(), &mut done) }, OK);
            if &data[..done] == wanted { return true; }
        }
    }
    /// An interface of the group's own list that is up, carries multicast, is no loopback and has an address of `family`.
    fn multicast_interface(lists: &Lists, family: u32) -> Option<u32> {
        lists.0.iter().find(|i| i.flags & MULTICAST != 0 && i.state == LINK_UP && i.kind != INTERFACE_LOOPBACK
            && lists.1.iter().any(|a| a.interface_index == i.index && a.address.family as u32 == family && (family == V4 || a.address.scope != 0))).map(|i| i.index)
    }
    /// A datagram to `group` reaches a member through `interface`, and no longer once the socket has left; then the statuses of a socket that is, and is not, in a group.
    fn traffic_and_statuses(family: u32, mut group: Address, other_family: Address, interface: u32) {
        let (receiver, sender, stream) = (open(family, UDP), open(family, UDP), open(family, TCP));
        group.port = bind_any(receiver, family);
        if family == V6 { group.scope = interface; }
        set(sender, sockets::MULTICAST_LOOPBACK, 1);
        set(sender, sockets::MULTICAST_INTERFACE, interface as u64);
        let send = |text: &[u8]| { let mut done = 0usize; assert_eq!((unsafe { socket_ops().send.unwrap()(sender, text.as_ptr(), text.len(), &group, &mut done) }, done), (OK, text.len())); };
        assert_eq!(member(receiver, &group, interface, 0), ADDRESS_NOT_AVAILABLE, "a group never joined");
        assert_eq!(member(receiver, &group, interface, 1), OK);
        send(b"first");
        assert!(arrives(receiver, b"first", Duration::from_secs(5)), "a member receives the group's datagram");
        assert_eq!((member(receiver, &group, interface, 1), member(receiver, &group, interface, 0)), (ADDRESS_IN_USE, OK));
        send(b"second");
        assert!(!arrives(receiver, b"second", Duration::from_millis(300)), "a socket that left receives nothing more");
        assert_eq!(member(receiver, &group, interface, 0), ADDRESS_NOT_AVAILABLE);
        assert_eq!((member(receiver, &group, 999_999, 1), member(receiver, &group, u32::MAX, 1)), (NOT_FOUND, NOT_FOUND));
        // A stream socket has no groups, and a group of the other family is not this socket's.
        assert_eq!((member(stream, &group, interface, 1), member(receiver, &other_family, interface, 1)), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        for socket in [receiver, sender, stream] { close(socket); }
    }
    #[test]
    fn membership_joins_and_leaves_with_real_datagrams() {
        let _serial = serial();
        let lists = lists();
        let (group, group6) = (Address::v4([239, 255, 77, 80], 0), Address::v6([0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x77, 0x80], 0, 0));
        let start = counts(&stats());
        match multicast_interface(&lists, V4) {
            Some(interface) => {
                traffic_and_statuses(V4, group, group6, interface);
                // Interface 0 is the OS's choice, where it has one (a route for the group).
                let socket = open(V4, UDP);
                let chosen = member(socket, &group, 0, 1);
                assert!([OK, NOT_FOUND, ADDRESS_NOT_AVAILABLE].contains(&chosen), "status {chosen}");
                if chosen == OK { assert_eq!(member(socket, &group, 0, 0), OK); }
                close(socket);
                println!("NETWORK note: IPv4 group traffic checked on interface {interface}, interface 0 answered {chosen}");
            }
            None => println!("NETWORK note: no interface that is up, carries multicast and has an IPv4 address; IPv4 membership is unchecked"),
        }
        match multicast_interface(&lists, V6) {
            Some(interface) => { traffic_and_statuses(V6, group6, group, interface); println!("NETWORK note: IPv6 group traffic checked on interface {interface}"); }
            None => println!("NETWORK note: no multicast interface with a link-local IPv6 address; IPv6 membership is unchecked"),
        }
        // Refused before the provider: no socket, no group, an address that is no group, a join that is neither 0 nor 1.
        let socket = open(V4, UDP);
        let before = counts(&stats());
        assert!(before[3] >= start[3] && before[0] == start[0] && before[1] == start[1]);
        let m = ops().membership.unwrap();
        for (group, join) in [(Address::v4([127, 0, 0, 1], 0), 1), (Address::v6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], 0, 0), 1), (Address { family: 0, ..group }, 1), (group, 2)] {
            assert_eq!(member(socket, &group, 0, join), INVALID_ARGUMENT, "{group:?} join {join}");
        }
        assert_eq!((member(ptr::null_mut(), &group, 0, 1), unsafe { m(socket, ptr::null(), 0, 1) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        let e = ops().interface_entry.unwrap();
        let a = ops().address_entry.unwrap();
        let (mut interface, mut address) = (Interface::EMPTY, InterfaceAddress::EMPTY);
        assert_eq!((unsafe { e(0, ptr::null_mut(), size_of::<Interface>()) }, unsafe { e(0, &mut interface, size_of::<Interface>() - 1) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((unsafe { a(0, ptr::null_mut(), size_of::<InterfaceAddress>()) }, unsafe { a(0, &mut address, size_of::<InterfaceAddress>() - 1) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!(counts(&stats()), { let mut c = before; c[4] += 10; c });
        assert_eq!(unsafe { ops().read_stats.unwrap()(ptr::null_mut(), size_of::<network::Stats>()) }, INVALID_ARGUMENT);
        close(socket);
    }
}
