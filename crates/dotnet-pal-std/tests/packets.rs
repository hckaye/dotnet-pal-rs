//! Exercises the packets group of the std port through the negotiated C table, on
//! whatever desktop OS runs the test: datagrams sent inside the process to a socket
//! bound to every address, whose description is compared with what the OS attaches to
//! the same traffic on a socket of the test's own; raw ICMP sockets of the sockets
//! group, which echo when the process may open them and are ACCESS_DENIED otherwise;
//! the fragmentation switch, read back from the OS and shown by its effect.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
#[test]
fn packets_are_absent_without_a_provider() {
    use dotnet_pal_rs::sockets::{DATAGRAM, DONT_FRAGMENT, IPV4, PACKET_INFORMATION, RAW};
    let api = unsafe { &*dotnet_pal_std::api() };
    assert_eq!(api.header.capabilities & dotnet_pal_rs::packets::CAP, 0);
    assert!(api.packets.receive.is_none() && api.packets.read_stats.is_some());
    // Nothing could describe a datagram, so the option that asks for it is not there; neither are raw sockets and the fragmentation switch.
    let (mut raw, mut socket, mut value) = (std::ptr::dangling_mut::<u64>().cast(), std::ptr::null_mut(), 7u64);
    assert_eq!(unsafe { api.sockets.create.unwrap()(IPV4, RAW, &mut raw) }, dotnet_pal_rs::UNSUPPORTED);
    assert!(raw.is_null());
    assert_eq!(unsafe { api.sockets.create.unwrap()(IPV4, DATAGRAM, &mut socket) }, dotnet_pal_rs::OK);
    for option in [PACKET_INFORMATION, DONT_FRAGMENT] {
        assert_eq!((unsafe { api.sockets.set_option.unwrap()(socket, option, 1) }, unsafe { api.sockets.get_option.unwrap()(socket, option, &mut value) }, value), (dotnet_pal_rs::UNSUPPORTED, dotnet_pal_rs::UNSUPPORTED, 0));
    }
    assert_eq!(unsafe { api.sockets.close.unwrap()(socket) }, dotnet_pal_rs::OK);
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod unix {
    use dotnet_pal_rs::io::{ACCESS_DENIED, CONNECTION_REFUSED, WOULD_BLOCK};
    #[cfg(target_vendor = "apple")]
    use dotnet_pal_rs::io::MESSAGE_TOO_LARGE;
    use dotnet_pal_rs::kernel::TIMEOUT;
    use dotnet_pal_rs::packets::{self, Info, Stats};
    use dotnet_pal_rs::sockets::{self, Address, PollEntry, DATAGRAM as UDP, DONT_FRAGMENT, HOPS, IPV4 as V4, IPV6 as V6, LOCAL, NO_CHANNEL, PACKET_INFORMATION, POLL_READ as READ, RAW, RECEIVE_PEEK as PEEK, STREAM as TCP};
    use dotnet_pal_rs::{INVALID_ARGUMENT, OK, UNSUPPORTED};
    use std::{ffi::c_void, mem::size_of, ptr, sync::{Mutex, MutexGuard}, time::Instant};

    const MS: u64 = 1_000_000;
    const NOWHERE: Described = (0, 0, Address { family: 0, port: 0, scope: 0, address: [0; 16] });
    /// `(interface, reserved, destination)`: an `Info` that can be compared and printed.
    type Described = (u32, u32, Address);
    /// One test at a time: the counters are the process's, a descriptor is found by what only one socket has, and a raw socket hears every echo.
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
    fn api() -> &'static dotnet_pal_rs::Api {
        let api = unsafe { &*dotnet_pal_std::api() };
        assert_eq!(api.header.capabilities & (packets::CAP | sockets::CAP), packets::CAP | sockets::CAP);
        api
    }
    fn p() -> &'static packets::Ops { &api().packets }
    fn s() -> &'static sockets::Ops { &api().sockets }
    fn counts() -> [u64; 2] {
        let mut out = Stats::default();
        assert_eq!(unsafe { p().read_stats.unwrap()(&mut out, size_of::<Stats>()) }, OK);
        [out.receive_ok, out.rejected_or_failed]
    }
    fn loopback(family: u32, port: u16) -> Address { if family == V4 { Address::v4([127, 0, 0, 1], port) } else { Address::v6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], port, 0) } }
    /// The IPv4 address as an IPv6 socket that also takes IPv4 traffic names it.
    fn mapped(a: Address) -> Address { Address::v6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a.address[0], a.address[1], a.address[2], a.address[3]], a.port, 0) }
    fn open(family: u32, kind: u32) -> *mut c_void {
        let mut socket = ptr::null_mut();
        assert_eq!(unsafe { s().create.unwrap()(family, kind, &mut socket) }, OK);
        assert_eq!(set(socket, sockets::RECEIVE_TIMEOUT, 5000), OK);
        socket
    }
    fn close(socket: *mut c_void) { assert_eq!(unsafe { s().close.unwrap()(socket) }, OK); }
    fn set(socket: *mut c_void, name: u32, value: u64) -> u32 { unsafe { s().set_option.unwrap()(socket, name, value) } }
    fn get(socket: *mut c_void, name: u32) -> (u32, u64) { let mut value = 7u64; (unsafe { s().get_option.unwrap()(socket, name, &mut value) }, value) }
    fn unsupported(socket: *mut c_void, name: u32) { assert_eq!((get(socket, name), set(socket, name, 1)), ((UNSUPPORTED, 0), UNSUPPORTED), "option {name}"); }
    fn bind(socket: *mut c_void, at: &Address) -> u32 { unsafe { s().bind.unwrap()(socket, at) } }
    fn local(socket: *mut c_void) -> Address { let mut at = Address::default(); assert_eq!(unsafe { s().local_address.unwrap()(socket, &mut at) }, OK); at }
    fn send(socket: *mut c_void, data: &[u8], to: Option<&Address>) -> (u32, usize) {
        let mut done = 7usize;
        (unsafe { s().send.unwrap()(socket, data.as_ptr(), data.len(), to.map_or(ptr::null(), |to| to), &mut done) }, done)
    }
    fn sent(socket: *mut c_void, data: &[u8], to: Option<&Address>) { assert_eq!(send(socket, data, to), (OK, data.len())); }
    fn readable(socket: *mut c_void) -> bool {
        let (mut entry, mut ready) = (PollEntry { socket, requested: READ, triggered: 99 }, 7usize);
        assert_eq!(unsafe { s().poll.unwrap()(&mut entry, 1, 5000 * MS, NO_CHANNEL, &mut ready) }, OK);
        entry.triggered != 0
    }
    /// Counts what the receives below have to add to the group's counters.
    #[derive(Default)]
    struct Tally([u64; 2]);
    /// One receive of the group and everything it wrote; what it did not write stays 0xAA.
    #[derive(Debug, PartialEq)]
    struct Arrival { status: u32, data: Vec<u8>, from: Address, info: Described }
    impl Tally {
        fn note(&mut self, status: u32) -> u32 { self.0[(status != OK) as usize] += 1; status }
        fn take(&mut self, socket: *mut c_void, capacity: usize, flags: u32) -> Arrival {
            let (mut data, mut done, mut from) = (vec![0xAAu8; capacity], 7usize, Address { family: 9, ..Address::default() });
            let mut info = Info { interface_index: 99, reserved: 99, destination: from };
            let status = self.note(unsafe { p().receive.unwrap()(socket, data.as_mut_ptr(), capacity, flags, &mut from, &mut info, size_of::<Info>(), &mut done) });
            assert!(done <= capacity && (status == OK || done == 0), "{done} bytes with status {status}");
            if status == OK { data.truncate(done); }
            Arrival { status, data, from, info: (info.interface_index, info.reserved, info.destination) }
        }
        fn settled(self, before: [u64; 2]) { assert_eq!(counts(), [before[0] + self.0[0], before[1] + self.0[1]], "counters from {before:?}"); }
    }
    fn arrival(data: &[u8], from: Address, info: Described) -> Arrival { Arrival { status: OK, data: data.to_vec(), from, info } }
    /// A failure leaves no byte count, no sender and no description behind, and the buffer alone.
    fn failure(status: u32, capacity: usize) -> Arrival { Arrival { status, data: vec![0xAA; capacity], from: Address::default(), info: NOWHERE } }

    /// What the OS itself holds and delivers, asked by the test.
    mod kernel {
        use super::{Address, Described, NOWHERE, V4, V6};
        use std::{mem::{size_of, size_of_val, zeroed}, ptr};
        pub fn option(fd: i32, level: i32, name: i32) -> Option<i32> {
            let (mut value, mut length) = (-7i32, size_of::<i32>() as libc::socklen_t);
            (unsafe { libc::getsockopt(fd, level, name, ptr::addr_of_mut!(value).cast(), &mut length) } == 0).then_some(value)
        }
        /// The descriptor behind a handle, found by what only that socket has: its family, its type and the port it is bound to.
        pub fn descriptor(v6: bool, kind: i32, port: u16) -> i32 {
            let family = if v6 { libc::AF_INET6 } else { libc::AF_INET };
            (0..4096).find(|&fd| {
                let (mut storage, mut length) = (unsafe { zeroed::<libc::sockaddr_storage>() }, size_of::<libc::sockaddr_storage>() as libc::socklen_t);
                if unsafe { libc::getsockname(fd, ptr::addr_of_mut!(storage).cast(), &mut length) } != 0 || storage.ss_family as i32 != family { return false; }
                // sin_port and sin6_port are the same two bytes.
                u16::from_be(unsafe { ptr::addr_of!(storage).cast::<libc::sockaddr_in>().read() }.sin_port) == port && option(fd, libc::SOL_SOCKET, libc::SO_TYPE) == Some(kind)
            }).expect("no descriptor is bound to the socket's port")
        }
        pub fn index(name: &std::ffi::CStr) -> u32 { unsafe { libc::if_nametoindex(name.as_ptr()) } }
        pub fn loopback_index() -> u32 { [c"lo", c"lo0"].iter().map(|name| index(name)).find(|index| *index != 0).expect("no loopback interface") }
        /// The first interface besides the loopback that is up and has an IPv4 address, and that address.
        pub fn other_interface() -> Option<(u32, Address)> {
            let mut list = ptr::null_mut();
            assert_eq!(unsafe { libc::getifaddrs(&mut list) }, 0);
            let (mut next, mut found) = (list, None);
            while let Some(entry) = unsafe { next.as_ref() } {
                next = entry.ifa_next;
                if found.is_some() || entry.ifa_addr.is_null() || entry.ifa_flags & libc::IFF_UP as u32 == 0 || entry.ifa_flags & libc::IFF_LOOPBACK as u32 != 0 { continue; }
                if unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_INET { continue; }
                let address = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in>().read_unaligned() }.sin_addr.s_addr;
                found = Some((unsafe { libc::if_nametoindex(entry.ifa_name) }, Address::v4(address.to_ne_bytes(), 0)));
            }
            unsafe { libc::freeifaddrs(list) };
            found.filter(|(index, _)| *index != 0)
        }
        /// A socket of the test's own, bound to every address of its family as the group's is and asked to describe what it receives; and its port.
        pub fn twin(v6: bool, dual: bool) -> (i32, u16) {
            let fd = unsafe { libc::socket(if v6 { libc::AF_INET6 } else { libc::AF_INET }, libc::SOCK_DGRAM, 0) };
            assert!(fd >= 0);
            let (on, only) = (1i32, !dual as i32);
            let put = |level, name, value: &i32| assert_eq!(unsafe { libc::setsockopt(fd, level, name, (value as *const i32).cast(), size_of::<i32>() as libc::socklen_t) }, 0);
            let (mut storage, mut length) = (unsafe { zeroed::<libc::sockaddr_storage>() }, size_of::<libc::sockaddr_storage>() as libc::socklen_t);
            storage.ss_family = if v6 { libc::AF_INET6 } else { libc::AF_INET } as libc::sa_family_t;
            #[cfg(target_vendor = "apple")]
            { storage.ss_len = if v6 { size_of::<libc::sockaddr_in6>() } else { size_of::<libc::sockaddr_in>() } as u8; }
            if v6 { put(libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, &only); }
            assert_eq!(unsafe { libc::bind(fd, ptr::addr_of!(storage).cast(), if v6 { size_of::<libc::sockaddr_in6>() } else { size_of::<libc::sockaddr_in>() } as libc::socklen_t) }, 0);
            assert_eq!(unsafe { libc::getsockname(fd, ptr::addr_of_mut!(storage).cast(), &mut length) }, 0);
            if v6 { put(libc::IPPROTO_IPV6, libc::IPV6_RECVPKTINFO, &on); } else { put(libc::IPPROTO_IP, libc::IP_PKTINFO, &on); }
            (fd, u16::from_be(unsafe { ptr::addr_of!(storage).cast::<libc::sockaddr_in>().read() }.sin_port))
        }
        /// What the OS attached to the next datagram of `fd`, in the boundary's terms.
        pub fn attached(fd: i32) -> Described {
            let mut wait = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            assert_eq!(unsafe { libc::poll(&mut wait, 1, 5000) }, 1);
            let (mut data, mut control) = ([0u8; 64], [0u64; 32]);
            let mut part = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
            let mut message: libc::msghdr = unsafe { zeroed() };
            (message.msg_iov, message.msg_iovlen, message.msg_control, message.msg_controllen) = (&mut part, 1, control.as_mut_ptr().cast(), size_of_val(&control) as _);
            assert!(unsafe { libc::recvmsg(fd, &mut message, 0) } >= 0);
            let (mut described, mut header) = (NOWHERE, unsafe { libc::CMSG_FIRSTHDR(&message) });
            while !header.is_null() {
                let (level, kind, record) = unsafe { ((*header).cmsg_level, (*header).cmsg_type, libc::CMSG_DATA(header)) };
                if level == libc::IPPROTO_IP && kind == libc::IP_PKTINFO {
                    let arrived = unsafe { record.cast::<libc::in_pktinfo>().read_unaligned() };
                    // The index is an int on Linux and unsigned on macOS.
                    #[allow(clippy::unnecessary_cast)]
                    { described = (arrived.ipi_ifindex as u32, 0, Address::v4(arrived.ipi_addr.s_addr.to_ne_bytes(), 0)); }
                }
                if level == libc::IPPROTO_IPV6 && kind == libc::IPV6_PKTINFO {
                    let arrived = unsafe { record.cast::<libc::in6_pktinfo>().read_unaligned() };
                    #[allow(clippy::unnecessary_cast)]
                    { described = (arrived.ipi6_ifindex as u32, 0, Address::v6(arrived.ipi6_addr.s6_addr, 0, 0)); }
                }
                header = unsafe { libc::CMSG_NXTHDR(&message, header) };
            }
            described
        }
        pub fn family_of(v6: bool) -> u32 { if v6 { V6 } else { V4 } }
        /// `(level, name)` of the option that asks for the description.
        pub fn information(v6: bool) -> (i32, i32) { if v6 { (libc::IPPROTO_IPV6, libc::IPV6_RECVPKTINFO) } else { (libc::IPPROTO_IP, libc::IP_PKTINFO) } }
        /// Whether the OS holds the socket to unfragmented datagrams.
        pub fn whole(fd: i32) -> bool {
            #[cfg(not(target_vendor = "apple"))]
            { option(fd, libc::IPPROTO_IP, libc::IP_MTU_DISCOVER) == Some(libc::IP_PMTUDISC_DO) }
            #[cfg(target_vendor = "apple")]
            { option(fd, libc::IPPROTO_IP, libc::IP_DONTFRAG) == Some(1) }
        }
        /// Whether this process may open a raw ICMP socket: CAP_NET_RAW on Linux, root on macOS.
        pub fn may_open_raw() -> bool {
            let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_RAW, libc::IPPROTO_ICMP) };
            if fd < 0 { assert!(matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EPERM | libc::EACCES))); return false; }
            unsafe { libc::close(fd) };
            true
        }
    }

    /// The group's socket bound to every address of `family`, the test's own next to it, and a sender of family `sends`.
    struct Everywhere { receiver: *mut c_void, fd: i32, twin: i32, port: u16, twin_port: u16, sender: *mut c_void, v6: bool, maps: bool }
    impl Everywhere {
        fn new(family: u32, dual: bool, sends: u32) -> Self {
            let (receiver, v6) = (open(family, UDP), family == V6);
            if v6 { assert_eq!(set(receiver, sockets::IPV6_ONLY, !dual as u64), OK); }
            assert_eq!(bind(receiver, &Address { family: family as u16, ..Address::default() }), OK);
            let (port, (twin, twin_port)) = (local(receiver).port, kernel::twin(v6, dual));
            Self { receiver, fd: kernel::descriptor(v6, libc::SOCK_DGRAM, port), twin, port, twin_port, sender: open(sends, UDP), v6, maps: v6 && sends == V4 }
        }
        /// `address` as this socket names it.
        fn names(&self, address: Address) -> Address { if self.maps { mapped(address) } else { address } }
        /// One datagram to the group's socket and one to the test's own, from the same sender to the same address. The group reports the address
        /// the datagram was sent to and the interface that has it, which is what the OS attached for the test, and names the sender as
        /// sockets.receive does. A receive that only peeks describes the datagram as the one that takes it.
        fn arrives(&self, tally: &mut Tally, to: Address, interface: u32, scope: u32) {
            sent(self.sender, b"where?", Some(&Address { port: self.port, ..to }));
            assert!(readable(self.receiver));
            // A datagram to one of this host's addresses leaves from that address.
            let (from, wanted) = (self.names(Address { port: local(self.sender).port, ..to }), (interface, 0, self.names(Address { scope, ..to })));
            assert_eq!(tally.take(self.receiver, 64, PEEK), arrival(b"where?", from, wanted));
            assert_eq!(tally.take(self.receiver, 64, 0), arrival(b"where?", from, wanted));
            sent(self.sender, b"where?", Some(&Address { port: self.twin_port, ..to }));
            // The OS attaches the bare address; the scope of a link-local one is the group's to add.
            assert_eq!(kernel::attached(self.twin), (interface, 0, self.names(Address { scope: 0, ..to })));
        }
    }
    impl Drop for Everywhere { fn drop(&mut self) { close(self.receiver); close(self.sender); unsafe { libc::close(self.twin) }; } }

    fn describes(family: u32, dual: bool, sends: u32) {
        let (before, mut tally, lo) = (counts(), Tally::default(), kernel::loopback_index());
        let w = Everywhere::new(family, dual, sends);
        let (level, name) = kernel::information(w.v6);
        let to = loopback(sends, w.port);
        // Without the option a datagram has a sender and no description.
        assert_eq!((get(w.receiver, PACKET_INFORMATION), kernel::option(w.fd, level, name)), ((OK, 0), Some(0)));
        sent(w.sender, b"plain", Some(&to));
        let from = w.names(loopback(sends, local(w.sender).port));
        assert_eq!(tally.take(w.receiver, 64, 0), arrival(b"plain", from, NOWHERE));
        assert_eq!((set(w.receiver, PACKET_INFORMATION, 1), get(w.receiver, PACKET_INFORMATION), kernel::option(w.fd, level, name).map(|v| v != 0)), (OK, (OK, 1), Some(true)));
        w.arrives(&mut tally, loopback(sends, 0), lo, 0);
        if let Some((index, address)) = kernel::other_interface().filter(|_| sends == V4) { w.arrives(&mut tally, address, index, 0); }
        // macOS has a link-local address on its loopback interface: the destination carries the scope the address needs, as the sender does.
        #[cfg(target_vendor = "apple")]
        if sends == V6 { w.arrives(&mut tally, Address::v6([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], 0, lo), lo, lo); }
        // A datagram longer than the buffer is cut and the rest discarded, as in sockets.receive; its description is whole. No room at all takes it too.
        let here = (lo, 0, w.names(loopback(sends, 0)));
        for text in [&b"0123456789"[..], b"next", b"gone"] { sent(w.sender, text, Some(&to)); }
        assert_eq!(tally.take(w.receiver, 4, 0), arrival(b"0123", from, here));
        assert_eq!(tally.take(w.receiver, 64, PEEK), arrival(b"next", from, here));
        assert_eq!(tally.take(w.receiver, 64, 0), arrival(b"next", from, here));
        assert_eq!(tally.take(w.receiver, 0, 0), arrival(b"", from, here));
        // The sender is optional, and a caller built against a longer description gets the part this version knows. The sockets group counts none of this.
        #[repr(C)]
        struct Longer { info: Info, later: u64 }
        let (mut longer, mut small, mut done) = (Longer { info: Info::default(), later: 0xAAAA_AAAA_AAAA_AAAA }, [0u8; 4], 7usize);
        sent(w.sender, b"alone", Some(&to));
        let received = |stats: &sockets::Stats| (stats.receive_ok, stats.rejected_or_failed);
        let mut stats = sockets::Stats::default();
        assert_eq!(unsafe { s().read_stats.unwrap()(&mut stats, size_of::<sockets::Stats>()) }, OK);
        let held = received(&stats);
        assert_eq!(tally.note(unsafe { p().receive.unwrap()(w.receiver, small.as_mut_ptr(), 4, 0, ptr::null_mut(), &mut longer.info, size_of::<Longer>(), &mut done) }), OK);
        assert_eq!((done, &small, (longer.info.interface_index, longer.info.reserved, longer.info.destination), longer.later), (4, b"alon", here, 0xAAAA_AAAA_AAAA_AAAA));
        assert_eq!(unsafe { s().read_stats.unwrap()(&mut stats, size_of::<sockets::Stats>()) }, OK);
        assert_eq!(received(&stats), held);
        // Nothing to read: a timeout for a socket that waits, "not now" for one that does not, and no output either way.
        assert_eq!(set(w.receiver, sockets::RECEIVE_TIMEOUT, 100), OK);
        let start = Instant::now();
        assert_eq!(tally.take(w.receiver, 64, 0), failure(TIMEOUT, 64));
        assert!(start.elapsed().as_millis() >= 80);
        assert_eq!(unsafe { s().set_blocking.unwrap()(w.receiver, 0) }, OK);
        assert_eq!(tally.take(w.receiver, 64, 0), failure(WOULD_BLOCK, 64));
        assert_eq!(tally.take(w.receiver, 64, PEEK), failure(WOULD_BLOCK, 64));
        // No room is no reason to answer without a datagram (macOS would: a receive into nothing returns at once there, with nothing).
        assert_eq!(tally.take(w.receiver, 0, 0), failure(WOULD_BLOCK, 0));
        // Off again: the next datagram has no description.
        assert_eq!((set(w.receiver, PACKET_INFORMATION, 0), get(w.receiver, PACKET_INFORMATION), kernel::option(w.fd, level, name)), (OK, (OK, 0), Some(0)));
        sent(w.sender, b"quiet", Some(&to));
        assert!(readable(w.receiver));
        assert_eq!(tally.take(w.receiver, 64, 0), arrival(b"quiet", from, NOWHERE));
        tally.settled(before);
    }
    /// IPv6 is there when its loopback address can be bound.
    fn has_v6() -> bool {
        let mut socket = ptr::null_mut();
        if unsafe { s().create.unwrap()(V6, UDP, &mut socket) } != OK { return false; }
        let bound = bind(socket, &loopback(V6, 0)) == OK;
        close(socket);
        bound
    }

    #[test]
    fn packets_describe_ipv4_datagrams_by_interface_and_destination() {
        let _serial = serial();
        describes(V4, false, V4);
    }
    #[test]
    fn packets_describe_ipv6_datagrams_and_ipv4_ones_on_an_ipv6_socket() {
        let _serial = serial();
        if !has_v6() { eprintln!("packets: no IPv6 loopback here; its datagrams were not checked"); return; }
        describes(V6, false, V6);
        describes(V6, true, V4);
        describes(V6, true, V6);
    }

    #[test]
    fn packets_report_a_refused_datagram_as_sockets_do() {
        let _serial = serial();
        let (before, mut tally) = (counts(), Tally::default());
        let (gone, client) = (open(V4, UDP), open(V4, UDP));
        assert_eq!(bind(gone, &loopback(V4, 0)), OK);
        let at = local(gone);
        close(gone);
        assert_eq!((set(client, PACKET_INFORMATION, 1), unsafe { s().connect.unwrap()(client, &at) }), (OK, OK));
        sent(client, b"anyone?", None);
        assert!(readable(client));
        assert_eq!(tally.take(client, 64, 0), failure(CONNECTION_REFUSED, 64));
        close(client);
        // A socket that is not connected learns it where the OS has RECEIVE_ERRORS (Linux): once, and it is not in error afterwards.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let stranger = open(V4, UDP);
            assert_eq!((set(stranger, PACKET_INFORMATION, 1), set(stranger, sockets::RECEIVE_ERRORS, 1), unsafe { s().set_blocking.unwrap()(stranger, 0) }), (OK, OK, OK));
            sent(stranger, b"anyone?", Some(&at));
            assert!(readable(stranger));
            assert_eq!(tally.take(stranger, 64, 0), failure(CONNECTION_REFUSED, 64));
            let (mut entry, mut ready) = (PollEntry { socket: stranger, requested: READ, triggered: 99 }, 7usize);
            assert_eq!((unsafe { s().poll.unwrap()(&mut entry, 1, 0, NO_CHANNEL, &mut ready) }, entry.triggered), (OK, 0));
            assert_eq!(tally.take(stranger, 64, 0), failure(WOULD_BLOCK, 64));
            close(stranger);
        }
        tally.settled(before);
    }

    #[test]
    fn packets_refuse_other_sockets_and_malformed_arguments() {
        let _serial = serial();
        let (before, mut tally) = (counts(), Tally::default());
        // Only a datagram has a destination of its own: a stream's is its local address, and a local socket has no IP address.
        let refusing = [open(V4, TCP), open(LOCAL, UDP), open(LOCAL, TCP)];
        for socket in refusing { assert_eq!(tally.take(socket, 64, 0), failure(INVALID_ARGUMENT, 64)); unsupported(socket, PACKET_INFORMATION); unsupported(socket, sockets::RECEIVE_ERRORS); }
        for socket in refusing { close(socket); }
        let socket = open(V4, UDP);
        let (receive, mut buffer, mut done) = (p().receive.unwrap(), [0u8; 8], 7usize);
        let (mut info, mut from) = (Info { interface_index: 99, reserved: 99, destination: Address { family: 9, ..Address::default() } }, Address { family: 9, ..Address::default() });
        let (data, size) = (buffer.as_mut_ptr(), size_of::<Info>());
        // Places for the byte count and the description, which has at least this version's size, and a sender that is one when given: nothing is written before they are known.
        for status in [unsafe { receive(socket, data, 8, 0, &mut from, &mut info, size, ptr::null_mut()) }, unsafe { receive(socket, data, 8, 0, &mut from, &mut info, size, ptr::addr_of_mut!(done).cast::<u8>().wrapping_add(1).cast()) },
            unsafe { receive(socket, data, 8, 0, &mut from, ptr::null_mut(), size, &mut done) }, unsafe { receive(socket, data, 8, 0, &mut from, ptr::addr_of_mut!(info).cast::<u8>().wrapping_add(1).cast(), size, &mut done) },
            unsafe { receive(socket, data, 8, 0, &mut from, &mut info, size - 1, &mut done) }, unsafe { receive(socket, data, 8, 0, &mut from, &mut info, 0, &mut done) },
            unsafe { receive(socket, data, 8, 0, ptr::addr_of_mut!(from).cast::<u8>().wrapping_add(1).cast(), &mut info, size, &mut done) }] {
            assert_eq!((tally.note(status), done, info.interface_index, from.family), (INVALID_ARGUMENT, 7, 99, 9));
        }
        // A socket, a buffer that is one, and no flag but PEEK: the outputs hold nothing afterwards.
        let end = ptr::null_mut::<u8>().wrapping_sub(4);
        for (handle, data, capacity, flags) in [(ptr::null_mut(), data, 8, 0), (socket, ptr::null_mut(), 1, 0), (socket, data, isize::MAX as usize + 1, 0), (socket, end, 8, 0), (socket, data, 8, 2), (socket, data, 8, PEEK | 4), (socket, data, 8, u32::MAX)] {
            (done, info.interface_index, info.reserved, info.destination, from) = (7, 99, 99, Address { family: 9, ..Address::default() }, Address { family: 9, ..Address::default() });
            assert_eq!(tally.note(unsafe { receive(handle, data, capacity, flags, &mut from, &mut info, size, &mut done) }), INVALID_ARGUMENT);
            assert_eq!((done, (info.interface_index, info.reserved, info.destination), from), (0, NOWHERE, Address::default()));
        }
        close(socket);
        let (read, mut stats) = (p().read_stats.unwrap(), Stats::default());
        assert_eq!((unsafe { read(ptr::null_mut(), size_of::<Stats>()) }, unsafe { read(&mut stats, size_of::<Stats>() - 1) }, unsafe { read(ptr::addr_of_mut!(stats).cast::<u8>().wrapping_add(1).cast(), size_of::<Stats>()) }),
            (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        tally.settled(before);
    }

    /// The Internet checksum of `bytes` on top of `sum`; over a message that carries its own it comes out as 0.
    fn checksum(bytes: &[u8], mut sum: u32) -> u16 {
        for pair in bytes.chunks(2) { sum += (pair[0] as u32) << 8 | pair.get(1).map_or(0, |low| *low as u32); }
        while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
        !(sum as u16)
    }
    const PAYLOAD: &[u8] = b"pal-packets-echo";
    /// An echo request as Ping builds it: type, code 0, checksum, identifier, sequence number, payload. ICMPv6's checksum covers the addresses, so the OS fills it in.
    fn ask(raw: *mut c_void, v6: bool, id: u16, sequence: u16, to: Option<&Address>) {
        let mut request = vec![if v6 { 128 } else { 8 }, 0, 0, 0, (id >> 8) as u8, id as u8, (sequence >> 8) as u8, sequence as u8];
        request.extend_from_slice(PAYLOAD);
        if !v6 { let sum = checksum(&request, 0); (request[2], request[3]) = ((sum >> 8) as u8, sum as u8); }
        sent(raw, &request, to);
    }
    /// What the header on the wire said about the request, which the loopback hands to every raw socket as it hands it the reply.
    #[derive(Default)]
    struct Wire { requests: u32, hops: u8, whole: bool }
    /// Reads until the reply to `sequence` has come, through the packets group or through the sockets group. What else ICMP brings meanwhile is
    /// not the test's business. Over IPv4 a message starts with its 20-byte IP header, over IPv6 with the ICMPv6 header.
    fn answered(tally: &mut Tally, raw: *mut c_void, v6: bool, described: bool, id: u16, sequence: u16, wire: &mut Wire) {
        let from = loopback(kernel::family_of(v6), 0);
        let here = (kernel::loopback_index(), 0, from);
        let mut read = |flags: u32| {
            if described { return tally.take(raw, 128, flags); }
            let (mut data, mut done, mut from) = (vec![0xAAu8; 128], 7usize, Address::default());
            let status = unsafe { s().receive.unwrap()(raw, data.as_mut_ptr(), 128, flags, &mut from, &mut done) };
            data.truncate(done);
            Arrival { status, data, from, info: here }
        };
        for _ in 0..64 {
            let peeked = read(PEEK);
            assert_eq!(peeked.status, OK);
            let icmp = &peeked.data[if v6 { 0 } else { 20.min(peeked.data.len()) }..];
            let ours = icmp.len() >= 8 && (v6 || (peeked.data[0] == 0x45 && peeked.data[9] == libc::IPPROTO_ICMP as u8)) && icmp[4..8] == [(id >> 8) as u8, id as u8, (sequence >> 8) as u8, sequence as u8];
            if ours {
                // Both the request and the reply went from the loopback address to the loopback address, and a raw socket's sender has no port.
                assert_eq!((peeked.from, peeked.info, icmp[1], &icmp[8..]), (from, here, 0, PAYLOAD));
                if v6 {
                    let mut pseudo = [0u8; 40];
                    (pseudo[15], pseudo[31], pseudo[35], pseudo[39]) = (1, 1, icmp.len() as u8, libc::IPPROTO_ICMPV6 as u8);
                    assert_eq!(checksum(icmp, !checksum(&pseudo, 0) as u32), 0);
                } else { assert_eq!((checksum(icmp, 0), &peeked.data[12..20]), (0, &[127, 0, 0, 1, 127, 0, 0, 1][..])); }
                if icmp[0] == if v6 { 128 } else { 8 } { wire.requests += 1; if !v6 { (wire.hops, wire.whole) = (peeked.data[8], peeked.data[6] & 0x40 != 0); } }
            }
            // The message that was peeked at is taken now: the same bytes, sender and description.
            assert_eq!(read(0), peeked);
            if ours && icmp[0] == if v6 { 129 } else { 0 } { return; }
        }
        panic!("no echo reply");
    }
    /// A raw socket of the sockets group: an echo request to the loopback address, and the reply through both groups.
    fn echoes(tally: &mut Tally, v6: bool) {
        let (family, id) = (kernel::family_of(v6), std::process::id() as u16);
        let (raw, mut at, mut wire) = (open(family, RAW), loopback(family, 0), Wire::default());
        // A raw socket has no ports, whatever the OS keeps in their place (Linux: the protocol number).
        assert_eq!(local(raw), Address { family: family as u16, ..Address::default() });
        assert_eq!((set(raw, PACKET_INFORMATION, 1), get(raw, PACKET_INFORMATION)), (OK, (OK, 1)));
        // Ping asks a raw socket for what the network reports about its requests, where the OS has the option.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert_eq!((get(raw, sockets::RECEIVE_ERRORS), set(raw, sockets::RECEIVE_ERRORS, 1), get(raw, sockets::RECEIVE_ERRORS)), ((OK, 0), OK, (OK, 1)));
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        unsupported(raw, sockets::RECEIVE_ERRORS);
        ask(raw, v6, id, 1, Some(&at));
        assert!(readable(raw));
        answered(tally, raw, v6, true, id, 1, &mut wire);
        // The hop limit and the fragmentation switch Ping sets show in the header of what the socket sends. A port in the target is dropped:
        // Linux would read an IPv6 one as a protocol number and refuse it.
        assert_eq!((set(raw, HOPS, 5), get(raw, HOPS)), (OK, (OK, 5)));
        if v6 { unsupported(raw, DONT_FRAGMENT); } else { assert_eq!((get(raw, DONT_FRAGMENT), set(raw, DONT_FRAGMENT, 1), get(raw, DONT_FRAGMENT)), ((OK, 0), OK, (OK, 1))); }
        at.port = 77;
        ask(raw, v6, id, 2, Some(&at));
        answered(tally, raw, v6, false, id, 2, &mut wire);
        if !v6 {
            assert_eq!((wire.requests, wire.hops, wire.whole), (2, 5, true));
            assert_eq!(set(raw, DONT_FRAGMENT, 0), OK);
            ask(raw, v6, id, 3, Some(&at));
            answered(tally, raw, v6, true, id, 3, &mut wire);
            assert_eq!((wire.requests, wire.hops, wire.whole), (3, 5, false));
        }
        // bind and connect take an address and drop its port; the endpoints report none.
        assert_eq!((bind(raw, &Address { port: 55, ..at }), local(raw)), (OK, Address { port: 0, ..at }));
        let mut peer = Address::default();
        assert_eq!((unsafe { s().connect.unwrap()(raw, &at) }, unsafe { s().peer_address.unwrap()(raw, &mut peer) }, peer), (OK, OK, Address { port: 0, ..at }));
        ask(raw, v6, id, 4, None);
        answered(tally, raw, v6, true, id, 4, &mut wire);
        close(raw);
    }
    #[test]
    fn raw_sockets_echo_with_the_privilege_and_are_denied_without() {
        let _serial = serial();
        let (before, mut tally) = (counts(), Tally::default());
        let create = |family: u32| { let mut socket = ptr::dangling_mut::<u64>().cast(); (unsafe { s().create.unwrap()(family, RAW, &mut socket) }, socket) };
        // A local socket speaks no ICMP, with or without the privilege.
        assert_eq!(create(LOCAL), (INVALID_ARGUMENT, ptr::null_mut()));
        if kernel::may_open_raw() {
            echoes(&mut tally, false);
            if has_v6() { echoes(&mut tally, true); }
        } else {
            assert_eq!((create(V4), create(V6)), ((ACCESS_DENIED, ptr::null_mut()), (ACCESS_DENIED, ptr::null_mut())));
            eprintln!("packets: this process may not open raw sockets; the echo and the headers on the wire were not checked");
        }
        tally.settled(before);
    }

    #[test]
    fn dont_fragment_is_held_by_the_os_and_keeps_datagrams_whole() {
        let _serial = serial();
        let (sender, receiver) = (open(V4, UDP), open(V4, UDP));
        assert_eq!((bind(receiver, &loopback(V4, 0)), bind(sender, &loopback(V4, 0))), (OK, OK));
        let (at, fd) = (local(receiver), kernel::descriptor(false, libc::SOCK_DGRAM, local(sender).port));
        // What nobody set lets a datagram that is too long be fragmented, so it reads as off.
        assert_eq!((get(sender, DONT_FRAGMENT), kernel::whole(fd)), ((OK, 0), false));
        assert_eq!((set(sender, DONT_FRAGMENT, 1), get(sender, DONT_FRAGMENT), kernel::whole(fd)), (OK, (OK, 1), true));
        // macOS gives its loopback an MTU of 16384 and a datagram socket room for 9216 bytes: with more room, a datagram longer than the MTU is
        // refused while it may not be fragmented and sent once it may. No IPv4 datagram is longer than the MTU of the Linux loopback (65536).
        #[cfg(target_vendor = "apple")]
        {
            let long = vec![0x5Au8; 20000];
            assert_eq!((set(sender, sockets::SEND_BUFFER, 65536), set(receiver, sockets::RECEIVE_BUFFER, 65536)), (OK, OK));
            assert_eq!(send(sender, &long, Some(&at)), (MESSAGE_TOO_LARGE, 0));
            assert_eq!((set(sender, DONT_FRAGMENT, 0), send(sender, &long, Some(&at))), (OK, (OK, long.len())));
        }
        assert_eq!((set(sender, DONT_FRAGMENT, 0), get(sender, DONT_FRAGMENT), kernel::whole(fd)), (OK, (OK, 0), false));
        #[cfg(not(target_vendor = "apple"))]
        {
            // PROBE, which only a caller behind the boundary's back sets, keeps datagrams whole as well; DONT is what off sets.
            assert_eq!(kernel::option(fd, libc::IPPROTO_IP, libc::IP_MTU_DISCOVER), Some(libc::IP_PMTUDISC_DONT));
            let probe = libc::IP_PMTUDISC_PROBE;
            assert_eq!(unsafe { libc::setsockopt(fd, libc::IPPROTO_IP, libc::IP_MTU_DISCOVER, ptr::addr_of!(probe).cast(), size_of::<i32>() as libc::socklen_t) }, 0);
            assert_eq!(get(sender, DONT_FRAGMENT), (OK, 1));
        }
        sent(sender, b"still a socket", Some(&at));
        assert!(readable(receiver));
        // A stream's segments may be kept whole too; the switch is IPv4's, and a local socket has no IP at all.
        let (stream, local_socket) = (open(V4, TCP), open(LOCAL, UDP));
        assert_eq!((get(stream, DONT_FRAGMENT), set(stream, DONT_FRAGMENT, 1), get(stream, DONT_FRAGMENT)), ((OK, 0), OK, (OK, 1)));
        unsupported(local_socket, DONT_FRAGMENT);
        if has_v6() { for kind in [UDP, TCP] { let six = open(V6, kind); unsupported(six, DONT_FRAGMENT); close(six); } }
        for socket in [sender, receiver, stream, local_socket] { close(socket); }
    }
}
