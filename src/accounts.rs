//! Users and groups of the target (`CAP_ACCOUNTS`): one account by its numeric id or
//! by its name, the supplementary groups of this process and the groups of an account.
use crate::port::{Accounts, Port};
use crate::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 549755813888;
/// Longest account name a lookup takes.
pub const MAX_ACCOUNT_NAME: usize = 255;
/// Most groups one answer holds.
pub const MAX_GROUPS: usize = 65536;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Account { pub user_id: u32, pub group_id: u32, pub name: [u8; 256], pub home: [u8; 1024], pub shell: [u8; 256] }
impl Account {
    pub const EMPTY: Self = Self { user_id: 0, group_id: 0, name: [0; 256], home: [0; 1024], shell: [0; 256] };
    /// An account whose texts are cut at their first NUL; `None` when one does not fit its field with a terminator.
    pub fn new(user_id: u32, group_id: u32, name: &[u8], home: &[u8], shell: &[u8]) -> Option<Self> {
        let mut account = Self { user_id, group_id, ..Self::EMPTY };
        for (field, text) in [(&mut account.name[..], name), (&mut account.home[..], home), (&mut account.shell[..], shell)] {
            let text = &text[..text.iter().position(|b| *b == 0).unwrap_or(text.len())];
            if text.len() >= field.len() { return None; }
            field[..text.len()].copy_from_slice(text);
        }
        Some(account)
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub user_by_id: Option<unsafe extern "C" fn(u32, *mut Account, usize) -> u32>,
    pub user_by_name: Option<unsafe extern "C" fn(*const u8, usize, *mut Account, usize) -> u32>,
    pub process_groups: Option<unsafe extern "C" fn(*mut u32, usize, *mut usize) -> u32>,
    pub user_groups: Option<unsafe extern "C" fn(*const u8, usize, u32, *mut u32, usize, *mut usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub user_ok: u64, pub groups_ok: u64, pub rejected_or_failed: u64 }
const FAILED: usize = 2;
static COUNTERS: [Counter; 3] = [const { Counter::new() }; 3];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | NOT_FOUND => status,
        BUFFER_TOO_SMALL if index == 1 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { FAILED }].increment();
    status
}
/// An account name: 1..=255 bytes without NUL.
unsafe fn name<'a>(data: *const u8, length: usize) -> Option<&'a [u8]> {
    if data.is_null() || length == 0 || length > MAX_ACCOUNT_NAME || (data as usize).checked_add(length).is_none() { return None; }
    // SAFETY: a readable borrow of `length` bytes is the caller's contract.
    let bytes = unsafe { core::slice::from_raw_parts(data, length) };
    (!bytes.contains(&0)).then_some(bytes)
}
unsafe fn user(out: *mut Account, out_size: usize, asked: Option<u32>, query: impl FnOnce() -> crate::port::Result<Account>) -> u32 {
    if !aligned_output(out) || out_size < mem::size_of::<Account>() { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(Account::EMPTY) };
    match query() {
        // An account answers to the id it was asked by, has a name, and every text ends inside its field.
        Ok(a) if a.name[0] == 0 || a.name[255] != 0 || a.home[1023] != 0 || a.shell[255] != 0 || asked.is_some_and(|id| id != a.user_id) => record(OS_ERROR, 0),
        Ok(mut a) => {
            // What a provider left behind a terminator is not part of the text.
            for field in [&mut a.name[..], &mut a.home[..], &mut a.shell[..]] { let end = field.iter().position(|b| *b == 0).unwrap_or(field.len()); field[end..].fill(0); }
            unsafe { out.write(a) };
            record(OK, 0)
        }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn user_by_id<A: Accounts>(user_id: u32, out: *mut Account, out_size: usize) -> u32 {
    unsafe { user(out, out_size, Some(user_id), || A::user_by_id(user_id)) }
}
unsafe extern "C" fn user_by_name<A: Accounts>(text: *const u8, length: usize, out: *mut Account, out_size: usize) -> u32 {
    if !aligned_output(out) || out_size < mem::size_of::<Account>() { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(Account::EMPTY) };
    let Some(text) = (unsafe { name(text, length) }) else { return record(INVALID_ARGUMENT, 0); };
    unsafe { user(out, out_size, None, || A::user_by_name(text)) }
}
/// The shared shape of the two group lists: the provider writes up to `capacity` ids and returns how many there are.
unsafe fn groups(out: *mut u32, capacity: usize, count: *mut usize, query: impl FnOnce() -> crate::port::Result<usize>) -> u32 {
    if !aligned_output(count) { return record(INVALID_ARGUMENT, 1); }
    unsafe { count.write(0) };
    if capacity > MAX_GROUPS || (capacity != 0 && !aligned_output(out)) { return record(INVALID_ARGUMENT, 1); }
    let status = match query() {
        Ok(found) if found > MAX_GROUPS => OS_ERROR,
        Ok(found) => { unsafe { count.write(found) }; if found > capacity { BUFFER_TOO_SMALL } else { OK } }
        Err(e) => e.status(),
    };
    // A list that did not fit, or a call that failed, leaves no partial list behind.
    if status != OK && capacity != 0 { unsafe { core::ptr::write_bytes(out, 0, capacity) }; }
    record(status, 1)
}
unsafe extern "C" fn process_groups<A: Accounts>(out: *mut u32, capacity: usize, count: *mut usize) -> u32 {
    unsafe { groups(out, capacity, count, || A::process_groups(out, capacity)) }
}
unsafe extern "C" fn user_groups<A: Accounts>(text: *const u8, length: usize, primary_group: u32, out: *mut u32, capacity: usize, count: *mut usize) -> u32 {
    if !aligned_output(count) { return record(INVALID_ARGUMENT, 1); }
    unsafe { count.write(0) };
    let Some(text) = (unsafe { name(text, length) }) else { return record(INVALID_ARGUMENT, 1); };
    unsafe { groups(out, capacity, count, || A::user_groups(text, primary_group, out, capacity)) }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { user_ok: COUNTERS[0].load(), groups_ok: COUNTERS[1].load(), rejected_or_failed: COUNTERS[FAILED].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { user_by_id: None, user_by_name: None, process_groups: None, user_groups: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's account provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Accounts;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { user_by_id: Some(user_by_id::<T<P>>), user_by_name: Some(user_by_name::<T<P>>), process_groups: Some(process_groups::<T<P>>),
        user_groups: Some(user_groups::<T<P>>), read_stats: Some(read_stats) })
}
