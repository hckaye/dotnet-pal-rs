//! Network interfaces, reverse lookup and multicast membership of the desktop port.
//!
//! Unix: getifaddrs(3). An interface is a distinct name of that list (an alias label
//! such as "eth0:1" names its interface); its index is if_nametoindex, its MTU the
//! SIOCGIFMTU ioctl. Linux takes the type and the hardware address from the
//! AF_PACKET entry, the speed from sysfs and calls an Ethernet device with a
//! `wireless` directory there wireless. macOS takes them from the AF_LINK entry and
//! the `if_data` behind it, whose 32-bit line speed cannot tell its largest value
//! from a faster link, so that value is reported as unknown; Wi-Fi is IFT_ETHER
//! there and reads as Ethernet. macOS cuts a netmask's `sa_len` to its last set
//! byte (5 for 255.0.0.0, measured), so only the bytes inside it are counted.
//!
//! Both enumerations answer from one snapshot. Index 0 of either takes a new one,
//! as does the first call of all, and every call copies its entry out under the
//! lock: threads that enumerate at once each get whole entries and an end.
//!
//! Membership is `socket2` on the handles of the sockets provider. macOS joins a
//! group on an interface of its own choice when the index names none (measured), so
//! an index nobody has is answered here. An IPv6 socket that carries both families
//! joins an IPv4 group at the IPv4 level on Linux, which macOS refuses (EINVAL); macOS
//! takes the group mapped into IPv6 (::ffff:a.b.c.d) at the IPv6 level instead. Either
//! OS lets an IPv6-only socket join that way and delivers nothing to it (all measured,
//! Linux 6.12 and Darwin 25), so the socket is asked first. Reverse lookup is getnameinfo(3).
//! Windows offers none of this: the capability is absent there.



mod absent {
    use super::super::Std;
    use dotnet_pal_rs::network::{Interface, InterfaceAddress};
    use dotnet_pal_rs::port::{self, Error, Result};
    use dotnet_pal_rs::sockets::Address;
    use std::ffi::c_void;
    impl port::Network for Std {
        const PROVIDED: bool = false;
        fn interface_entry(_: usize) -> Result<Interface> { Err(Error::Unsupported) }
        fn address_entry(_: usize) -> Result<InterfaceAddress> { Err(Error::Unsupported) }
        unsafe fn reverse_lookup(_: &Address, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
        unsafe fn membership(_: *mut c_void, _: &Address, _: u32, _: bool) -> Result<()> { Err(Error::Unsupported) }
    }
}
