//! Clock/scheduling capability. No std, allocation, managed callbacks or unwinding.
use crate::{aligned_output, Counter, INVALID_ARGUMENT, OK};
#[cfg(any(feature = "linux", feature = "host-services", feature = "wasi-clock"))]
use crate::OS_ERROR;
use core::mem;

pub const CAP_CLOCK: u64 = 4;
pub const CAP_SCHEDULER: u64 = 8;
pub const CAPABILITIES: u64 = if cfg!(any(feature = "linux", feature = "host-services")) {
    CAP_CLOCK | CAP_SCHEDULER
} else if cfg!(feature = "wasi-clock") { CAP_CLOCK } else { 0 };

#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub clock_ok: u64,
    pub sleep_ok: u64,
    pub yield_ok: u64,
    pub rejected_or_failed: u64,
}
#[repr(C)]
pub struct Ops {
    pub monotonic_ns: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub sleep_ns: Option<unsafe extern "C" fn(u64) -> u32>,
    pub yield_thread: Option<unsafe extern "C" fn() -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
static CLOCK: Counter = Counter::new();
static SLEEP: Counter = Counter::new();
static YIELD: Counter = Counter::new();
static FAILED: Counter = Counter::new();
#[cfg(any(feature = "linux", feature = "host-services", feature = "wasi-clock"))]
fn record(status: u32, counter: &Counter) -> u32 {
    // Unknown foreign status values must not escape as success or a new ABI value.
    crate::record(if status <= crate::OUT_OF_MEMORY { status } else { OS_ERROR }, counter, &FAILED)
}
#[cfg(any(feature = "linux", feature = "host-services", feature = "wasi-clock"))]
unsafe extern "C" fn monotonic_ns(out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, &CLOCK); }
    // SAFETY: writable, aligned output storage is a caller precondition.
    unsafe { out.write(0) };
    let mut value = 0;
    let status = unsafe { platform::clock(&mut value) };
    if status == OK { unsafe { out.write(value) }; }
    record(status, &CLOCK)
}
#[cfg(any(feature = "linux", feature = "host-services"))]
unsafe extern "C" fn sleep_ns(ns: u64) -> u32 {
    record(unsafe { platform::sleep(ns) }, &SLEEP)
}
#[cfg(any(feature = "linux", feature = "host-services"))]
unsafe extern "C" fn yield_thread() -> u32 {
    record(unsafe { platform::yield_now() }, &YIELD)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    // Individual diagnostic counters; not a synchronized transaction snapshot.
    unsafe { out.write(Stats {
        clock_ok: CLOCK.load(), sleep_ok: SLEEP.load(), yield_ok: YIELD.load(),
        rejected_or_failed: FAILED.load(),
    }) };
    OK
}
pub const OPS: Ops = Ops {
    #[cfg(any(feature = "linux", feature = "host-services", feature = "wasi-clock"))]
    monotonic_ns: Some(monotonic_ns),
    #[cfg(not(any(feature = "linux", feature = "host-services", feature = "wasi-clock")))]
    monotonic_ns: None,
    #[cfg(any(feature = "linux", feature = "host-services"))]
    sleep_ns: Some(sleep_ns),
    #[cfg(not(any(feature = "linux", feature = "host-services")))]
    sleep_ns: None,
    #[cfg(any(feature = "linux", feature = "host-services"))]
    yield_thread: Some(yield_thread),
    #[cfg(not(any(feature = "linux", feature = "host-services")))]
    yield_thread: None,
    read_stats: Some(read_stats),
};
pub fn available() -> bool {
    #[cfg(feature = "host-services")]
    return platform::table().is_some();
    #[cfg(not(feature = "host-services"))]
    true
}

#[cfg(feature = "linux")]
mod platform {
    use super::*;
    pub unsafe fn clock(out: &mut u64) -> u32 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return OS_ERROR;
        }
        if ts.tv_sec < 0 || !(0..1_000_000_000).contains(&ts.tv_nsec) { return OS_ERROR; }
        let Some(value) = (ts.tv_sec as u64).checked_mul(1_000_000_000)
            .and_then(|s| s.checked_add(ts.tv_nsec as u64)) else { return OS_ERROR; };
        *out = value;
        OK
    }
    pub unsafe fn sleep(ns: u64) -> u32 {
        let Ok(seconds) = (ns / 1_000_000_000).try_into() else { return INVALID_ARGUMENT; };
        let mut request = libc::timespec { tv_sec: seconds, tv_nsec: (ns % 1_000_000_000) as _ };
        loop {
            let mut remaining = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            if unsafe { libc::nanosleep(&request, &mut remaining) } == 0 { return OK; }
            if unsafe { *libc::__errno_location() } != libc::EINTR { return OS_ERROR; }
            request = remaining;
        }
    }
    pub unsafe fn yield_now() -> u32 {
        if unsafe { libc::sched_yield() } == 0 { OK } else { OS_ERROR }
    }
}

// Optional separate host extension: the existing VM host ABI never changes.
// Selecting host-services requires all three callbacks before runtime startup.
#[cfg(feature = "host-services")]
mod platform {
    use super::*;
    #[repr(C)]
    pub struct HostServices {
        header: crate::Header,
        clock: Option<unsafe extern "C" fn(*mut u64) -> u32>,
        sleep: Option<unsafe extern "C" fn(u64) -> u32>,
        yield_now: Option<unsafe extern "C" fn() -> u32>,
    }
    extern "C" { fn dotnet_pal_host_services_v2() -> *const HostServices; }
    pub fn table() -> Option<&'static HostServices> {
        let p = unsafe { dotnet_pal_host_services_v2() };
        if p.is_null() || (p as usize) % mem::align_of::<HostServices>() != 0 { return None; }
        // The host contract guarantees a readable Header even for rejected tables.
        let h = unsafe { core::ptr::addr_of!((*p).header).read() };
        if h.abi_version != crate::ABI_VERSION || (h.struct_size as usize) < mem::size_of::<HostServices>()
            || h.capabilities & CAPABILITIES != CAPABILITIES { return None; }
        // Only form the full reference AFTER checking advertised size.
        let t = unsafe { &*p };
        if t.clock.is_none() || t.sleep.is_none() || t.yield_now.is_none() { return None; }
        Some(t)
    }
    pub unsafe fn clock(out: &mut u64) -> u32 {
        match table().and_then(|t| t.clock) { Some(f) => unsafe { f(out) }, None => OS_ERROR }
    }
    pub unsafe fn sleep(ns: u64) -> u32 {
        match table().and_then(|t| t.sleep) { Some(f) => unsafe { f(ns) }, None => OS_ERROR }
    }
    pub unsafe fn yield_now() -> u32 {
        match table().and_then(|t| t.yield_now) { Some(f) => unsafe { f() }, None => OS_ERROR }
    }
}

#[cfg(feature = "wasi-clock")]
mod platform {
    use super::*;
    // WASI Preview 1: clockid monotonic=1, timestamps/precision are u64 nanoseconds.
    // No WASI libc, filesystem, environment, scheduling or allocator imports.
    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    extern "C" {
        fn clock_time_get(clock: u32, precision: u64, out: *mut u64) -> u16;
    }
    pub unsafe fn clock(out: &mut u64) -> u32 {
        if unsafe { clock_time_get(1, 1, out) } == 0 { OK } else { OS_ERROR }
    }
}
