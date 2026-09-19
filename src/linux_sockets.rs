//! Linux provider for the sockets group: BSD sockets with close-on-exec
//! descriptors, ppoll(2) readiness and one eventfd per wake channel. No Rust
//! heap; a handle is a CRT allocation holding the descriptor, so descriptor 0
//! is as valid as any other, and the one setting the kernel does not read back:
//! getsockopt(IP_MULTICAST_IF) answers with an address (4 bytes, measured on
//! Linux 6.12), never with the index the option was set from. The handle also
//! says whether the socket is a Unix domain socket and whether it is a stream:
//! the calls that take an IP address refuse a local socket before the kernel
//! sees the address, and the sender of what a local socket receives is answered
//! from the handle, because the kernel names it by whether it has a path.
use crate::kernel::INFINITE;
use crate::linux::Linux;
use crate::port::{self, Error, Result};
use crate::sockets::{self, Address, PollEntry, DATAGRAM, IPV4, IPV6, LOCAL, MAX_HOST_NAME, POLL_CHANNELS, POLL_ERROR, POLL_HANGUP, POLL_READ, POLL_WRITE, RECEIVE_PEEK, STREAM};
use core::{ffi::c_void, mem, ptr, sync::atomic::{AtomicI32, AtomicU32, Ordering}};

#[repr(C)]
struct Socket { fd: i32, local: bool, stream: bool, multicast_interface: AtomicU32 }
/// Polls of at most this many descriptors (the entries plus the channel's eventfd) stay on the stack.
const INLINE_POLL: usize = 64;

pub(crate) fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// The boundary's name for an errno value. EWOULDBLOCK is EAGAIN on Linux.
fn error(code: i32) -> Error {
    match code {
        libc::EAGAIN => Error::WouldBlock, libc::EINPROGRESS => Error::InProgress, libc::EPIPE => Error::BrokenPipe,
        libc::ECONNREFUSED => Error::ConnectionRefused, libc::ECONNRESET => Error::ConnectionReset,
        libc::ECONNABORTED => Error::ConnectionAborted, libc::ENOTCONN => Error::NotConnected, libc::EISCONN => Error::AlreadyConnected,
        libc::EADDRINUSE => Error::AddressInUse, libc::EADDRNOTAVAIL => Error::AddressNotAvailable,
        libc::ENETUNREACH | libc::ENETDOWN => Error::NetworkUnreachable, libc::EHOSTUNREACH => Error::HostUnreachable,
        libc::EMFILE | libc::ENFILE => Error::TooManyHandles, libc::EMSGSIZE => Error::MessageTooLarge,
        libc::EACCES | libc::EPERM => Error::AccessDenied, libc::ETIMEDOUT => Error::Timeout,
        libc::ENOMEM | libc::ENOBUFS => Error::OutOfMemory, libc::EINVAL => Error::InvalidArgument,
        libc::EAFNOSUPPORT | libc::EPROTONOSUPPORT | libc::EOPNOTSUPP | libc::ENOPROTOOPT => Error::Unsupported,
        _ => Error::Os,
    }
}
fn failed<T>() -> Result<T> { Err(error(errno())) }
/// The failure of a transfer or an accept. The kernel reports an expired SO_RCVTIMEO or SO_SNDTIMEO
/// as EAGAIN, the error a non-blocking socket uses for "not now"; the descriptor's mode tells them apart.
pub(crate) fn waited<T>(fd: i32) -> Result<T> {
    let code = errno();
    if code == libc::EAGAIN && unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK == 0 { return Err(Error::Timeout); }
    Err(error(code))
}
pub(crate) unsafe fn descriptor(socket: *mut c_void) -> Result<i32> {
    if socket.is_null() || !(socket as usize).is_multiple_of(mem::align_of::<Socket>()) { return Err(Error::InvalidArgument); }
    Ok(unsafe { (*socket.cast::<Socket>()).fd })
}
/// The descriptor of a Unix domain socket and whether it is a stream; `Err(InvalidArgument)` for a socket of another family.
pub(crate) unsafe fn local(socket: *mut c_void) -> Result<(i32, bool)> {
    let fd = unsafe { descriptor(socket) }?;
    // SAFETY: `descriptor` has checked the handle, which `wrap` made.
    let socket = unsafe { &*socket.cast::<Socket>() };
    if socket.local { Ok((fd, socket.stream)) } else { Err(Error::InvalidArgument) }
}
/// The descriptor of a socket that takes IP addresses; a local socket has none to bind, reach or send to.
unsafe fn internet(socket: *mut c_void) -> Result<i32> {
    let fd = unsafe { descriptor(socket) }?;
    if unsafe { (*socket.cast::<Socket>()).local } { Err(Error::InvalidArgument) } else { Ok(fd) }
}
/// `(is a stream, the domain)` of a descriptor: its protocol decides which options and which group calls it has.
pub(crate) fn shape(fd: i32) -> Result<(bool, i32)> {
    Ok((get::<i32>(fd, libc::SOL_SOCKET, libc::SO_TYPE)? == libc::SOCK_STREAM, get::<i32>(fd, libc::SOL_SOCKET, libc::SO_DOMAIN)?))
}
/// Wraps a descriptor this provider owns; the descriptor is closed when no handle can be made.
fn wrap(fd: i32, local: bool, stream: bool) -> Result<*mut c_void> {
    let p = unsafe { libc::calloc(1, mem::size_of::<Socket>()) }.cast::<Socket>();
    if p.is_null() { unsafe { libc::close(fd) }; return Err(Error::OutOfMemory); }
    unsafe { (*p).fd = fd; (*p).local = local; (*p).stream = stream; }
    Ok(p.cast())
}

/// A native address and the length the kernel expects for its family.
pub(crate) fn native(address: &Address) -> Result<(libc::sockaddr_storage, libc::socklen_t)> {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let raw = ptr::addr_of_mut!(storage);
    match address.family as u32 {
        IPV4 => {
            let a = address.address;
            let v4 = libc::sockaddr_in { sin_family: libc::AF_INET as libc::sa_family_t, sin_port: address.port.to_be(),
                sin_addr: libc::in_addr { s_addr: u32::from_ne_bytes([a[0], a[1], a[2], a[3]]) }, sin_zero: [0; 8] };
            unsafe { raw.cast::<libc::sockaddr_in>().write(v4) };
            Ok((storage, mem::size_of::<libc::sockaddr_in>() as libc::socklen_t))
        }
        IPV6 => {
            let v6 = libc::sockaddr_in6 { sin6_family: libc::AF_INET6 as libc::sa_family_t, sin6_port: address.port.to_be(), sin6_flowinfo: 0,
                sin6_addr: libc::in6_addr { s6_addr: address.address }, sin6_scope_id: address.scope };
            unsafe { raw.cast::<libc::sockaddr_in6>().write(v6) };
            Ok((storage, mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t))
        }
        _ => Err(Error::InvalidArgument),
    }
}
/// The boundary's form of a native address: the bare family for a Unix domain socket, whose path the
/// local_sockets group reports; `None` for another family or a short answer.
unsafe fn boundary(raw: *const libc::sockaddr, length: libc::socklen_t) -> Option<Address> {
    let length = length as usize;
    if raw.is_null() || length < mem::size_of::<libc::sa_family_t>() { return None; }
    match unsafe { raw.cast::<libc::sa_family_t>().read_unaligned() } as i32 {
        libc::AF_INET if length >= mem::size_of::<libc::sockaddr_in>() => {
            let v4 = unsafe { raw.cast::<libc::sockaddr_in>().read_unaligned() };
            Some(Address::v4(v4.sin_addr.s_addr.to_ne_bytes(), u16::from_be(v4.sin_port)))
        }
        libc::AF_INET6 if length >= mem::size_of::<libc::sockaddr_in6>() => {
            let v6 = unsafe { raw.cast::<libc::sockaddr_in6>().read_unaligned() };
            Some(Address::v6(v6.sin6_addr.s6_addr, u16::from_be(v6.sin6_port), v6.sin6_scope_id))
        }
        libc::AF_UNIX => Some(Address { family: LOCAL as u16, ..Address::default() }),
        _ => None,
    }
}
type Query = unsafe extern "C" fn(i32, *mut libc::sockaddr, *mut libc::socklen_t) -> i32;
unsafe fn endpoint(socket: *mut c_void, query: Query) -> Result<Address> {
    let fd = unsafe { descriptor(socket) }?;
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut length = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    if unsafe { query(fd, ptr::addr_of_mut!(storage).cast(), &mut length) } != 0 { return failed(); }
    unsafe { boundary(ptr::addr_of!(storage).cast(), length) }.ok_or(Error::Os)
}

pub(crate) fn get<T>(fd: i32, level: i32, name: i32) -> Result<T> {
    let mut value = mem::MaybeUninit::<T>::zeroed();
    let mut length = mem::size_of::<T>() as libc::socklen_t;
    if unsafe { libc::getsockopt(fd, level, name, value.as_mut_ptr().cast(), &mut length) } != 0 { return failed(); }
    // Every option read here is plain integers, for which the zeroed remainder of a short answer is valid.
    Ok(unsafe { value.assume_init() })
}
fn set<T>(fd: i32, level: i32, name: i32, value: &T) -> Result<()> {
    if unsafe { libc::setsockopt(fd, level, name, (value as *const T).cast(), mem::size_of::<T>() as libc::socklen_t) } != 0 { return failed(); }
    Ok(())
}
/// `(level, name, is a flag)` of the options that are one native integer.
fn integer(option: u32) -> Option<(i32, i32, bool)> {
    Some(match option {
        sockets::REUSE_ADDRESS => (libc::SOL_SOCKET, libc::SO_REUSEADDR, true), sockets::NO_DELAY => (libc::IPPROTO_TCP, libc::TCP_NODELAY, true),
        sockets::KEEP_ALIVE => (libc::SOL_SOCKET, libc::SO_KEEPALIVE, true), sockets::BROADCAST => (libc::SOL_SOCKET, libc::SO_BROADCAST, true),
        sockets::RECEIVE_BUFFER => (libc::SOL_SOCKET, libc::SO_RCVBUF, false), sockets::SEND_BUFFER => (libc::SOL_SOCKET, libc::SO_SNDBUF, false),
        sockets::IPV6_ONLY => (libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, true),
        // On a datagram socket the kernel answers these three with ENOPROTOOPT or EOPNOTSUPP itself.
        sockets::KEEP_ALIVE_IDLE => (libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, false), sockets::KEEP_ALIVE_INTERVAL => (libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, false),
        sockets::KEEP_ALIVE_COUNT => (libc::IPPROTO_TCP, libc::TCP_KEEPCNT, false),
        _ => return None,
    })
}
/// `(level, name, is a flag)` of the hop limits and multicast options, which live at the level of the socket's family.
/// A stream socket has none of the multicast ones, and the kernel does not say so the same way twice (EINVAL,
/// ENOPROTOOPT or success, measured), so that answer is made here. A local socket has none of them at all.
fn routed(fd: i32, option: u32) -> Result<(i32, i32, bool)> {
    let (stream, domain) = shape(fd)?;
    if domain == libc::AF_UNIX || (stream && option != sockets::HOPS) { return Err(Error::Unsupported); }
    let v6 = domain == libc::AF_INET6;
    Ok(match (option, v6) {
        (sockets::HOPS, false) => (libc::IPPROTO_IP, libc::IP_TTL, false), (sockets::HOPS, true) => (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS, false),
        (sockets::MULTICAST_HOPS, false) => (libc::IPPROTO_IP, libc::IP_MULTICAST_TTL, false),
        (sockets::MULTICAST_HOPS, true) => (libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_HOPS, false),
        (sockets::MULTICAST_LOOPBACK, false) => (libc::IPPROTO_IP, libc::IP_MULTICAST_LOOP, true),
        (sockets::MULTICAST_LOOPBACK, true) => (libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_LOOP, true),
        (sockets::MULTICAST_INTERFACE, false) => (libc::IPPROTO_IP, libc::IP_MULTICAST_IF, false),
        (sockets::MULTICAST_INTERFACE, true) => (libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_IF, false),
        _ => return Err(Error::InvalidArgument),
    })
}
fn timeout(option: u32) -> i32 { if option == sockets::RECEIVE_TIMEOUT { libc::SO_RCVTIMEO } else { libc::SO_SNDTIMEO } }

static WAKE: [AtomicI32; POLL_CHANNELS as usize] = [const { AtomicI32::new(-1) }; POLL_CHANNELS as usize];
/// The eventfd of one wake channel, created on first use. It stays readable
/// until that channel's poll drains it, which is what makes a wake with no poll
/// in progress reach the channel's next poll and no other channel's.
fn wake_descriptor(channel: u32) -> Result<i32> {
    let Some(slot) = WAKE.get(channel as usize) else { return Err(Error::InvalidArgument); };
    let current = slot.load(Ordering::Acquire);
    if current >= 0 { return Ok(current); }
    let created = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if created < 0 { return failed(); }
    match slot.compare_exchange(-1, created, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(created),
        Err(winner) => { unsafe { libc::close(created) }; Ok(winner) }
    }
}
fn events(requested: u32) -> i16 {
    // The peer's shutdown counts as a hangup only for a reader: a writer to a half-closed connection has nothing to learn from it.
    (if requested & POLL_READ != 0 { libc::POLLIN | libc::POLLRDHUP } else { 0 }) | (if requested & POLL_WRITE != 0 { libc::POLLOUT } else { 0 })
}
fn triggered(revents: i16) -> u32 {
    (if revents & libc::POLLIN != 0 { POLL_READ } else { 0 }) | (if revents & libc::POLLOUT != 0 { POLL_WRITE } else { 0 })
        | (if revents & (libc::POLLERR | libc::POLLNVAL) != 0 { POLL_ERROR } else { 0 })
        | (if revents & (libc::POLLHUP | libc::POLLRDHUP) != 0 { POLL_HANGUP } else { 0 })
}
/// `set` has one slot per entry and, when the poll names a channel, a last one for its eventfd.
unsafe fn wait(entries: &mut [PollEntry], set: &mut [libc::pollfd], wake: Option<i32>, timeout_ns: u64) -> Result<()> {
    for (slot, entry) in set.iter_mut().zip(entries.iter()) {
        *slot = libc::pollfd { fd: unsafe { descriptor(entry.socket) }?, events: events(entry.requested), revents: 0 };
    }
    if let Some(fd) = wake { set[entries.len()] = libc::pollfd { fd, events: libc::POLLIN, revents: 0 }; }
    // ppoll leaves the caller's timespec alone, so an interrupted wait resumes against a deadline.
    let deadline = if timeout_ns == 0 || timeout_ns == INFINITE { None } else { Some(<Linux as port::Clock>::monotonic_ns()?.saturating_add(timeout_ns)) };
    let mut remaining = timeout_ns;
    loop {
        // Seconds are held to what every time_t can carry; a wait is never that long.
        let limit = libc::timespec { tv_sec: (remaining / 1_000_000_000).min(i32::MAX as u64) as _, tv_nsec: (remaining % 1_000_000_000) as _ };
        let rc = unsafe { libc::ppoll(set.as_mut_ptr(), set.len() as libc::nfds_t, if timeout_ns == INFINITE { ptr::null() } else { &limit }, ptr::null()) };
        if rc >= 0 { break; }
        if errno() != libc::EINTR { return failed(); }
        if let Some(deadline) = deadline { remaining = deadline.saturating_sub(<Linux as port::Clock>::monotonic_ns()?); }
    }
    for (slot, entry) in set.iter().zip(entries.iter_mut()) { entry.triggered = triggered(slot.revents); }
    if let Some(fd) = wake.filter(|_| set[entries.len()].revents & libc::POLLIN != 0) {
        // One read takes every wake so far. A wake that arrives after ppoll returned is left for the channel's next poll.
        let mut count = 0u64;
        unsafe { libc::read(fd, ptr::addr_of_mut!(count).cast(), mem::size_of::<u64>()) };
    }
    Ok(())
}

impl port::Sockets for Linux {
    unsafe fn create(family: u32, kind: u32) -> Result<*mut c_void> {
        let domain = match family { IPV4 => libc::AF_INET, IPV6 => libc::AF_INET6, LOCAL => libc::AF_UNIX, _ => return Err(Error::InvalidArgument) };
        let shape = match kind { STREAM => libc::SOCK_STREAM, DATAGRAM => libc::SOCK_DGRAM, _ => return Err(Error::InvalidArgument) };
        let fd = unsafe { libc::socket(domain, shape | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 { return failed(); }
        wrap(fd, family == LOCAL, kind == STREAM)
    }
    unsafe fn close(socket: *mut c_void) -> Result<()> {
        let fd = unsafe { descriptor(socket) }?;
        // close releases the descriptor even when it reports EINTR, so it is never retried.
        let code = if unsafe { libc::close(fd) } == 0 { 0 } else { errno() };
        unsafe { libc::free(socket) };
        if code == 0 || code == libc::EINTR { Ok(()) } else { Err(error(code)) }
    }
    unsafe fn bind(socket: *mut c_void, address: &Address) -> Result<()> {
        let (fd, (storage, length)) = (unsafe { internet(socket) }?, native(address)?);
        if unsafe { libc::bind(fd, ptr::addr_of!(storage).cast(), length) } != 0 { return failed(); }
        Ok(())
    }
    unsafe fn listen(socket: *mut c_void, backlog: u32) -> Result<()> {
        let fd = unsafe { descriptor(socket) }?;
        if unsafe { libc::listen(fd, backlog.min(i32::MAX as u32) as i32) } != 0 { return failed(); }
        Ok(())
    }
    unsafe fn accept(socket: *mut c_void) -> Result<(*mut c_void, Address)> {
        let fd = unsafe { descriptor(socket) }?;
        let local = unsafe { (*socket.cast::<Socket>()).local };
        loop {
            let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
            let mut length = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            // The accepted descriptor does not inherit O_NONBLOCK on Linux: it starts blocking as the contract says.
            let accepted = unsafe { libc::accept4(fd, ptr::addr_of_mut!(storage).cast(), &mut length, libc::SOCK_CLOEXEC) };
            if accepted < 0 { if errno() == libc::EINTR { continue; } return waited(fd); }
            return Ok((wrap(accepted, local, true)?, unsafe { boundary(ptr::addr_of!(storage).cast(), length) }.unwrap_or_default()));
        }
    }
    unsafe fn connect(socket: *mut c_void, address: &Address) -> Result<()> {
        let (fd, (storage, length)) = (unsafe { internet(socket) }?, native(address)?);
        if unsafe { libc::connect(fd, ptr::addr_of!(storage).cast(), length) } == 0 { return Ok(()); }
        if errno() != libc::EINTR { return failed(); }
        // An interrupted connect carries on in the kernel; a second call would report EALREADY. Wait for its outcome instead.
        let mut slot = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
        while unsafe { libc::poll(&mut slot, 1, -1) } < 0 { if errno() != libc::EINTR { return failed(); } }
        match get::<i32>(fd, libc::SOL_SOCKET, libc::SO_ERROR)? { 0 => Ok(()), code => Err(error(code)) }
    }
    unsafe fn send(socket: *mut c_void, data: *const u8, size: usize, to: Option<&Address>) -> Result<usize> {
        let fd = if to.is_some() { unsafe { internet(socket) } } else { unsafe { descriptor(socket) } }?;
        let target = match to { Some(address) => Some(native(address)?), None => None };
        let (address, length) = match &target { Some((storage, length)) => ((storage as *const libc::sockaddr_storage).cast(), *length), None => (ptr::null(), 0) };
        loop {
            let n = unsafe { libc::sendto(fd, data.cast(), size, libc::MSG_NOSIGNAL, address, length) };
            if n < 0 { if errno() == libc::EINTR { continue; } return waited(fd); }
            return Ok(n as usize);
        }
    }
    unsafe fn receive(socket: *mut c_void, out: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<Address>)> {
        let fd = unsafe { descriptor(socket) }?;
        let flags = if flags & RECEIVE_PEEK != 0 { libc::MSG_PEEK } else { 0 };
        loop {
            let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
            let mut length = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let n = unsafe { libc::recvfrom(fd, out.cast(), capacity, flags, ptr::addr_of_mut!(storage).cast(), &mut length) };
            if n < 0 { if errno() == libc::EINTR { continue; } return waited(fd); }
            // A stream socket names no sender: the kernel reports a zero length and the answer is `None`. For a local socket the
            // kernel goes by the sender's name instead (measured): it names a stream's sender that has a path and no datagram's
            // sender that has none, so the handle answers.
            let handle = unsafe { &*socket.cast::<Socket>() };
            let sender = if handle.local { (!handle.stream).then_some(Address { family: LOCAL as u16, ..Address::default() }) } else { unsafe { boundary(ptr::addr_of!(storage).cast(), length) } };
            return Ok((n as usize, sender));
        }
    }
    unsafe fn shutdown(socket: *mut c_void, how: u32) -> Result<()> {
        let fd = unsafe { descriptor(socket) }?;
        let how = match how { sockets::SHUTDOWN_READ => libc::SHUT_RD, sockets::SHUTDOWN_WRITE => libc::SHUT_WR, sockets::SHUTDOWN_BOTH => libc::SHUT_RDWR, _ => return Err(Error::InvalidArgument) };
        if unsafe { libc::shutdown(fd, how) } != 0 { return failed(); }
        Ok(())
    }
    unsafe fn local_address(socket: *mut c_void) -> Result<Address> { unsafe { endpoint(socket, libc::getsockname) } }
    unsafe fn peer_address(socket: *mut c_void) -> Result<Address> { unsafe { endpoint(socket, libc::getpeername) } }
    unsafe fn set_blocking(socket: *mut c_void, blocking: bool) -> Result<()> {
        let fd = unsafe { descriptor(socket) }?;
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 { return failed(); }
        let wanted = if blocking { flags & !libc::O_NONBLOCK } else { flags | libc::O_NONBLOCK };
        if wanted != flags && unsafe { libc::fcntl(fd, libc::F_SETFL, wanted) } != 0 { return failed(); }
        Ok(())
    }
    unsafe fn get_option(socket: *mut c_void, option: u32) -> Result<u64> {
        let fd = unsafe { descriptor(socket) }?;
        if let Some((level, name, flag)) = integer(option) {
            let value = get::<i32>(fd, level, name)?;
            return Ok(if flag { (value != 0) as u64 } else { value.max(0) as u64 });
        }
        match option {
            sockets::LINGER => {
                let value = get::<libc::linger>(fd, libc::SOL_SOCKET, libc::SO_LINGER)?;
                Ok(if value.l_onoff == 0 { 0 } else { value.l_linger.max(0) as u64 + 1 })
            }
            sockets::RECEIVE_TIMEOUT | sockets::SEND_TIMEOUT => {
                let value = get::<libc::timeval>(fd, libc::SOL_SOCKET, timeout(option))?;
                Ok((value.tv_sec.max(0) as u64).saturating_mul(1000).saturating_add((value.tv_usec.max(0) as u64).div_ceil(1000)))
            }
            // Reading SO_ERROR clears it; the errno travels as the status the failing call would have reported.
            sockets::ERROR => Ok(match get::<i32>(fd, libc::SOL_SOCKET, libc::SO_ERROR)? { 0 => crate::OK, code => error(code).status() } as u64),
            sockets::AVAILABLE => {
                let mut value: i32 = 0;
                if unsafe { libc::ioctl(fd, libc::FIONREAD, ptr::addr_of_mut!(value)) } != 0 { return failed(); }
                Ok(value.max(0) as u64)
            }
            sockets::HOPS | sockets::MULTICAST_HOPS | sockets::MULTICAST_LOOPBACK | sockets::MULTICAST_INTERFACE => {
                let (level, name, flag) = routed(fd, option)?;
                // The IPv4 index is the one this handle set last (0 until then): nothing else reaches the descriptor.
                if name == libc::IP_MULTICAST_IF { return Ok(unsafe { (*socket.cast::<Socket>()).multicast_interface.load(Ordering::Acquire) } as u64); }
                let value = get::<i32>(fd, level, name)?;
                Ok(if flag { (value != 0) as u64 } else { value.max(0) as u64 })
            }
            _ => Err(Error::InvalidArgument),
        }
    }
    unsafe fn set_option(socket: *mut c_void, option: u32, value: u64) -> Result<()> {
        let fd = unsafe { descriptor(socket) }?;
        // The kernel clamps sizes and linger times itself; only the conversion to its integer saturates here.
        let clamped = value.min(i32::MAX as u64) as i32;
        if let Some((level, name, _)) = integer(option) { return set(fd, level, name, &clamped); }
        match option {
            sockets::LINGER => {
                let seconds = value.saturating_sub(1).min(i32::MAX as u64) as i32;
                set(fd, libc::SOL_SOCKET, libc::SO_LINGER, &libc::linger { l_onoff: (value != 0) as i32, l_linger: seconds })
            }
            sockets::RECEIVE_TIMEOUT | sockets::SEND_TIMEOUT => {
                // The front end holds `value` to 32 bits, so the seconds fit every time_t.
                let limit = libc::timeval { tv_sec: (value / 1000).min(i32::MAX as u64) as _, tv_usec: (value % 1000 * 1000) as _ };
                set(fd, libc::SOL_SOCKET, timeout(option), &limit)
            }
            sockets::HOPS | sockets::MULTICAST_HOPS | sockets::MULTICAST_LOOPBACK => { let (level, name, _) = routed(fd, option)?; set(fd, level, name, &clamped) }
            sockets::MULTICAST_INTERFACE => {
                let (level, name, _) = routed(fd, option)?;
                let result = if name == libc::IP_MULTICAST_IF {
                    set(fd, level, name, &libc::ip_mreqn { imr_multiaddr: libc::in_addr { s_addr: 0 }, imr_address: libc::in_addr { s_addr: 0 }, imr_ifindex: clamped })
                        .map(|()| unsafe { (*socket.cast::<Socket>()).multicast_interface.store(clamped as u32, Ordering::Release) })
                } else { set(fd, level, name, &clamped) };
                // No such interface is EADDRNOTAVAIL from IPv4 and ENODEV from IPv6 (measured): one status for both.
                result.map_err(|e| if errno() == libc::ENODEV { Error::AddressNotAvailable } else { e })
            }
            _ => Err(Error::InvalidArgument),
        }
    }
    unsafe fn poll(entries: &mut [PollEntry], timeout_ns: u64, channel: Option<u32>) -> Result<()> {
        let wake = match channel { Some(channel) => Some(wake_descriptor(channel)?), None => None };
        let count = entries.len() + wake.is_some() as usize;
        let mut inline = [libc::pollfd { fd: -1, events: 0, revents: 0 }; INLINE_POLL];
        let heap = if count > INLINE_POLL { unsafe { libc::calloc(count, mem::size_of::<libc::pollfd>()) }.cast::<libc::pollfd>() } else { ptr::null_mut() };
        if count > INLINE_POLL && heap.is_null() { return Err(Error::OutOfMemory); }
        // SAFETY: zeroed CRT storage is a valid array of `count` pollfd.
        let set = if heap.is_null() { &mut inline[..count] } else { unsafe { core::slice::from_raw_parts_mut(heap, count) } };
        let result = unsafe { wait(entries, set, wake, timeout_ns) };
        unsafe { libc::free(heap.cast()) };
        result
    }
    fn wake(channel: u32) -> Result<()> {
        let fd = wake_descriptor(channel)?;
        let one = 1u64;
        loop {
            if unsafe { libc::write(fd, ptr::addr_of!(one).cast(), mem::size_of::<u64>()) } >= 0 { return Ok(()); }
            // A counter that cannot take one more is already signaled.
            match errno() { libc::EINTR => continue, libc::EAGAIN => return Ok(()), code => return Err(error(code)) }
        }
    }
    unsafe fn resolve(name: &[u8], family: u32, out: *mut Address, capacity: usize) -> Result<usize> {
        if name.len() > MAX_HOST_NAME { return Err(Error::InvalidArgument); }
        let mut node = [0u8; MAX_HOST_NAME + 1];
        node[..name.len()].copy_from_slice(name);
        let mut hints: libc::addrinfo = unsafe { mem::zeroed() };
        hints.ai_family = match family { 0 => libc::AF_UNSPEC, IPV4 => libc::AF_INET, IPV6 => libc::AF_INET6, _ => return Err(Error::InvalidArgument) };
        // One socket type, so an address is listed once rather than once per type.
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut list: *mut libc::addrinfo = ptr::null_mut();
        let code = loop {
            let code = unsafe { libc::getaddrinfo(node.as_ptr().cast(), ptr::null(), &hints, &mut list) };
            if code != libc::EAI_SYSTEM || errno() != libc::EINTR { break code; }
        };
        match code {
            0 => {}
            libc::EAI_NONAME | libc::EAI_NODATA => return Err(Error::NotFound),
            libc::EAI_AGAIN => return Err(Error::Timeout),
            libc::EAI_MEMORY => return Err(Error::OutOfMemory),
            libc::EAI_FAMILY => return Err(Error::Unsupported),
            libc::EAI_SYSTEM => return failed(),
            _ => return Err(Error::Os),
        }
        let (mut found, mut next) = (0, list);
        while !next.is_null() && found < capacity {
            let info = unsafe { &*next };
            next = info.ai_next;
            let Some(address) = (unsafe { boundary(info.ai_addr, info.ai_addrlen) }) else { continue; };
            // The hosts file may name one address on several lines.
            if (0..found).any(|i| unsafe { out.add(i).read() } == address) { continue; }
            unsafe { out.add(found).write(address) };
            found += 1;
        }
        unsafe { libc::freeaddrinfo(list) };
        Ok(found)
    }
    unsafe fn host_name(out: *mut u8, capacity: usize) -> Result<usize> {
        // One byte past what gethostname may fill keeps the text terminated when the name is truncated.
        let mut name = [0u8; MAX_HOST_NAME + 2];
        if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len() - 1) } != 0 { return failed(); }
        let length = name.iter().position(|b| *b == 0).unwrap_or(name.len() - 1) + 1;
        if length <= capacity { unsafe { ptr::copy_nonoverlapping(name.as_ptr(), out, length) }; }
        Ok(length)
    }
}
