//! Unix domain sockets of the desktop port: paths and the peer's user on the handles
//! of the sockets provider, which makes the sockets and says which of them are local.
//!
//! `socket2` builds the address; the name of a socket is read from the bytes the OS
//! returned, because a name that fills `sun_path` has no terminator (Linux lets a
//! peer bind one) and `socket2` assumes it has. A path may be 107 bytes on Linux and
//! 103 on macOS. Abstract names are no paths: such a socket answers as one without.
//!
//! What differs, measured on Linux 6.12 and Darwin 25. A second bind of a socket:
//! Linux looks at the path first and calls the socket's own path taken, macOS answers
//! EINVAL, so a socket with a name is refused here. A socket without a name reports
//! no name on Linux and an empty one on macOS. A path that is no socket refuses the
//! connection on Linux and is ENOTSOCK on macOS. A listener whose backlog is full
//! keeps a Linux connect waiting (EAGAIN for a non-blocking socket and for an expired
//! SEND_TIMEOUT) and refuses one on macOS at once; neither reports a connect in
//! progress. The peer's user is SO_PEERCRED on Linux, which also answers a listener
//! (with its own user) and a socket without a connection (with user -1), and
//! `getpeereid` on macOS, which answers neither; a datagram socket has no credentials
//! on either (user -1, EINVAL). Once the peer has closed, both still answer with its
//! user; Linux still names its path, macOS calls getpeername EINVAL, which is
//! reported as the connection it no longer has. Windows offers none of this: the
//! capability is absent.

mod unix {
    use super::super::sockets::{retry, waited, Socket};
    use super::super::{borrow, Std};
    use dotnet_pal_rs::port::{self, Error, Result};
    use socket2::SockAddr;
    use std::{ffi::{c_void, OsStr}, io, mem::offset_of, os::unix::ffi::OsStrExt, ptr};

    fn error(e: io::Error) -> Error {
        match e.raw_os_error() {
            Some(libc::ENOENT) => Error::NotFound, Some(libc::EACCES | libc::EPERM) => Error::AccessDenied, Some(libc::ENOTDIR) => Error::NotDirectory,
            Some(libc::EROFS) => Error::ReadOnly, Some(libc::ENAMETOOLONG) => Error::NameTooLong, Some(libc::EADDRINUSE) => Error::AddressInUse,
            // A path nobody listens on, one that is no socket (ENOTSOCK on macOS), and a socket of the other kind (EPROTOTYPE) all refuse the connection.
            Some(libc::ECONNREFUSED | libc::EPROTOTYPE | libc::ENOTSOCK) => Error::ConnectionRefused, Some(libc::EINPROGRESS) => Error::InProgress,
            Some(libc::EISCONN) => Error::AlreadyConnected, Some(libc::ENOTCONN) => Error::NotConnected, Some(libc::ETIMEDOUT) => Error::Timeout,
            Some(libc::EINVAL) => Error::InvalidArgument, Some(libc::ENOMEM | libc::ENOBUFS) => Error::OutOfMemory,
            Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => Error::WouldBlock,
            _ => Error::Os,
        }
    }
    unsafe fn local<'a>(socket: *mut c_void) -> Result<&'a Socket> {
        let socket = unsafe { borrow::<Socket>(socket) }?;
        if socket.local { Ok(socket) } else { Err(Error::InvalidArgument) }
    }
    /// A native address for a path the front end has checked (not empty, no NUL); the OS wants room for a terminator.
    fn native(path: &[u8]) -> Result<SockAddr> {
        let room = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }.sun_path.len();
        if path.len() >= room { return Err(Error::NameTooLong); }
        SockAddr::unix(OsStr::from_bytes(path)).map_err(|_| Error::InvalidArgument)
    }
    /// The bytes of `sun_path` the OS filled in: none for a socket without a name on Linux, zeros for one on macOS.
    fn name(address: &SockAddr) -> &[u8] {
        if !address.is_unix() { return &[]; }
        let (start, room) = (offset_of!(libc::sockaddr_un, sun_path), unsafe { std::mem::zeroed::<libc::sockaddr_un>() }.sun_path.len());
        let used = (address.len() as usize).saturating_sub(start).min(room);
        // SAFETY: the storage behind a `SockAddr` is longer than a `sockaddr_un`, and `used` stays inside `sun_path`.
        unsafe { std::slice::from_raw_parts(address.as_ptr().cast::<u8>().add(start), used) }
    }

    fn user(socket: &Socket) -> Result<u32> {
        // A listener would answer with its own user, so the peer is asked for first.
        socket.inner.peer_addr().map_err(error)?;
        let (mut peer, mut length) = (libc::ucred { pid: 0, uid: u32::MAX, gid: u32::MAX }, std::mem::size_of::<libc::ucred>() as libc::socklen_t);
        if unsafe { libc::getsockopt(std::os::fd::AsRawFd::as_raw_fd(&socket.inner), libc::SOL_SOCKET, libc::SO_PEERCRED, ptr::addr_of_mut!(peer).cast(), &mut length) } != 0 { return Err(error(io::Error::last_os_error())); }
        if peer.uid == u32::MAX { Err(Error::NotConnected) } else { Ok(peer.uid) }
    }



    impl port::LocalSockets for Std {
        unsafe fn bind(socket: *mut c_void, path: &[u8]) -> Result<()> {
            let (socket, address) = (unsafe { local(socket) }?, native(path)?);
            // Any name counts, an abstract one too: Linux would answer for the path before it answers for the socket.
            if name(&socket.inner.local_addr().map_err(error)?).iter().any(|b| *b != 0) { return Err(Error::InvalidArgument); }
            socket.inner.bind(&address).map_err(error)
        }
        unsafe fn connect(socket: *mut c_void, path: &[u8]) -> Result<()> {
            let (socket, address) = (unsafe { local(socket) }?, native(path)?);
            // Nothing carries on in the OS after an interrupted local connect, so it is made again. EAGAIN is a full backlog:
            // "not now" for a non-blocking socket, an expired SEND_TIMEOUT for a blocking one.
            retry(|| socket.inner.connect(&address)).map_err(|e| match error(e) { Error::WouldBlock => waited(socket, io::Error::from_raw_os_error(libc::EAGAIN)), other => other })
        }
        unsafe fn address(socket: *mut c_void, peer: bool, out: *mut u8, capacity: usize) -> Result<usize> {
            let socket = unsafe { local(socket) }?;
            // macOS has EINVAL for the name of a peer that has closed; no argument of this call is at fault.
            let address = if peer { socket.inner.peer_addr().map_err(|e| match error(e) { Error::InvalidArgument => Error::NotConnected, other => other }) } else { socket.inner.local_addr().map_err(error) }?;
            // A name that fills `sun_path` has no terminator; a shorter one is reported with it. An abstract name starts with one.
            let path = name(&address);
            let path = &path[..path.iter().position(|b| *b == 0).unwrap_or(path.len())];
            if path.is_empty() { return Err(Error::NotFound); }
            if path.len() < capacity { unsafe { ptr::copy_nonoverlapping(path.as_ptr(), out, path.len()); out.add(path.len()).write(0); } }
            Ok(path.len() + 1)
        }
        unsafe fn peer_user(socket: *mut c_void) -> Result<u32> {
            let socket = unsafe { local(socket) }?;
            // The credentials belong to a connection: a datagram socket has none, connected or not.
            if !socket.stream { return Err(Error::Unsupported); }
            user(socket)
        }
    }
}
