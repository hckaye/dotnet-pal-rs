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



mod absent {
    use super::super::Std;
    use dotnet_pal_rs::port::{self, Error, Result};
    use std::ffi::c_void;
    impl port::LocalSockets for Std {
        const PROVIDED: bool = false;
        unsafe fn bind(_: *mut c_void, _: &[u8]) -> Result<()> { Err(Error::Unsupported) }
        unsafe fn connect(_: *mut c_void, _: &[u8]) -> Result<()> { Err(Error::Unsupported) }
        unsafe fn address(_: *mut c_void, _: bool, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
        unsafe fn peer_user(_: *mut c_void) -> Result<u32> { Err(Error::Unsupported) }
    }
}
