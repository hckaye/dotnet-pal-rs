//! Exercises the accounts group of the std port through the negotiated C table, on
//! whatever desktop OS runs the test, against the C library's own lookups:
//! getpwuid, getpwnam, getpwent, getgroups and getgrouplist.
#![cfg(unix)]
use dotnet_pal_rs::accounts::{self, Account, Stats, MAX_ACCOUNT_NAME, MAX_GROUPS};
use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use dotnet_pal_rs::{INVALID_ARGUMENT, OK, OS_ERROR};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::{mem::size_of, ptr};

const GUARD: u32 = 0xAAAA_AAAA;
const CHILD: &str = "PAL_STD_ACCOUNTS_CHILD";
/// One test at a time: the C library's lookups share their storage, and the counters are the process's.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
fn ops() -> &'static accounts::Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & accounts::CAP, accounts::CAP);
    &api.accounts
}
/// What the calls of this test should have added to the three counters.
static TALLY: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
fn tally(status: u32, index: usize) -> u32 { TALLY[if status == OK { index } else { 2 }].fetch_add(1, Ordering::Relaxed); status }
fn counters() -> ([u64; 3], [u64; 3]) {
    let mut out = Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<Stats>()) }, OK);
    ([out.user_ok, out.groups_ok, out.rejected_or_failed], [0, 1, 2].map(|index| TALLY[index].load(Ordering::Relaxed)))
}
/// Runs `body` and checks that the counters moved by exactly what its calls should have added.
fn counted(body: impl FnOnce()) {
    let (stats, tally) = counters();
    body();
    let (stats_after, tally_after) = counters();
    assert_eq!([0, 1, 2].map(|index| stats_after[index] - stats[index]), [0, 1, 2].map(|index| tally_after[index] - tally[index]));
}

fn image(account: &Account) -> &[u8] { unsafe { std::slice::from_raw_parts((account as *const Account).cast::<u8>(), size_of::<Account>()) } }
fn garbage() -> Account { Account { user_id: GUARD, group_id: GUARD, name: [0xAA; 256], home: [0xAA; 1024], shell: [0xAA; 256] } }
fn by_id(user_id: u32) -> (u32, Account) {
    let mut out = garbage();
    (tally(unsafe { ops().user_by_id.unwrap()(user_id, &mut out, size_of::<Account>()) }, 0), out)
}
fn by_name(name: &[u8]) -> (u32, Account) {
    let mut out = garbage();
    (tally(unsafe { ops().user_by_name.unwrap()(name.as_ptr(), name.len(), &mut out, size_of::<Account>()) }, 0), out)
}
/// Status, count and the buffer of one list: of this process without a name, of an account with one. One guard stands behind the capacity.
fn list(name: Option<&[u8]>, primary: u32, capacity: usize) -> (u32, usize, Vec<u32>) {
    let (mut count, mut buffer) = (7usize, vec![GUARD; capacity + 1]);
    let out = if capacity == 0 { ptr::null_mut() } else { buffer.as_mut_ptr() };
    let status = match name {
        Some(name) => unsafe { ops().user_groups.unwrap()(name.as_ptr(), name.len(), primary, out, capacity, &mut count) },
        None => unsafe { ops().process_groups.unwrap()(out, capacity, &mut count) },
    };
    assert_eq!(buffer.pop(), Some(GUARD));
    (tally(status, 1), count, buffer)
}

/// What the C library says about one account, copied out of its static storage; `None` for an entry the boundary's fields cannot hold.
unsafe fn known(entry: *const libc::passwd) -> Option<Account> {
    let text = |field: *const libc::c_char| if field.is_null() { &[][..] } else { unsafe { CStr::from_ptr(field) }.to_bytes() };
    let entry = unsafe { &*entry };
    Account::new(entry.pw_uid, entry.pw_gid, text(entry.pw_name), text(entry.pw_dir), text(entry.pw_shell))
}
fn c_by_id(user_id: u32) -> Option<Option<Account>> { let entry = unsafe { libc::getpwuid(user_id) }; (!entry.is_null()).then(|| unsafe { known(entry) }) }
fn c_by_name(name: &[u8]) -> Option<Option<Account>> {
    let name = CString::new(name).unwrap();
    let entry = unsafe { libc::getpwnam(name.as_ptr()) };
    (!entry.is_null()).then(|| unsafe { known(entry) })
}
fn name_of(account: &Account) -> &[u8] { CStr::from_bytes_until_nul(&account.name).unwrap().to_bytes() }
/// Every account the C library enumerates that the boundary can carry.
fn enumerated() -> Vec<Account> {
    let mut all = Vec::new();
    unsafe { libc::setpwent() };
    loop {
        let entry = unsafe { libc::getpwent() };
        if entry.is_null() { break; }
        all.extend(unsafe { known(entry) });
    }
    unsafe { libc::endpwent() };
    all
}
/// The answer is the C library's: the same account, a refusal of one too long for the boundary, or no account at all.
fn agrees(answer: (u32, Account), expected: Option<Option<Account>>) {
    match expected {
        Some(Some(account)) => { assert_eq!(answer.0, OK); assert!(image(&answer.1) == image(&account)); }
        Some(None) => { assert_eq!(answer.0, OS_ERROR); assert!(image(&answer.1) == image(&Account::EMPTY)); }
        None => { assert_eq!(answer.0, NOT_FOUND); assert!(image(&answer.1) == image(&Account::EMPTY)); }
    }
}
/// The groups getgrouplist names, in a buffer no list outgrows: macOS does not say how long a list is.
#[allow(clippy::unnecessary_cast)] // the ids of that call are `int` on Apple systems
fn c_group_list(name: &[u8], primary: u32) -> Vec<u32> {
    let name = CString::new(name).unwrap();
    let mut count = MAX_GROUPS as libc::c_int;
    #[cfg(target_vendor = "apple")]
    let mut groups = vec![0 as libc::c_int; MAX_GROUPS];
    #[cfg(not(target_vendor = "apple"))]
    let mut groups = vec![0 as libc::gid_t; MAX_GROUPS];
    assert!(unsafe { libc::getgrouplist(name.as_ptr(), primary as _, groups.as_mut_ptr(), &mut count) } >= 0);
    let mut groups: Vec<u32> = groups[..count as usize].iter().map(|group| *group as u32).collect();
    groups.sort_unstable();
    groups
}
fn c_process_groups() -> Vec<u32> {
    let mut groups = vec![0 as libc::gid_t; MAX_GROUPS];
    let count = unsafe { libc::getgroups(MAX_GROUPS as libc::c_int, groups.as_mut_ptr()) };
    assert!(count >= 0);
    groups.truncate(count as usize);
    groups.sort_unstable();
    groups
}
/// A list is `expected` in any order where it fits; its length alone, and a cleared buffer, where it does not.
fn lists(name: Option<&[u8]>, primary: u32, expected: &[u32]) {
    for capacity in [expected.len(), expected.len() + 5, MAX_GROUPS] {
        let (status, count, mut buffer) = list(name, primary, capacity);
        assert_eq!((status, count), (OK, expected.len()));
        buffer.truncate(count);
        buffer.sort_unstable();
        assert_eq!(buffer, expected);
    }
    if expected.is_empty() { return; }
    assert_eq!(list(name, primary, expected.len() - 1), (BUFFER_TOO_SMALL, expected.len(), vec![0; expected.len() - 1]));
    assert_eq!(list(name, primary, 0), (BUFFER_TOO_SMALL, expected.len(), Vec::new()));
}
/// An id nobody has.
fn unused_id() -> u32 { (54321..).find(|id| c_by_id(*id).is_none()).unwrap() }

#[test]
fn accounts_are_the_c_librarys_by_id_and_by_name() {
    let _serial = serial();
    counted(|| {
        let all = enumerated();
        assert!(!all.is_empty());
        for account in &all {
            // Two entries may share an id: by id the C library's own lookup says which one answers.
            agrees(by_id(account.user_id), c_by_id(account.user_id));
            agrees(by_name(name_of(account)), c_by_name(name_of(account)));
        }
        agrees(by_id(0), c_by_id(0));
        assert!(c_by_id(0).is_some());
        let own = unsafe { libc::geteuid() };
        agrees(by_id(own), c_by_id(own));
        agrees(by_name(b"nobody"), c_by_name(b"nobody"));
        // The name has a length and no terminator: what stands behind it is not part of it.
        let root = c_by_id(0).unwrap().unwrap();
        let mut behind = name_of(&root).to_vec();
        behind.extend_from_slice(b"XYZ");
        agrees(by_name(&behind[..behind.len() - 3]), Some(Some(root)));
    });
}
#[test]
fn unknown_accounts_are_not_found_and_names_that_are_none_are_refused() {
    let _serial = serial();
    counted(|| {
        let longest = vec![b'n'; MAX_ACCOUNT_NAME];
        assert!(c_by_name(b"pal-no-such-account").is_none() && c_by_name(&longest).is_none());
        agrees(by_id(unused_id()), None);
        agrees(by_name(b"pal-no-such-account"), None);
        // The longest name there can be reaches the provider, which knows nobody by it.
        agrees(by_name(&longest), None);
        for name in [&b""[..], &vec![b'n'; MAX_ACCOUNT_NAME + 1], b"ro\0ot"] {
            let (status, account) = by_name(name);
            assert_eq!(status, INVALID_ARGUMENT);
            assert!(image(&account) == image(&Account::EMPTY));
            assert_eq!(list(Some(name), 0, 4), (INVALID_ARGUMENT, 0, vec![GUARD; 4]));
        }
        // A name that is no account has no groups, whatever getgrouplist makes of it.
        assert_eq!(list(Some(b"pal-no-such-account"), 4242, 4), (NOT_FOUND, 0, vec![0; 4]));
        assert_eq!(list(Some(&longest), 4242, 0), (NOT_FOUND, 0, Vec::new()));
        // An answer that has no room is refused before anything is written.
        let mut out = garbage();
        assert_eq!(tally(unsafe { ops().user_by_id.unwrap()(0, &mut out, size_of::<Account>() - 1) }, 0), INVALID_ARGUMENT);
        assert_eq!(tally(unsafe { ops().user_by_name.unwrap()(b"root".as_ptr(), 4, &mut out, size_of::<Account>() - 1) }, 0), INVALID_ARGUMENT);
        assert!(image(&out) == image(&garbage()));
        assert_eq!(tally(unsafe { ops().user_by_id.unwrap()(0, ptr::null_mut(), size_of::<Account>()) }, 0), INVALID_ARGUMENT);
        assert_eq!(tally(unsafe { ops().user_by_name.unwrap()(ptr::null(), 4, &mut out, size_of::<Account>()) }, 0), INVALID_ARGUMENT);
        let (mut count, mut buffer) = (7usize, [GUARD; 4]);
        assert_eq!(tally(unsafe { ops().process_groups.unwrap()(ptr::null_mut(), 4, &mut count) }, 1), INVALID_ARGUMENT);
        assert_eq!(tally(unsafe { ops().process_groups.unwrap()(buffer.as_mut_ptr(), 4, ptr::null_mut()) }, 1), INVALID_ARGUMENT);
        assert_eq!(tally(unsafe { ops().process_groups.unwrap()(buffer.as_mut_ptr(), MAX_GROUPS + 1, &mut count) }, 1), INVALID_ARGUMENT);
        assert_eq!(tally(unsafe { ops().user_groups.unwrap()(b"root".as_ptr(), 4, 0, buffer.as_mut_ptr(), MAX_GROUPS + 1, &mut count) }, 1), INVALID_ARGUMENT);
        assert_eq!((count, buffer), (0, [GUARD; 4]));
        assert_eq!(unsafe { ops().read_stats.unwrap()(ptr::null_mut(), size_of::<Stats>()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { ops().read_stats.unwrap()(&mut Stats::default(), size_of::<Stats>() - 1) }, INVALID_ARGUMENT);
    });
}
#[test]
fn the_groups_of_this_process_are_what_getgroups_says() {
    let _serial = serial();
    counted(|| lists(None, 0, &c_process_groups()));
    // Root can ask again as a process that was given a list of the test's choosing, and as one without any group.
    if unsafe { libc::geteuid() } != 0 { println!("not root: the groups of a process with a list of the test's choosing were not checked"); return; }
    let output = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "child_mode", "--test-threads=1"]).env(CHILD, "regrouped").output().unwrap();
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
#[test]
fn child_mode() {
    if std::env::var_os(CHILD).is_none() { return; }
    let given: [libc::gid_t; 4] = [54321, 7, 65534, 12];
    assert_eq!(unsafe { libc::setgroups(given.len() as _, given.as_ptr()) }, 0);
    let mut sorted = given.to_vec();
    sorted.sort_unstable();
    assert_eq!(c_process_groups(), sorted);
    counted(|| lists(None, 0, &sorted));
    assert_eq!(unsafe { libc::setgroups(0, ptr::null()) }, 0);
    // No groups at all is an answer, and it fits into no buffer at all.
    counted(|| { assert_eq!(list(None, 0, 0), (OK, 0, Vec::new())); assert_eq!(list(None, 0, 4), (OK, 0, vec![GUARD; 4])); });
}
#[test]
fn the_groups_of_an_account_are_what_getgrouplist_says() {
    let _serial = serial();
    counted(|| {
        let all = enumerated();
        // Every account with its own primary group; the one with the longest list also with another group and with one it is a member of.
        let mut busiest: Option<(Account, Vec<u32>)> = None;
        for account in &all {
            let expected = c_group_list(name_of(account), account.group_id);
            assert!(expected.contains(&account.group_id));
            lists(Some(name_of(account)), account.group_id, &expected);
            if busiest.as_ref().is_none_or(|(_, longest)| expected.len() > longest.len()) { busiest = Some((*account, expected)); }
        }
        let (account, groups) = busiest.unwrap();
        let name = name_of(&account);
        lists(Some(name), 4242, &c_group_list(name, 4242));
        if let Some(member_of) = groups.iter().find(|group| **group != account.group_id) {
            let expected = c_group_list(name, *member_of);
            assert!(expected.len() <= groups.len());
            lists(Some(name), *member_of, &expected);
        }
        println!("longest list: {} groups of {}", groups.len(), String::from_utf8_lossy(name));
        let mut behind = name.to_vec();
        behind.extend_from_slice(b"XYZ");
        assert_eq!(list(Some(&behind[..name.len()]), account.group_id, MAX_GROUPS).1, groups.len());
    });
}
#[test]
fn two_threads_ask_at_once() {
    let _serial = serial();
    counted(|| {
        let all: Vec<(Account, usize)> = enumerated().into_iter().map(|account| (account, c_group_list(name_of(&account), account.group_id).len())).collect();
        let first_by_id: Vec<bool> = all.iter().map(|(account, _)| c_by_id(account.user_id).is_some_and(|first| first.is_some_and(|first| image(&first) == image(account)))).collect();
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| for _ in 0..5 {
                    for ((account, groups), first) in all.iter().zip(&first_by_id) {
                        let (status, found) = by_name(name_of(account));
                        assert_eq!(status, OK);
                        assert!(image(&found) == image(account));
                        let (status, found) = by_id(account.user_id);
                        assert_eq!((status, found.user_id), (OK, account.user_id));
                        assert!(!*first || image(&found) == image(account));
                        let (status, count, _) = list(Some(name_of(account)), account.group_id, 64);
                        assert_eq!((status, count), (if *groups > 64 { BUFFER_TOO_SMALL } else { OK }, *groups));
                    }
                });
            }
        });
    });
}
