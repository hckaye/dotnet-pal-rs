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
//! Names are resolved with getaddrinfo itself on Unix: std turns its failure code
//! into text, which cannot tell a name without addresses from an answer that could
//! not be obtained. On Windows std keeps the Winsock code, so `ToSocketAddrs` is
//! enough. The Windows branches compile but have not been executed here.
use super::{borrow, boxed, take, Std};
use dotnet_pal_rs::kernel::INFINITE;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::sockets::{self, Address, PollEntry, DATAGRAM, IPV4, IPV6, POLL_ERROR, POLL_HANGUP, POLL_READ, POLL_WRITE, RECEIVE_PEEK, STREAM};
use socket2::{Domain, MaybeUninitSlice, SockAddr, Type};
use std::{collections::HashMap, ffi::c_void, io, mem::MaybeUninit, net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV6}, ptr};
use std::{sync::{atomic::{AtomicBool, AtomicU32, Ordering}, OnceLock}, time::{Duration, Instant}};

/// The network group's membership call takes these handles, hence the visibility.
pub(crate) struct Socket { pub(crate) inner: socket2::Socket, blocking: AtomicBool, pub(crate) stream: bool, pub(crate) v6: bool, multicast_interface: AtomicU32 }
impl Socket {
    fn wrap(inner: socket2::Socket, stream: bool, v6: bool) -> *mut c_void {
        boxed(Self { inner, blocking: AtomicBool::new(true), stream, v6, multicast_interface: AtomicU32::new(0) })
    }
    /// `Err(Unsupported)` for an option the socket's protocol does not have, before the OS is asked.
    fn has(&self, option: u32) -> Result<()> {
        let missing = match option {
            sockets::NO_DELAY | sockets::KEEP_ALIVE_IDLE | sockets::KEEP_ALIVE_INTERVAL | sockets::KEEP_ALIVE_COUNT => !self.stream,
            sockets::MULTICAST_HOPS | sockets::MULTICAST_LOOPBACK | sockets::MULTICAST_INTERFACE => self.stream,
            sockets::IPV6_ONLY => !self.v6,
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
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        return match code {
            libc::EINPROGRESS => Error::InProgress, libc::EPIPE => Error::BrokenPipe,
            libc::ECONNREFUSED => Error::ConnectionRefused, libc::ECONNRESET => Error::ConnectionReset,
            libc::ECONNABORTED => Error::ConnectionAborted, libc::ENOTCONN => Error::NotConnected, libc::EISCONN => Error::AlreadyConnected,
            libc::EADDRINUSE => Error::AddressInUse, libc::EADDRNOTAVAIL => Error::AddressNotAvailable,
            libc::ENETUNREACH | libc::ENETDOWN => Error::NetworkUnreachable, libc::EHOSTUNREACH => Error::HostUnreachable,
            libc::EMFILE | libc::ENFILE => Error::TooManyHandles, libc::EMSGSIZE => Error::MessageTooLarge,
            libc::EACCES | libc::EPERM => Error::AccessDenied, libc::ETIMEDOUT => Error::Timeout,
            libc::ENOMEM | libc::ENOBUFS => Error::OutOfMemory, libc::EINVAL => Error::InvalidArgument,
            libc::EAFNOSUPPORT | libc::EPROTONOSUPPORT | libc::EOPNOTSUPP | libc::ENOPROTOOPT => Error::Unsupported,
            // One value on Linux and macOS, two on some other Unix.
            code if code == libc::EAGAIN || code == libc::EWOULDBLOCK => Error::WouldBlock,
            _ => Error::Os,
        };
    }
    #[cfg(windows)]
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
fn waited(socket: &Socket, e: io::Error) -> Error {
    match error(e) { Error::WouldBlock if socket.blocking.load(Ordering::Relaxed) => Error::Timeout, other => other }
}
fn retry<T>(mut call: impl FnMut() -> io::Result<T>) -> io::Result<T> {
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
fn endpoint(address: io::Result<SockAddr>) -> Result<Address> { address.map_err(error)?.as_socket().map(boundary).ok_or(Error::Os) }

#[cfg(unix)]
mod ready {
    use super::{PEER_CLOSED, POLL_ERROR, POLL_HANGUP, POLL_READ, POLL_WRITE};
    use std::{io, os::fd::{AsRawFd, RawFd}, ptr};
    pub type Slot = libc::pollfd;
    pub type Raw = RawFd;
    pub const PEEK: i32 = libc::MSG_PEEK;
    #[cfg(not(target_vendor = "apple"))]
    pub const SEND: i32 = libc::MSG_NOSIGNAL;
    #[cfg(target_vendor = "apple")]
    pub const SEND: i32 = 0;
    /// Asked of every socket whatever its entries request: macOS reports the end of
    /// the receiving direction to a poll that names POLLHUP, and nothing to one that names no event.
    #[cfg(target_vendor = "apple")]
    pub const ALWAYS: i16 = libc::POLLHUP;
    #[cfg(not(target_vendor = "apple"))]
    pub const ALWAYS: i16 = 0;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const READ: i16 = libc::POLLIN | libc::POLLRDHUP;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const READ: i16 = libc::POLLIN;

    pub fn raw(socket: &impl AsRawFd) -> Raw { socket.as_raw_fd() }
    pub fn slot(fd: Raw, events: i16) -> Slot { libc::pollfd { fd, events, revents: 0 } }
    pub fn events(requested: u32) -> i16 { (if requested & POLL_READ != 0 { READ } else { 0 }) | (if requested & POLL_WRITE != 0 { libc::POLLOUT } else { 0 }) }
    pub fn wait(set: &mut [Slot], timeout_ms: i32) -> io::Result<()> {
        if unsafe { libc::poll(set.as_mut_ptr(), set.len() as libc::nfds_t, timeout_ms) } < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
    /// Waits without limit for a connect that a signal interrupted to settle.
    pub fn settled(socket: &impl AsRawFd) -> io::Result<()> {
        let mut slot = [slot(socket.as_raw_fd(), libc::POLLOUT)];
        loop { match wait(&mut slot, -1) { Err(e) if e.kind() == io::ErrorKind::Interrupted => continue, other => return other } }
    }
    fn bit(set: bool, value: u32) -> u32 { if set { value } else { 0 } }
    /// What a slot with events says about its socket, in `POLL_*` bits and `PEER_CLOSED`.
    pub fn state(slot: &Slot) -> u32 {
        let r = slot.revents;
        let state = bit(r & libc::POLLIN != 0, POLL_READ) | bit(r & libc::POLLOUT != 0, POLL_WRITE) | bit(r & (libc::POLLERR | libc::POLLNVAL) != 0, POLL_ERROR);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { state | bit(r & libc::POLLHUP != 0, POLL_HANGUP) | bit(r & libc::POLLRDHUP != 0, PEER_CLOSED) }
        #[cfg(target_vendor = "apple")]
        {
            if r & libc::POLLHUP == 0 { return state; }
            // Each direction answers for itself when it is asked alone. A send on a closed direction does
            // not wait either, so it counts as writable, as it does on Linux.
            let ask = |events| { let mut one = [self::slot(slot.fd, events)]; if wait(&mut one, 0).is_ok() { one[0].revents } else { 0 } };
            let (read, write) = (ask(libc::POLLHUP), ask(libc::POLLOUT));
            // A receive that only peeks reports the pending error and leaves it for the ERROR option (Linux would clear it).
            let mut byte = 0u8;
            let failed = unsafe { libc::recv(slot.fd, ptr::addr_of_mut!(byte).cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) } < 0
                && !matches!(io::Error::last_os_error().raw_os_error(), Some(libc::EAGAIN | libc::EINTR | libc::ENOTCONN));
            state | bit(read & libc::POLLHUP != 0, PEER_CLOSED) | bit(read & write & libc::POLLHUP != 0, POLL_HANGUP)
                | bit(write & (libc::POLLOUT | libc::POLLHUP) != 0, POLL_WRITE) | bit(failed, POLL_ERROR)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        { let _ = PEER_CLOSED; state | bit(r & libc::POLLHUP != 0, POLL_HANGUP) }
    }
    pub fn available(socket: &impl AsRawFd) -> io::Result<u64> {
        let mut value: libc::c_int = 0;
        // FIONREAD counts the address records of queued datagrams on macOS (66 for datagrams of 8 and 10 bytes).
        // SO_NREAD is the next datagram, or the bytes of a stream: what FIONREAD is on Linux.
        #[cfg(target_vendor = "apple")]
        let rc = {
            let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            unsafe { libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NREAD, ptr::addr_of_mut!(value).cast(), &mut length) }
        };
        #[cfg(not(target_vendor = "apple"))]
        let rc = unsafe { libc::ioctl(socket.as_raw_fd(), libc::FIONREAD, ptr::addr_of_mut!(value)) };
        if rc != 0 { return Err(io::Error::last_os_error()); }
        Ok(value.max(0) as u64)
    }
}

#[cfg(windows)]
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

#[cfg(unix)]
pub(crate) mod tuning {
    use std::{io, mem::size_of, os::fd::AsRawFd, ptr, sync::atomic::AtomicU32};
    pub const TCP: i32 = libc::IPPROTO_TCP;
    #[cfg(target_vendor = "apple")]
    pub const IDLE: i32 = libc::TCP_KEEPALIVE;
    #[cfg(not(target_vendor = "apple"))]
    pub const IDLE: i32 = libc::TCP_KEEPIDLE;
    pub const INTERVAL: i32 = libc::TCP_KEEPINTVL;
    pub const COUNT: i32 = libc::TCP_KEEPCNT;
    /// <netinet/in.h> of the macOS SDK; `libc` 0.2.169 does not carry it.
    #[cfg(target_vendor = "apple")]
    const IP_MULTICAST_IFINDEX: i32 = 66;

    pub fn get(socket: &impl AsRawFd, level: i32, name: i32) -> io::Result<i32> {
        let (mut value, mut length) = (0i32, size_of::<i32>() as libc::socklen_t);
        if unsafe { libc::getsockopt(socket.as_raw_fd(), level, name, ptr::addr_of_mut!(value).cast(), &mut length) } != 0 { return Err(io::Error::last_os_error()); }
        Ok(value)
    }
    pub fn set<T>(socket: &impl AsRawFd, level: i32, name: i32, value: &T) -> io::Result<()> {
        if unsafe { libc::setsockopt(socket.as_raw_fd(), level, name, (value as *const T).cast(), size_of::<T>() as libc::socklen_t) } != 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    }
    /// Whether an interface has this index. No such interface is EADDRNOTAVAIL or ENODEV on Linux and ENXIO or EINVAL on macOS
    /// (measured), and macOS joins a group on an interface of its own choice when the index names none: the answer is made here.
    pub fn exists(index: u32) -> bool {
        let mut name = [0 as libc::c_char; libc::IF_NAMESIZE + 1];
        !unsafe { libc::if_indextoname(index, name.as_mut_ptr()) }.is_null()
    }
    /// An IPv6 hop limit nobody set reads as -1 on macOS: the limit in force is the system's default.
    pub fn hops_v6(socket: &impl AsRawFd) -> io::Result<i32> {
        let value = get(socket, libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS)?;
        if value >= 0 { return Ok(value); }
        #[cfg(target_vendor = "apple")]
        {
            let (mut limit, mut size) = (0i32, size_of::<i32>());
            if unsafe { libc::sysctlbyname(c"net.inet6.ip6.hlim".as_ptr(), ptr::addr_of_mut!(limit).cast(), &mut size, ptr::null_mut(), 0) } == 0 { return Ok(limit); }
        }
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn interface_v4(_: &impl AsRawFd, remembered: &AtomicU32) -> io::Result<u32> { Ok(remembered.load(std::sync::atomic::Ordering::Acquire)) }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn set_interface_v4(socket: &impl AsRawFd, index: u32, remembered: &AtomicU32) -> io::Result<()> {
        let by_index = libc::ip_mreqn { imr_multiaddr: libc::in_addr { s_addr: 0 }, imr_address: libc::in_addr { s_addr: 0 }, imr_ifindex: index.min(i32::MAX as u32) as i32 };
        set(socket, libc::IPPROTO_IP, libc::IP_MULTICAST_IF, &by_index)?;
        remembered.store(index, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    #[cfg(target_vendor = "apple")]
    pub fn interface_v4(socket: &impl AsRawFd, _: &AtomicU32) -> io::Result<u32> { get(socket, libc::IPPROTO_IP, IP_MULTICAST_IFINDEX).map(|index| index.max(0) as u32) }
    #[cfg(target_vendor = "apple")]
    pub fn set_interface_v4(socket: &impl AsRawFd, index: u32, _: &AtomicU32) -> io::Result<()> { set(socket, libc::IPPROTO_IP, IP_MULTICAST_IFINDEX, &(index.min(i32::MAX as u32) as i32)) }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    pub fn interface_v4(_: &impl AsRawFd, _: &AtomicU32) -> io::Result<u32> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    pub fn set_interface_v4(_: &impl AsRawFd, _: u32, _: &AtomicU32) -> io::Result<()> { Err(io::Error::from(io::ErrorKind::Unsupported)) }
}
#[cfg(windows)]
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
}

/// One wake channel: `wait` is readable from a `signal` until the channel's poll
/// drains it, which is what makes a wake with no poll in progress reach the
/// channel's next poll and no other channel's. Both ends are non-blocking.
#[cfg(unix)]
struct Waker { wait: std::os::unix::net::UnixStream, wake: std::os::unix::net::UnixStream }
#[cfg(unix)]
impl Waker {
    fn new() -> io::Result<Self> {
        let (wait, wake) = std::os::unix::net::UnixStream::pair()?;
        wait.set_nonblocking(true)?;
        wake.set_nonblocking(true)?;
        Ok(Self { wait, wake })
    }
    fn signal(&self) -> io::Result<usize> { io::Write::write(&mut &self.wake, &[1]) }
    fn drain(&self) { let mut sink = [0u8; 64]; while io::Read::read(&mut &self.wait, &mut sink).is_ok_and(|n| n != 0) {} }
}
/// `WSAPoll` waits on sockets only, so the channel is a loopback UDP socket connected to
/// itself, which takes datagrams from its own address alone.
#[cfg(not(unix))]
struct Waker { wait: std::net::UdpSocket }
#[cfg(not(unix))]
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
#[cfg(unix)]
unsafe fn lookup(name: &[u8], family: u32, out: *mut Address, capacity: usize) -> Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    const NO_DATA: i32 = libc::EAI_NODATA;
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    const NO_DATA: i32 = libc::EAI_NONAME;
    let node = std::ffi::CString::new(name).map_err(|_| Error::InvalidArgument)?;
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = match family { 0 => libc::AF_UNSPEC, IPV4 => libc::AF_INET, IPV6 => libc::AF_INET6, _ => return Err(Error::InvalidArgument) };
    // One socket type, so an address is listed once rather than once per type.
    hints.ai_socktype = libc::SOCK_STREAM;
    let mut results: *mut libc::addrinfo = ptr::null_mut();
    let code = loop {
        let code = unsafe { libc::getaddrinfo(node.as_ptr(), ptr::null(), &hints, &mut results) };
        if code != libc::EAI_SYSTEM || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted { break code; }
    };
    match code {
        0 => {}
        libc::EAI_AGAIN => return Err(Error::Timeout),
        libc::EAI_MEMORY => return Err(Error::OutOfMemory),
        libc::EAI_FAMILY => return Err(Error::Unsupported),
        libc::EAI_SYSTEM => return Err(error(io::Error::last_os_error())),
        code if code == libc::EAI_NONAME || code == NO_DATA => return Err(Error::NotFound),
        _ => return Err(Error::Os),
    }
    let (mut found, mut next) = (0, results);
    while !next.is_null() && found < capacity {
        let info = unsafe { &*next };
        next = info.ai_next;
        if info.ai_addr.is_null() { continue; }
        match info.ai_family {
            libc::AF_INET if info.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in>() => {
                let v4 = unsafe { info.ai_addr.cast::<libc::sockaddr_in>().read_unaligned() };
                unsafe { list(out, &mut found, Address::v4(v4.sin_addr.s_addr.to_ne_bytes(), 0)) };
            }
            libc::AF_INET6 if info.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in6>() => {
                let v6 = unsafe { info.ai_addr.cast::<libc::sockaddr_in6>().read_unaligned() };
                unsafe { list(out, &mut found, Address::v6(v6.sin6_addr.s6_addr, 0, v6.sin6_scope_id)) };
            }
            _ => {}
        }
    }
    unsafe { libc::freeaddrinfo(results) };
    Ok(found)
}
#[cfg(not(unix))]
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
#[cfg(unix)]
fn host() -> Result<Vec<u8>> {
    // One byte past what gethostname may fill keeps the text terminated when the name is truncated.
    let mut name = [0u8; sockets::MAX_HOST_NAME + 2];
    if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len() - 1) } != 0 { return Err(error(io::Error::last_os_error())); }
    let length = name.iter().position(|b| *b == 0).unwrap_or(name.len() - 1);
    Ok(name[..length].to_vec())
}
#[cfg(windows)]
fn host() -> Result<Vec<u8>> {
    use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    let (mut wide, mut length) = ([0u16; sockets::MAX_HOST_NAME + 1], sockets::MAX_HOST_NAME as u32 + 1);
    if unsafe { GetComputerNameExW(ComputerNameDnsHostname, wide.as_mut_ptr(), &mut length) } == 0 { return Err(Error::Os); }
    String::from_utf16(&wide[..length as usize]).map(String::into_bytes).map_err(|_| Error::Os)
}

impl port::Sockets for Std {
    unsafe fn create(family: u32, kind: u32) -> Result<*mut c_void> {
        let domain = match family { IPV4 => Domain::IPV4, IPV6 => Domain::IPV6, _ => return Err(Error::InvalidArgument) };
        let shape = match kind { STREAM => Type::STREAM, DATAGRAM => Type::DGRAM, _ => return Err(Error::InvalidArgument) };
        Ok(Socket::wrap(socket2::Socket::new(domain, shape, None).map_err(error)?, kind == STREAM, family == IPV6))
    }
    unsafe fn close(socket: *mut c_void) -> Result<()> { drop(unsafe { take::<Socket>(socket) }?); Ok(()) }
    unsafe fn bind(socket: *mut c_void, address: &Address) -> Result<()> { unsafe { borrow::<Socket>(socket) }?.inner.bind(&native(address)?).map_err(error) }
    unsafe fn listen(socket: *mut c_void, backlog: u32) -> Result<()> {
        unsafe { borrow::<Socket>(socket) }?.inner.listen(backlog.min(i32::MAX as u32) as i32).map_err(error)
    }
    unsafe fn accept(socket: *mut c_void) -> Result<(*mut c_void, Address)> {
        let listener = unsafe { borrow::<Socket>(socket) }?;
        let (inner, peer) = retry(|| listener.inner.accept()).map_err(|e| waited(listener, e))?;
        // Outside Linux an accepted socket inherits the listener's non-blocking mode; the contract starts it blocking.
        if !listener.blocking.load(Ordering::Relaxed) { inner.set_nonblocking(false).map_err(error)?; }
        Ok((Socket::wrap(inner, listener.stream, listener.v6), peer.as_socket().map(boundary).unwrap_or_default()))
    }
    unsafe fn connect(socket: *mut c_void, address: &Address) -> Result<()> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        match socket.inner.connect(&native(address)?) {
            Ok(()) => Ok(()),
            // An interrupted connect carries on in the OS; a second call would report EALREADY. Wait for its outcome instead.
            #[cfg(unix)]
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                ready::settled(&socket.inner).map_err(error)?;
                socket.inner.take_error().map_err(error)?.map_or(Ok(()), |e| Err(error(e)))
            }
            // Winsock reports a connect in progress with the code every other call uses for "not now".
            Err(e) => Err(match error(e) { Error::WouldBlock if cfg!(windows) => Error::InProgress, other => other }),
        }
    }
    unsafe fn send(socket: *mut c_void, data: *const u8, size: usize, to: Option<&Address>) -> Result<usize> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        let (bytes, to) = (unsafe { std::slice::from_raw_parts(data, size) }, to.map(native).transpose()?);
        retry(|| match &to { Some(to) => socket.inner.send_to_with_flags(bytes, to, ready::SEND), None => socket.inner.send_with_flags(bytes, ready::SEND) })
            .map_err(|e| waited(socket, e))
    }
    unsafe fn receive(socket: *mut c_void, out: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<Address>)> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        let flags = if flags & RECEIVE_PEEK != 0 { ready::PEEK } else { 0 };
        // The vectored call is the one `socket2` lets end in a truncated datagram on Windows, where Winsock calls that an error.
        let mut parts = [MaybeUninitSlice::new(unsafe { std::slice::from_raw_parts_mut(out.cast::<MaybeUninit<u8>>(), capacity) })];
        let (done, _, sender) = retry(|| socket.inner.recv_from_vectored_with_flags(&mut parts, flags)).map_err(|e| waited(socket, e))?;
        // A stream names no sender: the OS leaves the address empty and the answer is `None`.
        Ok((done, sender.as_socket().map(boundary)))
    }
    unsafe fn shutdown(socket: *mut c_void, how: u32) -> Result<()> {
        let how = match how { sockets::SHUTDOWN_READ => Shutdown::Read, sockets::SHUTDOWN_WRITE => Shutdown::Write, sockets::SHUTDOWN_BOTH => Shutdown::Both, _ => return Err(Error::InvalidArgument) };
        unsafe { borrow::<Socket>(socket) }?.inner.shutdown(how).map_err(error)
    }
    unsafe fn local_address(socket: *mut c_void) -> Result<Address> { endpoint(unsafe { borrow::<Socket>(socket) }?.inner.local_addr()) }
    unsafe fn peer_address(socket: *mut c_void) -> Result<Address> { endpoint(unsafe { borrow::<Socket>(socket) }?.inner.peer_addr()) }
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
            sockets::ERROR => s.take_error().map(|e| e.map_or(dotnet_pal_rs::OK, |e| error(e).status()) as u64),
            sockets::AVAILABLE => ready::available(s),
            sockets::KEEP_ALIVE_IDLE => tuning::get(s, tuning::TCP, tuning::IDLE).map(|v| v.max(0) as u64),
            sockets::KEEP_ALIVE_INTERVAL => tuning::get(s, tuning::TCP, tuning::INTERVAL).map(|v| v.max(0) as u64),
            sockets::KEEP_ALIVE_COUNT => tuning::get(s, tuning::TCP, tuning::COUNT).map(|v| v.max(0) as u64),
            sockets::HOPS => if socket.v6 { tuning::hops_v6(s).map(|v| v as u64) } else { s.ttl().map(u64::from) },
            sockets::MULTICAST_HOPS => if socket.v6 { s.multicast_hops_v6() } else { s.multicast_ttl_v4() }.map(u64::from),
            sockets::MULTICAST_LOOPBACK => flag(if socket.v6 { s.multicast_loop_v6() } else { s.multicast_loop_v4() }),
            sockets::MULTICAST_INTERFACE => if socket.v6 { s.multicast_if_v6() } else { tuning::interface_v4(s, &socket.multicast_interface) }.map(u64::from),
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
