//! Users and groups of the desktop port: the reentrant passwd lookups of the C
//! library, getgroups for this process and getgrouplist for an account.
//!
//! glibc says how long a list is when the buffer is short. macOS does not: it
//! fills the buffer, answers -1 and leaves the count at the capacity, and with a
//! capacity of 0 it answers success and no groups at all. So the list is read
//! into a buffer of the provider's own that grows until the call succeeds. macOS
//! also takes and writes the group ids of that call as `int`. Outside Linux the
//! calls of getgrouplist are serialized, as the runtime's native layer serializes
//! them. Windows has no such database: the capability is absent there.



mod absent {
    use super::super::Std;
    use dotnet_pal_rs::accounts::Account;
    use dotnet_pal_rs::port::{self, Error, Result};
    impl port::Accounts for Std {
        const PROVIDED: bool = false;
        fn user_by_id(_: u32) -> Result<Account> { Err(Error::Unsupported) }
        fn user_by_name(_: &[u8]) -> Result<Account> { Err(Error::Unsupported) }
        unsafe fn process_groups(_: *mut u32, _: usize) -> Result<usize> { Err(Error::Unsupported) }
        unsafe fn user_groups(_: &[u8], _: u32, _: *mut u32, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    }
}
