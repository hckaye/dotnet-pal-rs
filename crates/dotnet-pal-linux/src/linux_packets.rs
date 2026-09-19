//! Linux provider for the packets group: recvmsg(2) with a control buffer on the
//! descriptors of the sockets provider, whose handle says which sockets carry
//! datagrams. No Rust heap. The destination is `ipi_addr`, the address in the
//! datagram's header; `ipi_spec_dst` is the local address a reply would leave
//! from, which differs for broadcast and multicast traffic.
//!
//! Measured on Linux 6.12: a receive that only peeks and one that truncates carry
//! the same control data as any other; an IPv4 datagram that reaches an IPv6 socket
//! is described by IPV6_PKTINFO with the IPv4-mapped destination, the form the
//! kernel names its sender in, so both come out as one family.
use crate::linux::Linux;
use crate::linux_sockets::{errno, packet, sender, waited};
use dotnet_pal_rs::packets::Info;
use dotnet_pal_rs::port::{self, Result};
use dotnet_pal_rs::sockets::{Address, RECEIVE_PEEK};
use core::{ffi::c_void, mem, ptr};

/// The destination with the scope an IPv6 address of link scope needs to mean anything: the interface it arrived through.
fn scoped(address: libc::in6_addr, interface: u32) -> Address {
    let a = address.s6_addr;
    let link = (a[0] == 0xfe && a[1] & 0xc0 == 0x80) || (a[0] == 0xff && a[1] & 0x0f == 2);
    Address::v6(a, 0, if link { interface } else { 0 })
}

impl port::Packets for Linux {
    unsafe fn receive(socket: *mut c_void, data: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<Address>, Info)> {
        let fd = unsafe { packet(socket) }?;
        let flags = if flags & RECEIVE_PEEK != 0 { libc::MSG_PEEK } else { 0 };
        loop {
            let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
            // Both kinds of packet information take 64 bytes; the rest is for what an option set behind the provider's back adds.
            let mut control = [0u64; 32];
            let mut part = libc::iovec { iov_base: data.cast(), iov_len: capacity };
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_name = ptr::addr_of_mut!(storage).cast();
            message.msg_namelen = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            message.msg_iov = &mut part;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = mem::size_of_val(&control) as _;
            let n = unsafe { libc::recvmsg(fd, &mut message, flags) };
            if n < 0 { if errno() == libc::EINTR { continue; } return waited(fd); }
            let mut info = Info::default();
            // SAFETY: the kernel has left `msg_controllen` at the bytes of `control` it filled with whole records.
            let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
            while !header.is_null() {
                let (level, kind, record) = unsafe { ((*header).cmsg_level, (*header).cmsg_type, libc::CMSG_DATA(header)) };
                if level == libc::IPPROTO_IP && kind == libc::IP_PKTINFO {
                    let arrived = unsafe { record.cast::<libc::in_pktinfo>().read_unaligned() };
                    info = Info { interface_index: arrived.ipi_ifindex.max(0) as u32, reserved: 0, destination: Address::v4(arrived.ipi_addr.s_addr.to_ne_bytes(), 0) };
                } else if level == libc::IPPROTO_IPV6 && kind == libc::IPV6_PKTINFO {
                    let arrived = unsafe { record.cast::<libc::in6_pktinfo>().read_unaligned() };
                    info = Info { interface_index: arrived.ipi6_ifindex, reserved: 0, destination: scoped(arrived.ipi6_addr, arrived.ipi6_ifindex) };
                }
                header = unsafe { libc::CMSG_NXTHDR(&message, header) };
            }
            return Ok((n as usize, unsafe { sender(socket, &storage, message.msg_namelen) }, info));
        }
    }
}
