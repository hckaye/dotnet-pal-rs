//! Linux provider for the local_sockets group: `sockaddr_un` paths and SO_PEERCRED on
//! the descriptors of the sockets provider, whose handle says which sockets are local.
//! No Rust heap. Abstract names (a first byte of NUL) are not paths: a socket that has
//! one answers as a socket without a path.
//!
//! Measured on Linux 6.12: a second bind answers EADDRINUSE for the socket's own path,
//! because the kernel creates the path before it looks at the socket, so a bound socket
//! is refused here. SO_PEERCRED succeeds on every local socket: a listener answers with
//! its own user, a socket without a peer and a connected datagram socket with user -1.
//! A non-blocking connect never reports EINPROGRESS; it is done, or EAGAIN while the
//! listener's backlog is full, which is also what an expired SEND_TIMEOUT reports.
use crate::linux::Linux;
use crate::linux_sockets::{errno, get, local, waited};
use dotnet_pal_rs::port::{self, Error, Result};
use core::{ffi::c_void, mem, ptr};

const PATH: usize = mem::offset_of!(libc::sockaddr_un, sun_path);
/// A native address for a path the front end has checked (not empty, no NUL). The kernel takes a path that fills
/// `sun_path` without a terminator; the boundary's limit leaves room for it, as every other Unix requires.
fn native(path: &[u8]) -> Result<(libc::sockaddr_un, libc::socklen_t)> {
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    if path.len() >= address.sun_path.len() { return Err(Error::NameTooLong); }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (to, from) in address.sun_path.iter_mut().zip(path) { *to = *from as _; }
    Ok((address, (PATH + path.len() + 1) as libc::socklen_t))
}
fn error(code: i32) -> Error {
    match code {
        libc::ENOENT => Error::NotFound, libc::EACCES | libc::EPERM => Error::AccessDenied, libc::ENOTDIR => Error::NotDirectory,
        libc::EROFS => Error::ReadOnly, libc::ENAMETOOLONG => Error::NameTooLong, libc::EADDRINUSE => Error::AddressInUse,
        // A path nobody listens on, one that is no socket, and a socket of the other kind (EPROTOTYPE) all refuse the connection.
        libc::ECONNREFUSED | libc::EPROTOTYPE => Error::ConnectionRefused, libc::EINPROGRESS => Error::InProgress,
        libc::EISCONN => Error::AlreadyConnected, libc::ETIMEDOUT => Error::Timeout, libc::EINVAL => Error::InvalidArgument,
        libc::ENOMEM | libc::ENOBUFS => Error::OutOfMemory,
        _ => Error::Os,
    }
}
type Query = unsafe extern "C" fn(i32, *mut libc::sockaddr, *mut libc::socklen_t) -> i32;
/// What `query` reports and how many bytes of `sun_path` it used: none for a socket without a name.
fn named(fd: i32, query: Query) -> Result<(libc::sockaddr_un, usize)> {
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    let mut length = mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    if unsafe { query(fd, ptr::addr_of_mut!(address).cast(), &mut length) } != 0 { return Err(if errno() == libc::ENOTCONN { Error::NotConnected } else { error(errno()) }); }
    Ok((address, (length as usize).saturating_sub(PATH).min(address.sun_path.len())))
}

impl port::LocalSockets for Linux {
    unsafe fn bind(socket: *mut c_void, path: &[u8]) -> Result<()> {
        let ((fd, _), (address, length)) = (unsafe { local(socket) }?, native(path)?);
        // Any name counts, an abstract one too: the kernel would answer for the path before it answers for the socket.
        if named(fd, libc::getsockname)?.1 != 0 { return Err(Error::InvalidArgument); }
        if unsafe { libc::bind(fd, ptr::addr_of!(address).cast(), length) } != 0 { return Err(error(errno())); }
        Ok(())
    }
    unsafe fn connect(socket: *mut c_void, path: &[u8]) -> Result<()> {
        let ((fd, _), (address, length)) = (unsafe { local(socket) }?, native(path)?);
        loop {
            if unsafe { libc::connect(fd, ptr::addr_of!(address).cast(), length) } == 0 { return Ok(()); }
            // Nothing carries on in the kernel after an interrupted local connect, so it is made again.
            match errno() { libc::EINTR => continue, libc::EAGAIN => return waited(fd), code => return Err(error(code)) }
        }
    }
    unsafe fn address(socket: *mut c_void, peer: bool, out: *mut u8, capacity: usize) -> Result<usize> {
        let (fd, _) = unsafe { local(socket) }?;
        let (address, used) = named(fd, if peer { libc::getpeername } else { libc::getsockname })?;
        // A name that fills `sun_path` has no terminator; a shorter one is reported with it. An abstract name starts with one.
        let path = &address.sun_path[..used];
        let length = path.iter().position(|b| *b == 0).unwrap_or(used);
        if length == 0 { return Err(Error::NotFound); }
        if length < capacity { unsafe { ptr::copy_nonoverlapping(path.as_ptr().cast::<u8>(), out, length); out.add(length).write(0); } }
        Ok(length + 1)
    }
    unsafe fn peer_user(socket: *mut c_void) -> Result<u32> {
        let (fd, stream) = unsafe { local(socket) }?;
        // The credentials belong to a connection: a datagram socket has none, connected or not.
        if !stream { return Err(Error::Unsupported); }
        // A listener would answer with its own user, so the peer is asked for first.
        named(fd, libc::getpeername)?;
        let peer = get::<libc::ucred>(fd, libc::SOL_SOCKET, libc::SO_PEERCRED)?;
        if peer.uid == u32::MAX { Err(Error::NotConnected) } else { Ok(peer.uid) }
    }
}
