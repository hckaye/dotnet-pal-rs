//! Linux provider for the accounts group: the reentrant passwd lookups of the C
//! library, getgroups for this process and getgrouplist for an account. The strings
//! of an entry live in a block of the C heap that doubles while the C library says
//! ERANGE; nothing outlives a call, and there is no Rust heap.
use dotnet_pal_rs::accounts::{Account, MAX_ACCOUNT_NAME};
use crate::linux::Linux;
use dotnet_pal_rs::port::{self, Error, Result};
use core::ffi::{c_char, c_int, CStr};
use core::{mem, ptr};

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
/// The size the strings of an entry start with, and the size growing stops at: an entry the boundary carries is far smaller.
const STRINGS: usize = 1024;
const LIMIT: usize = 1 << 20;

/// One passwd lookup; `convert` reads the entry while its strings are alive.
fn lookup<T>(query: impl Fn(*mut libc::passwd, *mut c_char, usize, *mut *mut libc::passwd) -> c_int, convert: impl Fn(&libc::passwd) -> Result<T>) -> Result<T> {
    let mut size = STRINGS;
    loop {
        let strings = unsafe { libc::malloc(size) }.cast::<c_char>();
        if strings.is_null() { return Err(Error::OutOfMemory); }
        let mut entry = mem::MaybeUninit::<libc::passwd>::zeroed();
        let mut found = ptr::null_mut();
        let code = loop {
            let code = query(entry.as_mut_ptr(), strings, size, &mut found);
            if code != libc::EINTR { break code; }
        };
        // SAFETY: a result is the entry above, filled in, with its texts inside `strings`.
        let answer = if found.is_null() { None } else { Some(convert(unsafe { &*found })) };
        unsafe { libc::free(strings.cast()) };
        match (answer, code) {
            (Some(answer), _) => return answer,
            (None, libc::ERANGE) if size < LIMIT => size *= 2,
            // No entry is 0 without a result; POSIX lets a database that is not there say it with one of these as well.
            (None, 0 | libc::ENOENT | libc::ESRCH | libc::EBADF | libc::EPERM) => return Err(Error::NotFound),
            (None, libc::ENOMEM) => return Err(Error::OutOfMemory),
            (None, _) => return Err(Error::Os),
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
fn terminated(name: &[u8]) -> Result<[u8; MAX_ACCOUNT_NAME + 1]> {
    if name.is_empty() || name.len() > MAX_ACCOUNT_NAME || name.contains(&0) { return Err(Error::InvalidArgument); }
    let mut text = [0u8; MAX_ACCOUNT_NAME + 1];
    text[..name.len()].copy_from_slice(name);
    Ok(text)
}
fn by_name<T>(name: &[u8; MAX_ACCOUNT_NAME + 1], convert: impl Fn(&libc::passwd) -> Result<T>) -> Result<T> {
    lookup(|entry, strings, size, found| unsafe { libc::getpwnam_r(name.as_ptr().cast(), entry, strings, size, found) }, convert)
}

impl port::Accounts for Linux {
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
            let written = unsafe { libc::getgroups(given, out) };
            if written >= 0 { return Ok(written as usize); }
            // EINVAL is a list another thread made longer between the two calls.
            if errno() != libc::EINVAL { return Err(Error::Os); }
        }
    }
    unsafe fn user_groups(name: &[u8], primary_group: u32, out: *mut u32, capacity: usize) -> Result<usize> {
        let name = terminated(name)?;
        let given = c_int::try_from(capacity).map_err(|_| Error::InvalidArgument)?;
        // getgrouplist answers for any name with the group it was given. A name that is no account has no groups to list, so the lookup comes first.
        by_name(&name, |_| Ok(()))?;
        // The C library wants a buffer even for a list of no entries.
        let (mut count, mut none) = (given, 0u32);
        unsafe { *libc::__errno_location() = 0 };
        let result = unsafe { libc::getgrouplist(name.as_ptr().cast(), primary_group, if capacity == 0 { &mut none } else { out }, &mut count) };
        // -1 is a list longer than the buffer, with its length in `count`. Without a larger count the C library failed before it had one: glibc when it cannot allocate its own list.
        if result < 0 && count <= given { return Err(if errno() == libc::ENOMEM { Error::OutOfMemory } else { Error::Os }); }
        usize::try_from(count).map_err(|_| Error::Os)
    }
}
