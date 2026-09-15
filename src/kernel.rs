//! Synchronization, thread and TLS contracts. Handles are native-owned opaque values.
//! Close/join/delete require exclusive ownership; cancellation and managed reentry
//! across callbacks are forbidden. This is not a sandbox for arbitrary C pointers.
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK};
use core::{ffi::c_void, mem};
#[cfg(any(feature = "linux", feature = "host-kernel"))]
use crate::{OS_ERROR, OUT_OF_MEMORY};
#[cfg(any(feature = "linux", feature = "host-kernel"))]
use core::ptr;

pub const CAP_EVENTS: u64 = 16;
pub const CAP_MUTEX: u64 = 32;
pub const CAP_THREADS: u64 = 64;
pub const CAP_TLS: u64 = 128;
pub const CAP_STACK: u64 = 256;
pub const CAP_BARRIER: u64 = 512;
pub const ALL: u64 = CAP_EVENTS | CAP_MUTEX | CAP_THREADS | CAP_TLS | CAP_STACK | CAP_BARRIER;
pub const CAPABILITIES: u64 = if cfg!(any(feature = "linux", feature = "host-kernel")) { ALL } else { 0 };
pub const TIMEOUT: u32 = 5;
pub const BUSY: u32 = 6;
pub const INFINITE: u64 = u64::MAX;
pub type Entry = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
pub type Destructor = unsafe extern "C" fn(*mut c_void);
pub type HandleOp = unsafe extern "C" fn(*mut c_void) -> u32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub event_create: Option<unsafe extern "C" fn(u32, u32, *mut *mut c_void) -> u32>,
    pub event_destroy: Option<HandleOp>,
    pub event_set: Option<HandleOp>,
    pub event_reset: Option<HandleOp>,
    pub event_wait: Option<unsafe extern "C" fn(*mut c_void, u64) -> u32>,
    pub mutex_create: Option<unsafe extern "C" fn(u32, *mut *mut c_void) -> u32>,
    pub mutex_destroy: Option<HandleOp>,
    pub mutex_lock: Option<HandleOp>,
    pub mutex_unlock: Option<HandleOp>,
    pub thread_create: Option<unsafe extern "C" fn(Option<Entry>, *mut c_void, usize, *mut *mut c_void) -> u32>,
    pub thread_join: Option<HandleOp>,
    pub thread_detach: Option<HandleOp>,
    pub tls_create: Option<unsafe extern "C" fn(Option<Destructor>, *mut *mut c_void) -> u32>,
    pub tls_destroy: Option<HandleOp>,
    pub tls_get: Option<unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> u32>,
    pub tls_set: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32>,
    pub stack_bounds: Option<unsafe extern "C" fn(*mut *mut c_void, *mut *mut c_void) -> u32>,
    pub process_barrier: Option<unsafe extern "C" fn() -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct HostKernel { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub event_create_ok: u64, pub event_wait_ok: u64, pub event_timeout: u64,
    pub event_set_ok: u64, pub mutex_create_ok: u64, pub mutex_lock_ok: u64,
    pub thread_create_ok: u64, pub tls_create_ok: u64, pub tls_set_ok: u64, pub stack_bounds_ok: u64,
    pub barrier_ok: u64, pub rejected_or_failed: u64,
}
static COUNTERS: [Counter; 12] = [const { Counter::new() }; 12];

#[cfg(any(feature = "linux", feature = "host-kernel"))]
#[path = "kernel_dispatch.rs"]
mod dispatch;
#[cfg(feature = "linux")]
#[path = "kernel_linux.rs"]
mod platform;
#[cfg(feature = "host-kernel")]
#[path = "kernel_host.rs"]
mod platform;

#[cfg(any(feature = "linux", feature = "host-kernel"))]
fn record(status: u32, counter: usize) -> u32 {
    let status = if status <= BUSY { status } else { OS_ERROR };
    let status = if (status == TIMEOUT && counter != 1) || (status == BUSY && counter != 11) { OS_ERROR } else { status };
    if status == OK { COUNTERS[counter].increment(); }
    else if status == TIMEOUT { COUNTERS[2].increment(); }
    else { COUNTERS[11].increment(); }
    status
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats {
        event_create_ok: COUNTERS[0].load(), event_wait_ok: COUNTERS[1].load(),
        event_timeout: COUNTERS[2].load(), event_set_ok: COUNTERS[3].load(),
        mutex_create_ok: COUNTERS[4].load(), mutex_lock_ok: COUNTERS[5].load(),
        thread_create_ok: COUNTERS[6].load(), tls_create_ok: COUNTERS[7].load(), tls_set_ok: COUNTERS[8].load(),
        stack_bounds_ok: COUNTERS[9].load(), barrier_ok: COUNTERS[10].load(),
        rejected_or_failed: COUNTERS[11].load(),
    }) };
    OK
}
pub const EMPTY: Ops = Ops {
    event_create: None, event_destroy: None, event_set: None, event_reset: None, event_wait: None,
    mutex_create: None, mutex_destroy: None, mutex_lock: None, mutex_unlock: None,
    thread_create: None, thread_join: None, thread_detach: None,
    tls_create: None, tls_destroy: None, tls_get: None, tls_set: None,
    stack_bounds: None, process_barrier: None, read_stats: Some(read_stats),
};
#[cfg(any(feature = "linux", feature = "host-kernel"))]
pub const OPS: Ops = dispatch::OPS;
#[cfg(not(any(feature = "linux", feature = "host-kernel")))]
pub const OPS: Ops = EMPTY;
pub const OPS_NO_BARRIER: Ops = Ops { process_barrier: None, ..OPS };
pub fn available() -> bool {
    #[cfg(feature = "host-kernel")]
    return platform::table().is_some();
    #[cfg(not(feature = "host-kernel"))]
    true
}
pub fn has_barrier() -> bool {
    #[cfg(feature = "linux")]
    return platform::has_barrier();
    #[cfg(not(feature = "linux"))]
    cfg!(feature = "host-kernel")
}
