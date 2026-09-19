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



mod absent {
    use super::super::Std;
    use dotnet_pal_rs::packets::Info;
    use dotnet_pal_rs::port::{self, Error, Result};
    use dotnet_pal_rs::sockets::Address;
    use std::ffi::c_void;
    impl port::Packets for Std {
        const PROVIDED: bool = false;
        unsafe fn receive(_: *mut c_void, _: *mut u8, _: usize, _: u32) -> Result<(usize, Option<Address>, Info)> { Err(Error::Unsupported) }
    }
}
