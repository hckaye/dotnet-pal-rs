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
#[cfg(unix)]
mod unix {
    use super::super::Std;
    use dotnet_pal_rs::accounts::{Account, MAX_ACCOUNT_NAME, MAX_GROUPS};
    use dotnet_pal_rs::port::{self, Error, Result};
    use std::ffi::{c_char, c_int, CStr, CString};
    use std::ptr;

    /// A group id as getgrouplist takes and writes it.
    #[cfg(target_vendor = "apple")]
    type Listed = c_int;
    #[cfg(not(target_vendor = "apple"))]
    type Listed = libc::gid_t;
    // The boundary's ids are what the C library's are, so the group lists are written in place.
    const _: () = assert!(std::mem::size_of::<libc::gid_t>() == 4 && std::mem::size_of::<libc::uid_t>() == 4 && std::mem::size_of::<Listed>() == 4);

    /// One passwd lookup; `convert` reads the entry while its strings are alive.
    fn lookup<T>(query: impl Fn(*mut libc::passwd, *mut c_char, usize, *mut *mut libc::passwd) -> c_int, convert: impl Fn(&libc::passwd) -> Result<T>) -> Result<T> {
        let mut strings: Vec<c_char> = vec![0; 1024];
        loop {
            let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
            let mut found = ptr::null_mut();
            match query(&mut entry, strings.as_mut_ptr(), strings.len(), &mut found) {
                libc::EINTR => continue,
                // SAFETY: a result is the entry above, filled in, with its texts inside `strings`.
                _ if !found.is_null() => return convert(unsafe { &*found }),
                libc::ERANGE if strings.len() < 1 << 20 => { let size = strings.len() * 2; strings.resize(size, 0); }
                // No entry is 0 without a result; POSIX lets a database that is not there say it with one of these as well.
                0 | libc::ENOENT | libc::ESRCH | libc::EBADF | libc::EPERM => return Err(Error::NotFound),
                libc::ENOMEM => return Err(Error::OutOfMemory),
                _ => return Err(Error::Os),
            }
        }
    }
    /// A text of the entry; an entry without one has an empty one.
    unsafe fn text<'a>(field: *const c_char) -> &'a [u8] { if field.is_null() { &[] } else { unsafe { CStr::from_ptr(field) }.to_bytes() } }
    /// An entry whose name, home or shell is longer than its field is one the boundary cannot carry.
    fn account(entry: &libc::passwd) -> Result<Account> {
        unsafe { Account::new(entry.pw_uid, entry.pw_gid, text(entry.pw_name), text(entry.pw_dir), text(entry.pw_shell)) }.ok_or(Error::Os)
    }
    /// The C library takes a terminated name; the boundary's has a length.
    fn terminated(name: &[u8]) -> Result<CString> {
        if name.is_empty() || name.len() > MAX_ACCOUNT_NAME { return Err(Error::InvalidArgument); }
        CString::new(name).map_err(|_| Error::InvalidArgument)
    }
    fn by_name<T>(name: &CStr, convert: impl Fn(&libc::passwd) -> Result<T>) -> Result<T> {
        lookup(|entry, strings, size, found| unsafe { libc::getpwnam_r(name.as_ptr(), entry, strings, size, found) }, convert)
    }

    impl port::Accounts for Std {
        fn user_by_id(user_id: u32) -> Result<Account> {
            lookup(|entry, strings, size, found| unsafe { libc::getpwuid_r(user_id, entry, strings, size, found) }, account)
        }
        fn user_by_name(name: &[u8]) -> Result<Account> { by_name(&terminated(name)?, account) }
        unsafe fn process_groups(out: *mut u32, capacity: usize) -> Result<usize> {
            let given = c_int::try_from(capacity).map_err(|_| Error::InvalidArgument)?;
            loop {
                // getgroups refuses a buffer that is too short without saying how many there are: only a call without a buffer counts.
                let count = unsafe { libc::getgroups(0, ptr::null_mut()) };
                if count < 0 { return Err(Error::Os); }
                if count > given || count == 0 { return Ok(count as usize); }
                let written = unsafe { libc::getgroups(given, out.cast()) };
                if written >= 0 { return Ok(written as usize); }
                // EINVAL is a list another thread made longer between the two calls.
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINVAL) { return Err(Error::Os); }
            }
        }
        #[allow(clippy::unnecessary_cast)] // `Listed` is the boundary's type everywhere but on Apple systems
        unsafe fn user_groups(name: &[u8], primary_group: u32, out: *mut u32, capacity: usize) -> Result<usize> {
            let name = terminated(name)?;
            // getgrouplist answers for any name with the group it was given (macOS adds a group -1). A name that is no account
            // has no groups to list, so the lookup comes first.
            by_name(&name, |_| Ok(()))?;
            // glibc, musl and bionic say their getgrouplist is safe to call from several threads. Nothing says so elsewhere, and the runtime's
            // own native layer takes a lock there, so one call at a time.
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let _one_at_a_time = { static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(()); LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) };
            let mut size = capacity.clamp(64, MAX_GROUPS);
            loop {
                let mut list: Vec<Listed> = vec![0; size];
                let mut count = size as c_int;
                let result = unsafe { libc::getgrouplist(name.as_ptr(), primary_group as Listed, list.as_mut_ptr(), &mut count) };
                if result >= 0 {
                    let count = usize::try_from(count).ok().filter(|count| *count <= size).ok_or(Error::Os)?;
                    if count <= capacity { for (index, group) in list[..count].iter().enumerate() { unsafe { out.add(index).write(*group as u32) }; } }
                    return Ok(count);
                }
                // A list longer than any the boundary carries, or a C library that fails for another reason every time.
                if size == MAX_GROUPS { return Err(Error::Os); }
                size = usize::try_from(count).unwrap_or(0).max(size * 2).min(MAX_GROUPS);
            }
        }
    }
}

#[cfg(not(unix))]
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
