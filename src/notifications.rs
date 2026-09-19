//! Requests from outside the process (`CAP_NOTIFICATIONS`): the interrupt key, a
//! request to quit or terminate, a lost terminal, a resumed process, a resized
//! terminal window and the job-control stops.
//!
//! The port owns the mechanism (signals, a console control handler, a button on a
//! board) and the thread the report comes from. It calls [`deliver`] for a kind the
//! consumer has enabled, from an ordinary thread: the consumer's handler may use
//! the whole boundary, which a signal or interrupt context could not allow.
use crate::kernel::BUSY;
use crate::port::{Notifications, Port};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr, sync::atomic::{AtomicPtr, AtomicU32, AtomicU8, Ordering}};

pub const CAP: u64 = 2147483648;
pub const INTERRUPT: u32 = 1;
pub const QUIT: u32 = 2;
pub const TERMINATE: u32 = 3;
pub const HANGUP: u32 = 4;
pub const CONTINUE: u32 = 5;
pub const WINDOW_CHANGE: u32 = 6;
pub const STOP_INPUT: u32 = 7;
pub const STOP_OUTPUT: u32 = 8;
pub const STOP: u32 = 9;

pub type Handler = unsafe extern "C" fn(u32, *mut c_void);
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub install: Option<unsafe extern "C" fn(Option<Handler>, *mut c_void) -> u32>,
    pub enable: Option<unsafe extern "C" fn(u32) -> u32>,
    pub disable: Option<unsafe extern "C" fn(u32) -> u32>,
    pub default_action: Option<unsafe extern "C" fn(u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
/// What a C host supplies: it reports a notification by calling the callback `start` received.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostOps {
    pub start: Option<unsafe extern "C" fn(Option<unsafe extern "C" fn(u32) -> u32>) -> u32>,
    pub enable: Option<unsafe extern "C" fn(u32) -> u32>,
    pub disable: Option<unsafe extern "C" fn(u32) -> u32>,
    pub default_action: Option<unsafe extern "C" fn(u32) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: HostOps }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub installs: u64, pub enabled: u64, pub disabled: u64, pub delivered: u64, pub dropped: u64, pub rejected: u64 }
static COUNTERS: [Counter; 6] = [const { Counter::new() }; 6];

static STATE: AtomicU8 = AtomicU8::new(0);
static HANDLER: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
static DATA: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
/// Bit `kind` is set while the consumer wants that kind.
static ENABLED: AtomicU32 = AtomicU32::new(0);

fn valid(kind: u32) -> bool { (INTERRUPT..=STOP).contains(&kind) }
/// Whether the consumer currently wants `kind`. A port may ask before it goes to
/// the trouble of switching threads.
pub fn enabled(kind: u32) -> bool { valid(kind) && ENABLED.load(Ordering::Acquire) & (1 << kind) != 0 }
/// Reports one notification to the installed handler; `false` when nobody wanted
/// it, in which case the target's default action is the port's to take. Call it
/// from an ordinary thread, never from an interrupt or signal context.
pub fn deliver(kind: u32) -> bool {
    let handler = HANDLER.load(Ordering::Acquire);
    if handler.is_null() || !enabled(kind) { COUNTERS[4].increment(); return false; }
    COUNTERS[3].increment();
    // SAFETY: only `install` stores here, and it stores a `Handler`.
    let handler: Handler = unsafe { mem::transmute::<*mut (), Handler>(handler) };
    unsafe { handler(kind, DATA.load(Ordering::Acquire)) };
    true
}
/// [`deliver`] with the C signature a host table calls: 1 when the consumer took the report.
///
/// # Safety
/// Same context rule as [`deliver`]: an ordinary thread.
pub unsafe extern "C" fn deliver_raw(kind: u32) -> u32 { deliver(kind) as u32 }

fn fold(status: u32) -> u32 { match status { OK | UNSUPPORTED | INVALID_ARGUMENT => status, _ => OS_ERROR } }
unsafe extern "C" fn install<N: Notifications>(handler: Option<Handler>, data: *mut c_void) -> u32 {
    let Some(handler) = handler else { COUNTERS[5].increment(); return INVALID_ARGUMENT; };
    if STATE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() { COUNTERS[5].increment(); return BUSY; }
    DATA.store(data, Ordering::Release);
    HANDLER.store(handler as *mut (), Ordering::Release);
    let status = fold(crate::port::status(N::start()));
    if status != OK {
        HANDLER.store(ptr::null_mut(), Ordering::Release);
        STATE.store(0, Ordering::Release);
        COUNTERS[5].increment();
        return status;
    }
    STATE.store(2, Ordering::Release);
    COUNTERS[0].increment();
    OK
}
unsafe extern "C" fn enable<N: Notifications>(kind: u32) -> u32 {
    if !valid(kind) || STATE.load(Ordering::Acquire) != 2 { COUNTERS[5].increment(); return INVALID_ARGUMENT; }
    // The consumer hears about it from the moment the port might report it: a report between the port's
    // switch and this function's return would otherwise get the old action, which for some kinds ends the process.
    let before = ENABLED.fetch_or(1 << kind, Ordering::AcqRel);
    let status = fold(crate::port::status(N::enable(kind)));
    if status == OK { COUNTERS[1].increment(); } else { if before & (1 << kind) == 0 { ENABLED.fetch_and(!(1 << kind), Ordering::AcqRel); } COUNTERS[5].increment(); }
    status
}
unsafe extern "C" fn disable<N: Notifications>(kind: u32) -> u32 {
    if !valid(kind) || STATE.load(Ordering::Acquire) != 2 { COUNTERS[5].increment(); return INVALID_ARGUMENT; }
    // The consumer stops hearing about it first, so a report that races the port's own switch is dropped.
    ENABLED.fetch_and(!(1 << kind), Ordering::AcqRel);
    let status = fold(crate::port::status(N::disable(kind)));
    COUNTERS[if status == OK { 2 } else { 5 }].increment();
    status
}
unsafe extern "C" fn default_action<N: Notifications>(kind: u32) -> u32 {
    if !valid(kind) { COUNTERS[5].increment(); return INVALID_ARGUMENT; }
    let status = fold(crate::port::status(N::default_action(kind)));
    if status != OK { COUNTERS[5].increment(); }
    status
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let c = |i: usize| COUNTERS[i].load();
    unsafe { out.write(Stats { installs: c(0), enabled: c(1), disabled: c(2), delivered: c(3), dropped: c(4), rejected: c(5) }) };
    OK
}
pub const EMPTY: Ops = Ops { install: None, enable: None, disable: None, default_action: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's notification provider.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Notifications;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { install: Some(install::<T<P>>), enable: Some(enable::<T<P>>), disable: Some(disable::<T<P>>),
        default_action: Some(default_action::<T<P>>), read_stats: Some(read_stats) })
}
