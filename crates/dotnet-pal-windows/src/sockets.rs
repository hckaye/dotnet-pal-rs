//! Internet sockets of the desktop port, on `socket2`.
//!
//! A handle is a boxed `socket2::Socket` plus what the OS does not answer the same
//! way everywhere: the blocking mode (an expired RECEIVE or SEND timeout arrives as
//! the error a non-blocking socket uses for "not now", and only the mode tells them
//! apart), and the kind and family (for an option the protocol lacks Linux answers
//! ENOPROTOOPT or EOPNOTSUPP, macOS EINVAL, which would read as a bad argument).
//! `socket2` makes every descriptor close-on-exec and sets SO_NOSIGPIPE on Apple
//! targets, for accepted sockets too. It adds no flag to a send, so on every other
//! Unix each send carries MSG_NOSIGNAL here.
//!
//! Readiness is poll(2), or `WSAPoll`, over one slot per distinct socket. A wake
//! channel is made on first use: `wake` writes it one byte, the channel's poll
//! lists its reading end and drains it, so a wake stays pending for that channel
//! and reaches no other. On Unix the channel is a socket pair, where the byte is
//! readable when `wake` returns. A loopback UDP socket connected to itself, which
//! is what `WSAPoll` can wait on, was measured on macOS to deliver after the send
//! returns: there a wake could miss a next poll that ends at once for another
//! reason and end the one after it instead. Windows has that channel.
//!
//! macOS poll(2) differs from Linux in ways measured on Darwin 25 and evened out
//! in `ready::state`: a descriptor listed twice is answered once; POLLHUP means the
//! end of either direction, is reported only for a direction the poll asked about,
//! and withholds POLLOUT; POLLERR was not raised for a refused connect, a reset or
//! a datagram socket's pending error. What stays different: once a poll has found a
//! peer's half close on a socket whose entries request nothing, it stops watching
//! that socket, so a full hangup that follows is reported by the next poll; and a
//! datagram socket's pending error shows as READ to an entry that reads, never as
//! ERROR.
//!
//! Keep-alive tuning and the IPv4 multicast interface go to the OS directly (`tuning`):
//! `socket2` has the first behind its `all` feature, where setting it also switches
//! keep-alive on, and the second by address only. Linux sets the interface from an
//! index but reads back an address (4 bytes, measured on Linux 6.12), so the handle
//! remembers the index it set; macOS reads and writes it as IP_MULTICAST_IFINDEX.
//! macOS takes a keep-alive time or count of 0 and keeps its default, where Linux
//! refuses it, so 0 is refused here; macOS answers -1 for an IPv6 hop limit nobody
//! set, which is reported as the system's default (net.inet6.ip6.hlim).
//!
//! A Unix domain socket is a socket of this provider made with the LOCAL family; the
//! local_sockets provider binds and connects it. The handle says that it is one: the
//! calls that take an IP address refuse it, its endpoints answer with the bare
//! family, and the sender of what it receives is answered from the handle, because
//! the OS names a local sender by whether it has a path (Linux names the sender of
//! a stream that has one, macOS gives a datagram's sender without one an empty name;
//! both measured). Windows has no provider for them: the family is UNSUPPORTED there.
//!
//! A raw socket speaks its family's ICMP and needs a privilege: CAP_NET_RAW on Linux,
//! root on macOS, where `socket` answers EPERM otherwise, which is ACCESS_DENIED here.
//! macOS gives an unprivileged process ICMP through a datagram socket instead; that is
//! not offered under the raw kind, because it is not the same socket: IP_RECVPKTINFO is
//! EINVAL on it (measured on Darwin 25). A raw socket has no ports. Linux reports its
//! protocol number as its local port, keeps the port a connect named, and reads the port
//! an IPv6 send names as a protocol number (EINVAL for any but 58, all measured), so the
//! port of an address that goes to a raw socket is dropped and every address it reports
//! has port 0; only a connect on Linux names a port, the protocol number, because its
//! getpeername calls a socket whose peer has port 0 not connected (measured). Windows
//! has no raw kind here: UNSUPPORTED.
//!
//! PACKET_INFORMATION is IP_PKTINFO (IP_RECVPKTINFO on macOS, the same number) or
//! IPV6_RECVPKTINFO by the socket's family. An IPv6 socket that also takes IPv4 traffic
//! needs nothing more: both systems describe an IPv4 datagram there by IPV6_PKTINFO with
//! the IPv4-mapped destination, the form its sender has too, and macOS refuses IP_PKTINFO
//! on an IPv6 socket (EINVAL; all measured). DONT_FRAGMENT is IP_MTU_DISCOVER on Linux
//! (DO when on, DONT when off; DO and PROBE read as on, the default WANT as off) and
//! IP_DONTFRAG on macOS; macOS has EINVAL for it on an IPv6 socket and Linux would take
//! it, so the answer for an IPv6 socket is made here. Both are UNSUPPORTED on Windows.
//!
//! RECEIVE_ERRORS is Linux's IP_RECVERR or IPV6_RECVERR by the socket's family, and both
//! on an IPv6 datagram socket: IPV6_RECVERR alone drops what ICMP reports about a
//! datagram sent to an IPv4-mapped address (measured). With the option Linux also keeps
//! every such error in a queue of its own and calls the socket in error (POLLERR) for
//! as long as that queue holds one, after the failing call has long reported it
//! (measured); nothing in the boundary reads that queue and a level-triggered poll
//! would report ERROR for ever, so whatever reports a failure of the socket empties it.
//! macOS has no such option (none in its <netinet/in.h>; a datagram socket that is not
//! connected never learns that nobody listened where it sent to, a connected one does;
//! measured on Darwin 25): UNSUPPORTED there and on Windows.
//!
//! Names are resolved with getaddrinfo itself on Unix: std turns its failure code
//! into text, which cannot tell a name without addresses from an answer that could
//! not be obtained. On Windows std keeps the Winsock code, so `ToSocketAddrs` is
//! enough. The Windows branches compile but have not been executed here.
use super::{borrow, boxed, take, Std};
use dotnet_pal_rs::kernel::INFINITE;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::sockets::{self, Address, PollEntry, DATAGRAM, IPV4, IPV6, LOCAL, POLL_ERROR, POLL_HANGUP, POLL_READ, POLL_WRITE, RAW, RECEIVE_PEEK, STREAM};
use socket2::{Domain, MaybeUninitSlice, SockAddr, Type};
use std::{collections::HashMap, ffi::c_void, io, mem::MaybeUninit, net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV6}, ptr};
use std::{sync::{atomic::{AtomicBool, AtomicU32, Ordering}, OnceLock}, time::{Duration, Instant}};

/// The calls of the network, local_sockets and packets groups take these handles, hence the visibility.
pub(crate) struct Socket { pub(crate) inner: socket2::Socket, blocking: AtomicBool, pub(crate) stream: bool, pub(crate) v6: bool, pub(crate) local: bool, raw: bool, multicast_interface: AtomicU32 }
/// What every endpoint of a local socket answers with: its path is the local_sockets group's to report.
const BARE: Address = Address { family: LOCAL as u16, port: 0, scope: 0, address: [0; 16] };
impl Socket {
    fn wrap(inner: socket2::Socket, stream: bool, v6: bool, local: bool, raw: bool) -> *mut c_void {
        boxed(Self { inner, blocking: AtomicBool::new(true), stream, v6, local, raw, multicast_interface: AtomicU32::new(0) })
    }
    /// An address as the socket takes or reports it: without its port when the socket is a raw one.
    fn ported(&self, address: Address) -> Address { if self.raw { Address { port: 0, ..address } } else { address } }
    /// Who sent what the socket received. A stream names no sender: the OS leaves the address empty and the answer is `None`.
    /// A local datagram has a local sender, whatever the OS makes of its name.
    pub(crate) fn sender(&self, named: &SockAddr) -> Option<Address> {
        if self.local { (!self.stream).then_some(BARE) } else { named.as_socket().map(|address| self.ported(boundary(address))) }
    }
    /// The socket, for a call that takes an IP address: a local socket has none to bind, reach or send to.
    unsafe fn internet<'a>(socket: *mut c_void) -> Result<&'a Self> {
        let socket = unsafe { borrow::<Self>(socket) }?;
        if socket.local { Err(Error::InvalidArgument) } else { Ok(socket) }
    }
    /// `Err(Unsupported)` for an option the socket's protocol does not have, before the OS is asked.
    fn has(&self, option: u32) -> Result<()> {
        let missing = match option {
            sockets::NO_DELAY | sockets::KEEP_ALIVE_IDLE | sockets::KEEP_ALIVE_INTERVAL | sockets::KEEP_ALIVE_COUNT => !self.stream || self.local,
            sockets::MULTICAST_HOPS | sockets::MULTICAST_LOOPBACK | sockets::MULTICAST_INTERFACE => self.stream || self.local,
            sockets::HOPS => self.local,
            sockets::IPV6_ONLY => !self.v6,
            sockets::PACKET_INFORMATION => self.stream || self.local || cfg!(windows),
            sockets::DONT_FRAGMENT => self.v6 || self.local || cfg!(windows),
            sockets::RECEIVE_ERRORS => self.stream || self.local || !cfg!(any(target_os = "linux", target_os = "android")),
            _ => false,
        };
        if missing { Err(Error::Unsupported) } else { Ok(()) }
    }
}
/// Inside `poll` only: the peer has ended its sending direction. An entry that reads
/// learns it as `POLL_HANGUP`, which is what POLLRDHUP gives the Linux provider.
const PEER_CLOSED: u32 = 1 << 16;

/// The boundary status of a socket failure: errno on Unix, the Winsock code on
/// Windows, `ErrorKind` for the errors std raises without asking the OS.
fn error(e: io::Error) -> Error {


    if let Some(code) = e.raw_os_error() {
        use windows_sys::Win32::Networking::WinSock as w;
        return match code {
            w::WSAEWOULDBLOCK => Error::WouldBlock, w::WSAEINPROGRESS | w::WSAEALREADY => Error::InProgress, w::WSAESHUTDOWN => Error::BrokenPipe,
            w::WSAECONNREFUSED => Error::ConnectionRefused, w::WSAECONNRESET => Error::ConnectionReset, w::WSAECONNABORTED => Error::ConnectionAborted,
            w::WSAENOTCONN => Error::NotConnected, w::WSAEISCONN => Error::AlreadyConnected, w::WSAEADDRINUSE => Error::AddressInUse,
            w::WSAEADDRNOTAVAIL => Error::AddressNotAvailable, w::WSAENETUNREACH | w::WSAENETDOWN => Error::NetworkUnreachable,
            w::WSAEHOSTUNREACH => Error::HostUnreachable, w::WSAEMFILE => Error::TooManyHandles, w::WSAEMSGSIZE => Error::MessageTooLarge,
            w::WSAEACCES => Error::AccessDenied, w::WSAETIMEDOUT | w::WSATRY_AGAIN => Error::Timeout, w::WSAENOBUFS => Error::OutOfMemory,
            w::WSAEINVAL => Error::InvalidArgument, w::WSAHOST_NOT_FOUND | w::WSANO_DATA => Error::NotFound,
            w::WSAEAFNOSUPPORT | w::WSAEPROTONOSUPPORT | w::WSAEOPNOTSUPP | w::WSAENOPROTOOPT => Error::Unsupported,
            _ => Error::Os,
        };
    }
    use io::ErrorKind as Kind;
    match e.kind() {
        Kind::WouldBlock => Error::WouldBlock, Kind::BrokenPipe => Error::BrokenPipe, Kind::ConnectionRefused => Error::ConnectionRefused,
        Kind::ConnectionReset => Error::ConnectionReset, Kind::ConnectionAborted => Error::ConnectionAborted, Kind::NotConnected => Error::NotConnected,
        Kind::AddrInUse => Error::AddressInUse, Kind::AddrNotAvailable => Error::AddressNotAvailable,
        Kind::NetworkUnreachable | Kind::NetworkDown => Error::NetworkUnreachable, Kind::HostUnreachable => Error::HostUnreachable,
        Kind::PermissionDenied => Error::AccessDenied, Kind::TimedOut => Error::Timeout, Kind::OutOfMemory => Error::OutOfMemory,
        Kind::InvalidInput => Error::InvalidArgument, Kind::Unsupported => Error::Unsupported,
        _ => Error::Os,
    }
}
/// The failure of a transfer or an accept. Unix reports an expired RECEIVE or SEND
/// timeout with the error of a non-blocking socket that has nothing yet; the mode
/// the handle was put in tells them apart without asking the OS.
pub(crate) fn waited(socket: &Socket, e: io::Error) -> Error {
    match error(e) { Error::WouldBlock if socket.blocking.load(Ordering::Relaxed) => Error::Timeout, Error::WouldBlock => Error::WouldBlock, other => { tuning::forget_errors(&socket.inner); other } }
}
pub(crate) fn retry<T>(mut call: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop { match call() { Err(e) if e.kind() == io::ErrorKind::Interrupted => continue, other => return other } }
}

pub(crate) fn native(address: &Address) -> Result<SockAddr> {
    let a = address.address;
    Ok(SockAddr::from(match address.family as u32 {
        IPV4 => SocketAddr::from((Ipv4Addr::new(a[0], a[1], a[2], a[3]), address.port)),
        IPV6 => SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(a), address.port, 0, address.scope)),
        _ => return Err(Error::InvalidArgument),
    }))
}
fn boundary(address: SocketAddr) -> Address {
    match address {
        SocketAddr::V4(a) => Address::v4(a.ip().octets(), a.port()),
        SocketAddr::V6(a) => Address::v6(a.ip().octets(), a.port(), a.scope_id()),
    }
}
fn endpoint(socket: &Socket, address: io::Result<SockAddr>) -> Result<Address> {
    let address = address.map_err(error)?;
    if socket.local { Ok(BARE) } else { address.as_socket().map(|address| socket.ported(boundary(address))).ok_or(Error::Os) }
}




mod ready {
    use super::{POLL_ERROR, POLL_HANGUP, POLL_READ, POLL_WRITE};
    use std::{io, os::windows::io::AsRawSocket, time::Duration};
    use windows_sys::Win32::Networking::WinSock::{ioctlsocket, WSAPoll, FIONREAD, INVALID_SOCKET, MSG_PEEK, POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, POLLWRNORM, SOCKET, SOCKET_ERROR, WSAPOLLFD};
    pub type Slot = WSAPOLLFD;
    pub type Raw = SOCKET;
    pub const PEEK: i32 = MSG_PEEK;
    pub const SEND: i32 = 0;
    pub const ALWAYS: i16 = 0;
    pub fn raw(socket: &impl AsRawSocket) -> Raw { socket.as_raw_socket() as SOCKET }
    pub fn slot(fd: Raw, events: i16) -> Slot { WSAPOLLFD { fd, events, revents: 0 } }
    pub fn events(requested: u32) -> i16 { (if requested & POLL_READ != 0 { POLLRDNORM } else { 0 }) | (if requested & POLL_WRITE != 0 { POLLWRNORM } else { 0 }) }
    pub fn wait(set: &mut [Slot], timeout_ms: i32) -> io::Result<()> {
        // WSAPoll is documented to want an event for every socket and a socket in every array. A slot that
        // asks for nothing is skipped (a negative descriptor is), so its hangup goes unreported, and a wait
        // on no socket at all is a sleep, in pieces when it has no end.
        for slot in set.iter_mut() { if slot.events == 0 { slot.fd = INVALID_SOCKET; } }
        if set.iter().all(|slot| slot.fd == INVALID_SOCKET) {
            std::thread::sleep(Duration::from_millis(if timeout_ms < 0 { 1000 } else { timeout_ms as u64 }));
            return Ok(());
        }
        if unsafe { WSAPoll(set.as_mut_ptr(), set.len() as u32, timeout_ms) } == SOCKET_ERROR { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
    fn bit(set: bool, value: u32) -> u32 { if set { value } else { 0 } }
    pub fn state(slot: &Slot) -> u32 {
        // The documentation disagrees with itself on what a skipped slot holds afterwards.
        if slot.fd == INVALID_SOCKET { return 0; }
        let r = slot.revents;
        bit(r & POLLRDNORM != 0, POLL_READ) | bit(r & POLLWRNORM != 0, POLL_WRITE) | bit(r & (POLLERR | POLLNVAL) != 0, POLL_ERROR) | bit(r & POLLHUP != 0, POLL_HANGUP)
    }
    pub fn available(socket: &impl AsRawSocket) -> io::Result<u64> {
        let mut value = 0u32;
        if unsafe { ioctlsocket(raw(socket), FIONREAD, &mut value) } != 0 { return Err(io::Error::last_os_error()); }
        Ok(value as u64)
    }
}



mod tuning {
    use std::{io, net::Ipv4Addr, os::windows::io::AsRawSocket, ptr, sync::atomic::AtomicU32};
    use windows_sys::Win32::Networking::WinSock::{getsockopt, setsockopt, IPPROTO_TCP, SOCKET, TCP_KEEPCNT, TCP_KEEPIDLE, TCP_KEEPINTVL};
    pub const TCP: i32 = IPPROTO_TCP;
    pub const IDLE: i32 = TCP_KEEPIDLE;
    pub const INTERVAL: i32 = TCP_KEEPINTVL;
    pub const COUNT: i32 = TCP_KEEPCNT;
    pub fn get(socket: &impl AsRawSocket, level: i32, name: i32) -> io::Result<i32> {
        let (mut value, mut length) = (0i32, 4i32);
        if unsafe { getsockopt(socket.as_raw_socket() as SOCKET, level, name, ptr::addr_of_mut!(value).cast(), &mut length) } != 0 { return Err(io::Error::last_os_error()); }
        Ok(value)
    }
    pub fn set(socket: &impl AsRawSocket, level: i32, name: i32, value: &i32) -> io::Result<()> {
        if unsafe { setsockopt(socket.as_raw_socket() as SOCKET, level, name, (value as *const i32).cast(), 4) } != 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    }
    pub fn exists(_: u32) -> bool { true }
    pub fn hops_v6(socket: &socket2::Socket) -> io::Result<i32> { socket.unicast_hops_v6().map(|hops| hops as i32) }
    /// Winsock takes an interface index where an address goes, in network byte order and below 2^24.
    pub fn interface_v4(socket: &socket2::Socket, _: &AtomicU32) -> io::Result<u32> { socket.multicast_if_v4().map(u32::from) }
    pub fn set_interface_v4(socket: &socket2::Socket, index: u32, _: &AtomicU32) -> io::Result<()> {
        if index >= 1 << 24 { return Err(io::Error::from(io::ErrorKind::InvalidInput)); }
        socket.set_multicast_if_v4(&Ipv4Addr::from(index))
    }
    /// The socket's `has` answers for these three before they are reached.
    pub fn receives_errors(_: &socket2::Socket, _: bool) -> io::Result<bool> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    pub fn set_receives_errors(_: &socket2::Socket, _: bool, _: bool, _: bool) -> io::Result<()> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    pub fn forget_errors(_: &socket2::Socket) {}
    pub fn reports_packets(_: &socket2::Socket, _: bool) -> io::Result<bool> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    pub fn set_reports_packets(_: &socket2::Socket, _: bool, _: bool) -> io::Result<()> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    pub fn dont_fragment(_: &socket2::Socket) -> io::Result<bool> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    pub fn set_dont_fragment(_: &socket2::Socket, _: bool) -> io::Result<()> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
}

/// One wake channel: `wait` is readable from a `signal` until the channel's poll
/// drains it, which is what makes a wake with no poll in progress reach the
/// channel's next poll and no other channel's. Both ends are non-blocking.


/// `WSAPoll` waits on sockets only, so the channel is a loopback UDP socket connected to
/// itself, which takes datagrams from its own address alone.

struct Waker { wait: std::net::UdpSocket }

impl Waker {
    fn new() -> io::Result<Self> {
        let wait = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).or_else(|_| std::net::UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)))?;
        wait.connect(wait.local_addr()?)?;
        wait.set_nonblocking(true)?;
        Ok(Self { wait })
    }
    fn signal(&self) -> io::Result<usize> { self.wait.send(&[1]) }
    fn drain(&self) { let mut sink = [0u8; 8]; while self.wait.recv(&mut sink).is_ok() {} }
}
static WAKERS: [OnceLock<Waker>; sockets::POLL_CHANNELS as usize] = [const { OnceLock::new() }; sockets::POLL_CHANNELS as usize];
fn waker(channel: u32) -> Result<&'static Waker> {
    let slot = WAKERS.get(channel as usize).ok_or(Error::InvalidArgument)?;
    if let Some(waker) = slot.get() { return Ok(waker); }
    let waker = Waker::new().map_err(error)?;
    // A creator that lost the race drops its own and uses the winner's.
    Ok(slot.get_or_init(|| waker))
}

/// Appends `address` unless it is listed already (a hosts file may name one address on several lines).
unsafe fn list(out: *mut Address, found: &mut usize, address: Address) {
    if (0..*found).any(|i| unsafe { out.add(i).read() } == address) { return; }
    unsafe { out.add(*found).write(address) };
    *found += 1;
}


unsafe fn lookup(name: &[u8], family: u32, out: *mut Address, capacity: usize) -> Result<usize> {
    // A name that is not text names no host.
    let name = std::str::from_utf8(name).map_err(|_| Error::NotFound)?;
    let mut found = 0;
    for address in std::net::ToSocketAddrs::to_socket_addrs(&(name, 0)).map_err(error)?.map(boundary) {
        if found == capacity { break; }
        if family == 0 || address.family as u32 == family { unsafe { list(out, &mut found, address) }; }
    }
    Ok(found)
}


fn host() -> Result<Vec<u8>> {
    use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    let (mut wide, mut length) = ([0u16; sockets::MAX_HOST_NAME + 1], sockets::MAX_HOST_NAME as u32 + 1);
    if unsafe { GetComputerNameExW(ComputerNameDnsHostname, wide.as_mut_ptr(), &mut length) } == 0 { return Err(Error::Os); }
    String::from_utf16(&wide[..length as usize]).map(String::into_bytes).map_err(|_| Error::Os)
}

impl port::Sockets for Std {
    unsafe fn create(family: u32, kind: u32) -> Result<*mut c_void> {
        let domain = match family {
            IPV4 => Domain::IPV4, IPV6 => Domain::IPV6,
            LOCAL if cfg!(unix) => Domain::UNIX,
            // No local_sockets provider could bind or connect it.
            LOCAL => return Err(Error::Unsupported),
            _ => return Err(Error::InvalidArgument),
        };
        let (shape, protocol) = match kind {
            STREAM => (Type::STREAM, None), DATAGRAM => (Type::DGRAM, None),
            // `socket2` names the raw type behind a feature. Without the privilege the OS answers EPERM, which reads as ACCESS_DENIED.


            RAW if family != LOCAL => return Err(Error::Unsupported),
            _ => return Err(Error::InvalidArgument),
        };
        Ok(Socket::wrap(socket2::Socket::new(domain, shape, protocol).map_err(error)?, kind == STREAM, family == IPV6, family == LOCAL, kind == RAW))
    }
    unsafe fn close(socket: *mut c_void) -> Result<()> { drop(unsafe { take::<Socket>(socket) }?); Ok(()) }
    unsafe fn bind(socket: *mut c_void, address: &Address) -> Result<()> { let socket = unsafe { Socket::internet(socket) }?; socket.inner.bind(&native(&socket.ported(*address))?).map_err(error) }
    unsafe fn listen(socket: *mut c_void, backlog: u32) -> Result<()> {
        unsafe { borrow::<Socket>(socket) }?.inner.listen(backlog.min(i32::MAX as u32) as i32).map_err(error)
    }
    unsafe fn accept(socket: *mut c_void) -> Result<(*mut c_void, Address)> {
        let listener = unsafe { borrow::<Socket>(socket) }?;
        let (inner, peer) = retry(|| listener.inner.accept()).map_err(|e| waited(listener, e))?;
        // Outside Linux an accepted socket inherits the listener's non-blocking mode; the contract starts it blocking.
        if !listener.blocking.load(Ordering::Relaxed) { inner.set_nonblocking(false).map_err(error)?; }
        Ok((Socket::wrap(inner, listener.stream, listener.v6, listener.local, false), if listener.local { BARE } else { peer.as_socket().map(boundary).unwrap_or_default() }))
    }
    unsafe fn connect(socket: *mut c_void, address: &Address) -> Result<()> {
        let socket = unsafe { Socket::internet(socket) }?;
        #[allow(unused_mut)]
        let mut peer = socket.ported(*address);
        // Linux calls a socket connected when its peer has a port (getpeername is ENOTCONN otherwise, measured), and a raw socket's peer has
        // none: the connect names the protocol number, which is what Linux itself reports as such a socket's local port.

        match socket.inner.connect(&native(&peer)?) {
            Ok(()) => Ok(()),
            // An interrupted connect carries on in the OS; a second call would report EALREADY. Wait for its outcome instead.

            // Winsock reports a connect in progress with the code every other call uses for "not now".
            Err(e) => Err(match error(e) { Error::WouldBlock if cfg!(windows) => Error::InProgress, other => other }),
        }
    }
    unsafe fn send(socket: *mut c_void, data: *const u8, size: usize, to: Option<&Address>) -> Result<usize> {
        let socket = if to.is_some() { unsafe { Socket::internet(socket) } } else { unsafe { borrow::<Socket>(socket) } }?;
        let (bytes, to) = (unsafe { std::slice::from_raw_parts(data, size) }, to.map(|to| native(&socket.ported(*to))).transpose()?);
        retry(|| match &to { Some(to) => socket.inner.send_to_with_flags(bytes, to, ready::SEND), None => socket.inner.send_with_flags(bytes, ready::SEND) })
            .map_err(|e| waited(socket, e))
    }
    unsafe fn receive(socket: *mut c_void, out: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<Address>)> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        let flags = if flags & RECEIVE_PEEK != 0 { ready::PEEK } else { 0 };
        // macOS answers a datagram receive into no room at once and with nothing while no datagram is there, where Linux waits for one (both
        // measured); with a datagram there both take it. One byte of room that is not reported makes every OS wait, or say that it would have to.
        let mut spare = [MaybeUninit::<u8>::uninit()];
        let room = if capacity == 0 && !socket.stream { &mut spare[..] } else { unsafe { std::slice::from_raw_parts_mut(out.cast::<MaybeUninit<u8>>(), capacity) } };
        // The vectored call is the one `socket2` lets end in a truncated datagram on Windows, where Winsock calls that an error.
        let mut parts = [MaybeUninitSlice::new(room)];
        let (done, _, sender) = retry(|| socket.inner.recv_from_vectored_with_flags(&mut parts, flags)).map_err(|e| waited(socket, e))?;
        Ok((done.min(capacity), socket.sender(&sender)))
    }
    unsafe fn shutdown(socket: *mut c_void, how: u32) -> Result<()> {
        let how = match how { sockets::SHUTDOWN_READ => Shutdown::Read, sockets::SHUTDOWN_WRITE => Shutdown::Write, sockets::SHUTDOWN_BOTH => Shutdown::Both, _ => return Err(Error::InvalidArgument) };
        unsafe { borrow::<Socket>(socket) }?.inner.shutdown(how).map_err(error)
    }
    unsafe fn local_address(socket: *mut c_void) -> Result<Address> { let socket = unsafe { borrow::<Socket>(socket) }?; endpoint(socket, socket.inner.local_addr()) }
    unsafe fn peer_address(socket: *mut c_void) -> Result<Address> { let socket = unsafe { borrow::<Socket>(socket) }?; endpoint(socket, socket.inner.peer_addr()) }
    unsafe fn set_blocking(socket: *mut c_void, blocking: bool) -> Result<()> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        socket.inner.set_nonblocking(!blocking).map_err(error)?;
        socket.blocking.store(blocking, Ordering::Relaxed);
        Ok(())
    }
    unsafe fn get_option(socket: *mut c_void, option: u32) -> Result<u64> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        socket.has(option)?;
        let s = &socket.inner;
        let flag = |value: io::Result<bool>| value.map(u64::from);
        let timeout = |value: io::Result<Option<Duration>>| value.map(|t| t.map_or(0, |t| t.as_nanos().div_ceil(1_000_000) as u64));
        match option {
            sockets::REUSE_ADDRESS => flag(s.reuse_address()), sockets::NO_DELAY => flag(s.nodelay()), sockets::KEEP_ALIVE => flag(s.keepalive()),
            sockets::BROADCAST => flag(s.broadcast()), sockets::IPV6_ONLY => flag(s.only_v6()),
            sockets::RECEIVE_BUFFER => s.recv_buffer_size().map(|size| size as u64), sockets::SEND_BUFFER => s.send_buffer_size().map(|size| size as u64),
            sockets::LINGER => s.linger().map(|l| l.map_or(0, |l| l.as_secs() + 1)),
            sockets::RECEIVE_TIMEOUT => timeout(s.read_timeout()), sockets::SEND_TIMEOUT => timeout(s.write_timeout()),
            // Reading the pending error clears it; it travels as the status the failing call would have reported.
            sockets::ERROR => s.take_error().map(|e| e.map_or(dotnet_pal_rs::OK, |e| { tuning::forget_errors(s); error(e).status() }) as u64),
            sockets::AVAILABLE => ready::available(s),
            sockets::KEEP_ALIVE_IDLE => tuning::get(s, tuning::TCP, tuning::IDLE).map(|v| v.max(0) as u64),
            sockets::KEEP_ALIVE_INTERVAL => tuning::get(s, tuning::TCP, tuning::INTERVAL).map(|v| v.max(0) as u64),
            sockets::KEEP_ALIVE_COUNT => tuning::get(s, tuning::TCP, tuning::COUNT).map(|v| v.max(0) as u64),
            sockets::HOPS => if socket.v6 { tuning::hops_v6(s).map(|v| v as u64) } else { s.ttl().map(u64::from) },
            sockets::MULTICAST_HOPS => if socket.v6 { s.multicast_hops_v6() } else { s.multicast_ttl_v4() }.map(u64::from),
            sockets::MULTICAST_LOOPBACK => flag(if socket.v6 { s.multicast_loop_v6() } else { s.multicast_loop_v4() }),
            sockets::MULTICAST_INTERFACE => if socket.v6 { s.multicast_if_v6() } else { tuning::interface_v4(s, &socket.multicast_interface) }.map(u64::from),
            sockets::PACKET_INFORMATION => flag(tuning::reports_packets(s, socket.v6)), sockets::DONT_FRAGMENT => flag(tuning::dont_fragment(s)),
            sockets::RECEIVE_ERRORS => flag(tuning::receives_errors(s, socket.v6)),
            _ => return Err(Error::InvalidArgument),
        }.map_err(error)
    }
    unsafe fn set_option(socket: *mut c_void, option: u32, value: u64) -> Result<()> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        socket.has(option)?;
        // The OS clamps sizes and linger times itself; only the conversion to its integer saturates here.
        let (s, on, size) = (&socket.inner, value != 0, value.min(i32::MAX as u64) as usize);
        let timeout = (value != 0).then(|| Duration::from_millis(value));
        match option {
            sockets::REUSE_ADDRESS => s.set_reuse_address(on), sockets::NO_DELAY => s.set_nodelay(on), sockets::KEEP_ALIVE => s.set_keepalive(on),
            sockets::BROADCAST => s.set_broadcast(on), sockets::IPV6_ONLY => s.set_only_v6(on),
            sockets::RECEIVE_BUFFER => s.set_recv_buffer_size(size), sockets::SEND_BUFFER => s.set_send_buffer_size(size),
            // Winsock keeps the seconds in sixteen bits.
            sockets::LINGER => s.set_linger(on.then(|| Duration::from_secs((value - 1).min(if cfg!(windows) { u16::MAX as u64 } else { i32::MAX as u64 })))),
            sockets::RECEIVE_TIMEOUT => s.set_read_timeout(timeout), sockets::SEND_TIMEOUT => s.set_write_timeout(timeout),
            // No time and no count: Linux refuses a 0, macOS answers OK and keeps its default (measured). One answer for both.
            sockets::KEEP_ALIVE_IDLE | sockets::KEEP_ALIVE_INTERVAL | sockets::KEEP_ALIVE_COUNT if value == 0 => return Err(Error::InvalidArgument),
            sockets::KEEP_ALIVE_IDLE => tuning::set(s, tuning::TCP, tuning::IDLE, &(size as i32)),
            sockets::KEEP_ALIVE_INTERVAL => tuning::set(s, tuning::TCP, tuning::INTERVAL, &(size as i32)),
            sockets::KEEP_ALIVE_COUNT => tuning::set(s, tuning::TCP, tuning::COUNT, &(size as i32)),
            sockets::HOPS => if socket.v6 { s.set_unicast_hops_v6(size as u32) } else { s.set_ttl(size as u32) },
            sockets::MULTICAST_HOPS => if socket.v6 { s.set_multicast_hops_v6(size as u32) } else { s.set_multicast_ttl_v4(size as u32) },
            sockets::MULTICAST_LOOPBACK => if socket.v6 { s.set_multicast_loop_v6(on) } else { s.set_multicast_loop_v4(on) },
            // The front end holds `value` to 32 bits.
            sockets::MULTICAST_INTERFACE if value != 0 && !tuning::exists(value as u32) => return Err(Error::AddressNotAvailable),
            sockets::MULTICAST_INTERFACE => if socket.v6 { s.set_multicast_if_v6(value as u32) } else { tuning::set_interface_v4(s, value as u32, &socket.multicast_interface) },
            sockets::PACKET_INFORMATION => tuning::set_reports_packets(s, socket.v6, on), sockets::DONT_FRAGMENT => tuning::set_dont_fragment(s, on),
            sockets::RECEIVE_ERRORS => tuning::set_receives_errors(s, socket.v6, socket.raw, on),
            _ => return Err(Error::InvalidArgument),
        }.map_err(error)
    }
    unsafe fn poll(entries: &mut [PollEntry], timeout_ns: u64, channel: Option<u32>) -> Result<()> {
        let waker = channel.map(waker).transpose()?;
        // One slot per distinct socket: macOS answers only the last slot that names a descriptor.
        let (mut set, mut slots, mut seen) = (Vec::with_capacity(entries.len() + 1), Vec::with_capacity(entries.len()), HashMap::new());
        for entry in entries.iter() {
            let raw = ready::raw(&unsafe { borrow::<Socket>(entry.socket) }?.inner);
            let slot = *seen.entry(raw).or_insert_with(|| { set.push(ready::slot(raw, ready::ALWAYS)); set.len() - 1 });
            set[slot].events |= ready::events(entry.requested);
            slots.push(slot);
        }
        let sockets = set.len();
        if let Some(waker) = waker { set.push(ready::slot(ready::raw(&waker.wait), ready::events(POLL_READ))); }
        let deadline = if timeout_ns == INFINITE { None } else { Instant::now().checked_add(Duration::from_nanos(timeout_ns)) };
        loop {
            // The OS counts milliseconds: a wait is rounded up, and one that ends early resumes against the deadline.
            let limit = deadline.map_or(-1, |d| d.saturating_duration_since(Instant::now()).as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32);
            for slot in set.iter_mut() { slot.revents = 0; }
            match ready::wait(&mut set, limit) { Err(e) if e.kind() == io::ErrorKind::Interrupted => continue, other => other.map_err(error)? }
            let mut done = false;
            if let Some(waker) = waker.filter(|_| set[sockets].revents != 0) {
                // Every wake so far goes at once. One that arrives after this stays for the channel's next poll.
                waker.drain();
                done = true;
            }
            let states: Vec<u32> = set[..sockets].iter().map(|slot| if slot.revents == 0 { 0 } else { ready::state(slot) }).collect();
            for (entry, slot) in entries.iter_mut().zip(&slots) {
                let state = states[*slot];
                let hangup = if state & PEER_CLOSED != 0 && entry.requested & POLL_READ != 0 { POLL_HANGUP } else { 0 };
                entry.triggered = (state & (entry.requested | POLL_ERROR | POLL_HANGUP)) | hangup;
                done |= entry.triggered != 0;
            }
            if done || deadline.is_some_and(|d| Instant::now() >= d) { return Ok(()); }
            // A wait that ended with nothing to report goes on. On macOS that is a peer's half close on a socket
            // no entry reads: it would end every further wait of this call at once, so it is no longer asked about.
            for (slot, state) in set.iter_mut().zip(&states) { if state & PEER_CLOSED != 0 { slot.events &= !ready::ALWAYS; } }
        }
    }
    fn wake(channel: u32) -> Result<()> {
        let waker = waker(channel)?;
        // A queue that takes no more is already a pending wake.
        match retry(|| waker.signal()) { Err(e) if e.kind() != io::ErrorKind::WouldBlock => Err(error(e)), _ => Ok(()) }
    }
    unsafe fn resolve(name: &[u8], family: u32, out: *mut Address, capacity: usize) -> Result<usize> { unsafe { lookup(name, family, out, capacity) } }
    unsafe fn host_name(out: *mut u8, capacity: usize) -> Result<usize> {
        let name = host()?;
        let needed = name.len() + 1;
        if needed <= capacity { unsafe { ptr::copy_nonoverlapping(name.as_ptr(), out, name.len()); out.add(name.len()).write(0); } }
        Ok(needed)
    }
}
