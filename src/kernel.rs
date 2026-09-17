//! Synchronization, thread and TLS contracts. Handles are native-owned opaque values.
//! Close/join/delete require exclusive ownership; cancellation and managed reentry
//! across callbacks are forbidden. This is not a sandbox for arbitrary C pointers.
use crate::port::{self, Events, Mutexes, Port, ProcessBarrier, StackBounds, ThreadLocal, Threads};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY};
use core::{ffi::c_void, mem, ptr};

pub const CAP_EVENTS: u64 = 16;
pub const CAP_MUTEX: u64 = 32;
pub const CAP_THREADS: u64 = 64;
pub const CAP_TLS: u64 = 128;
pub const CAP_STACK: u64 = 256;
pub const CAP_BARRIER: u64 = 512;
pub const ALL: u64 = CAP_EVENTS | CAP_MUTEX | CAP_THREADS | CAP_TLS | CAP_STACK | CAP_BARRIER;
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

fn record(status: u32, counter: usize) -> u32 {
    let status = if status <= BUSY { status } else { OS_ERROR };
    let status = if (status == TIMEOUT && counter != 1) || (status == BUSY && counter != 11) { OS_ERROR } else { status };
    if status == OK { COUNTERS[counter].increment(); }
    else if status == TIMEOUT { COUNTERS[2].increment(); }
    else { COUNTERS[11].increment(); }
    status
}
// Caller guarantees output storage is live, writable and disjoint from handles.
unsafe fn create(out: *mut *mut c_void, counter: usize, operation: impl FnOnce() -> port::Result<*mut c_void>) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, counter); }
    unsafe { out.write(ptr::null_mut()) };
    let mut status = match operation() {
        Ok(handle) if handle.is_null() => OS_ERROR,
        Ok(handle) => { unsafe { out.write(handle) }; OK }
        Err(e) => e.status(),
    };
    // TIMEOUT/BUSY cannot be a successful or expected create result.
    if status > OUT_OF_MEMORY { status = OS_ERROR; }
    record(status, counter)
}
unsafe extern "C" fn event_create<E: Events>(manual: u32, initial: u32, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 0, || {
        if manual > 1 || initial > 1 { return Err(port::Error::InvalidArgument); }
        E::create(manual != 0, initial != 0)
    }) }
}
unsafe extern "C" fn mutex_create<M: Mutexes>(recursive: u32, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 4, || {
        if recursive > 1 { return Err(port::Error::InvalidArgument); }
        M::create(recursive != 0)
    }) }
}
unsafe extern "C" fn thread_create<T: Threads>(entry: Option<Entry>, arg: *mut c_void, stack: usize, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 6, || {
        let Some(entry) = entry else { return Err(port::Error::InvalidArgument); };
        if stack > isize::MAX as usize { return Err(port::Error::InvalidArgument); }
        T::create(entry, arg, stack)
    }) }
}
unsafe extern "C" fn tls_create<T: ThreadLocal>(dtor: Option<Destructor>, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 7, || T::create(dtor)) }
}
// Counters below are a deliberately small activity sample, not a resource ledger.
// Close/join/get/reset successes do not count as create/set/lock activities.
unsafe fn simple(handle: *mut c_void, operation: impl FnOnce(*mut c_void) -> port::Result<()>) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 11); }
    match operation(handle) { Ok(()) => OK, Err(e) => record(e.status(), 11) }
}
unsafe extern "C" fn event_destroy<E: Events>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| E::destroy(h)) } }
unsafe extern "C" fn event_reset<E: Events>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| E::reset(h)) } }
unsafe extern "C" fn mutex_destroy<M: Mutexes>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| M::destroy(h)) } }
unsafe extern "C" fn mutex_unlock<M: Mutexes>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| M::unlock(h)) } }
unsafe extern "C" fn thread_join<T: Threads>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| T::join(h)) } }
unsafe extern "C" fn thread_detach<T: Threads>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| T::detach(h)) } }
unsafe extern "C" fn tls_destroy<T: ThreadLocal>(h: *mut c_void) -> u32 { unsafe { simple(h, |h| T::destroy(h)) } }
unsafe extern "C" fn event_set<E: Events>(h: *mut c_void) -> u32 {
    if h.is_null() { return record(INVALID_ARGUMENT, 3); }
    record(port::status(unsafe { E::set(h) }), 3)
}
unsafe extern "C" fn mutex_lock<M: Mutexes>(h: *mut c_void) -> u32 {
    if h.is_null() { return record(INVALID_ARGUMENT, 5); }
    record(port::status(unsafe { M::lock(h) }), 5)
}
unsafe extern "C" fn event_wait<E: Events>(handle: *mut c_void, timeout_ns: u64) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 1); }
    record(port::status(unsafe { E::wait(handle, timeout_ns) }), 1)
}
unsafe extern "C" fn tls_set<T: ThreadLocal>(handle: *mut c_void, value: *mut c_void) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 8); }
    record(port::status(unsafe { T::set(handle, value) }), 8)
}
unsafe extern "C" fn tls_get<T: ThreadLocal>(handle: *mut c_void, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 11); }
    unsafe { out.write(ptr::null_mut()) };
    if handle.is_null() { return record(INVALID_ARGUMENT, 11); }
    match unsafe { T::get(handle) } {
        Ok(value) => { unsafe { out.write(value) }; OK }
        Err(e) => record(e.status(), 11),
    }
}
unsafe extern "C" fn stack_bounds<S: StackBounds>(low: *mut *mut c_void, high: *mut *mut c_void) -> u32 {
    if !aligned_output(low) || !aligned_output(high) || low == high { return record(INVALID_ARGUMENT, 9); }
    unsafe { low.write(ptr::null_mut()); high.write(ptr::null_mut()); }
    let status = match S::current() {
        Ok((lo, hi)) => {
            if lo.is_null() || (lo as usize) >= (hi as usize) { OS_ERROR }
            else { unsafe { low.write(lo); high.write(hi); } OK }
        }
        Err(e) => e.status(),
    };
    record(status, 9)
}
unsafe extern "C" fn process_barrier<B: ProcessBarrier>() -> u32 { record(port::status(B::barrier()), 10) }
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
/// Capability bits and callbacks for the port's kernel providers. The process
/// barrier is probed once here; an unavailable barrier clears its bit.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    let mut caps = 0;
    let mut ops = EMPTY;
    if P::Events::PROVIDED {
        caps |= CAP_EVENTS;
        ops.event_create = Some(event_create::<P::Events>); ops.event_destroy = Some(event_destroy::<P::Events>);
        ops.event_set = Some(event_set::<P::Events>); ops.event_reset = Some(event_reset::<P::Events>);
        ops.event_wait = Some(event_wait::<P::Events>);
    }
    if P::Mutexes::PROVIDED {
        caps |= CAP_MUTEX;
        ops.mutex_create = Some(mutex_create::<P::Mutexes>); ops.mutex_destroy = Some(mutex_destroy::<P::Mutexes>);
        ops.mutex_lock = Some(mutex_lock::<P::Mutexes>); ops.mutex_unlock = Some(mutex_unlock::<P::Mutexes>);
    }
    if P::Threads::PROVIDED {
        caps |= CAP_THREADS;
        ops.thread_create = Some(thread_create::<P::Threads>); ops.thread_join = Some(thread_join::<P::Threads>);
        ops.thread_detach = Some(thread_detach::<P::Threads>);
    }
    if P::ThreadLocal::PROVIDED {
        caps |= CAP_TLS;
        ops.tls_create = Some(tls_create::<P::ThreadLocal>); ops.tls_destroy = Some(tls_destroy::<P::ThreadLocal>);
        ops.tls_get = Some(tls_get::<P::ThreadLocal>); ops.tls_set = Some(tls_set::<P::ThreadLocal>);
    }
    if P::StackBounds::PROVIDED { caps |= CAP_STACK; ops.stack_bounds = Some(stack_bounds::<P::StackBounds>); }
    if P::ProcessBarrier::PROVIDED && P::ProcessBarrier::available() {
        caps |= CAP_BARRIER; ops.process_barrier = Some(process_barrier::<P::ProcessBarrier>);
    }
    (caps, ops)
}
