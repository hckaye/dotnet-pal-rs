//! Where a datagram arrived, for the desktop port: recvmsg(2) with a control buffer on
//! the handles of the sockets provider, which sets the option that asks for the
//! description and says which sockets carry datagrams.
//!
//! The destination is `ipi_addr`, the address in the datagram's header. `ipi_spec_dst`
//! is the local address a reply would leave from on Linux and stays 0.0.0.0 on macOS.
//! Both systems hand the description to a receive that only peeks and to one that
//! truncates, and describe an IPv4 datagram that reaches an IPv6 socket by IPV6_PKTINFO
//! with the IPv4-mapped destination, the form its sender is named in (all measured on
//! Linux 6.12 and Darwin 25). Every other target, Windows included, has no provider: the
//! capability is absent there.

mod unix {
    use super::super::sockets::{waited, Socket};
    use super::super::{borrow, Std};
    use dotnet_pal_rs::packets::Info;
    use dotnet_pal_rs::port::{self, Error, Result};
    use dotnet_pal_rs::sockets::{Address, RECEIVE_PEEK};
    use socket2::SockAddr;
    use std::{ffi::c_void, io, mem::{size_of_val, zeroed}, os::fd::AsRawFd};

    /// The destination with the scope an IPv6 address of link scope needs to mean anything: the interface it arrived through.
    fn scoped(address: libc::in6_addr, interface: u32) -> Address {
        let a = address.s6_addr;
        let link = (a[0] == 0xfe && a[1] & 0xc0 == 0x80) || (a[0] == 0xff && a[1] & 0x0f == 2);
        Address::v6(a, 0, if link { interface } else { 0 })
    }
    /// What the control data of `message` says about the datagram; nothing when the option was off as it arrived.
    unsafe fn described(message: &libc::msghdr) -> Info {
        let mut info = Info::default();
        // SAFETY: the OS has left `msg_controllen` at the bytes of the control buffer it filled with whole records.
        let mut header = unsafe { libc::CMSG_FIRSTHDR(message) };
        while !header.is_null() {
            let (level, kind, record) = unsafe { ((*header).cmsg_level, (*header).cmsg_type, libc::CMSG_DATA(header)) };
            if level == libc::IPPROTO_IP && kind == libc::IP_PKTINFO {
                let arrived = unsafe { record.cast::<libc::in_pktinfo>().read_unaligned() };
                // The index is an int on Linux and unsigned on macOS.
                #[allow(clippy::unnecessary_cast)]
                let interface_index = arrived.ipi_ifindex as u32;
                info = Info { interface_index, reserved: 0, destination: Address::v4(arrived.ipi_addr.s_addr.to_ne_bytes(), 0) };
            } else if level == libc::IPPROTO_IPV6 && kind == libc::IPV6_PKTINFO {
                let arrived = unsafe { record.cast::<libc::in6_pktinfo>().read_unaligned() };
                // Unsigned on Linux and macOS, an int on Android.
                #[allow(clippy::unnecessary_cast)]
                let interface_index = arrived.ipi6_ifindex as u32;
                info = Info { interface_index, reserved: 0, destination: scoped(arrived.ipi6_addr, interface_index) };
            }
            header = unsafe { libc::CMSG_NXTHDR(message, header) };
        }
        info
    }

    impl port::Packets for Std {
        unsafe fn receive(socket: *mut c_void, data: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<Address>, Info)> {
            let socket = unsafe { borrow::<Socket>(socket) }?;
            // Only a datagram has a destination of its own: a stream's is its local address, and a local socket has no IP address.
            if socket.stream || socket.local { return Err(Error::InvalidArgument); }
            let flags = if flags & RECEIVE_PEEK != 0 { libc::MSG_PEEK } else { 0 };
            // Both kinds of packet information take 64 bytes; the rest is for what an option set behind the provider's back adds.
            let mut control = [0u64; 32];
            // macOS answers a receive into no room at once and with nothing while no datagram is there, where Linux waits for one (both measured);
            // with a datagram there both take it. One byte of room that is not reported makes every OS wait, or say that it would have to.
            let mut spare = 0u8;
            let mut part = if capacity == 0 { libc::iovec { iov_base: std::ptr::addr_of_mut!(spare).cast(), iov_len: 1 } } else { libc::iovec { iov_base: data.cast(), iov_len: capacity } };
            let mut message: libc::msghdr = unsafe { zeroed() };
            (message.msg_iov, message.msg_iovlen, message.msg_control) = (&mut part, 1, control.as_mut_ptr().cast());
            // SAFETY: the OS fills the storage `try_init` lends with the sender's address and says how much of it that is.
            let ((done, info), sender) = unsafe { SockAddr::try_init(|storage, length| loop {
                (message.msg_name, message.msg_namelen, message.msg_controllen) = (storage.cast(), *length, size_of_val(&control) as _);
                let n = libc::recvmsg(socket.inner.as_raw_fd(), &mut message, flags);
                if n >= 0 { *length = message.msg_namelen; return Ok(((n as usize).min(capacity), described(&message))); }
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted { return Err(e); }
            }) }.map_err(|e| waited(socket, e))?;
            Ok((done, socket.sender(&sender), info))
        }
    }
}
