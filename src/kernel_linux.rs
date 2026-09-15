//! Linux implementation. No Rust heap allocator; native resources use the CRT.
//! Never move a pthread object after initialization and never form Rust references
//! covering storage concurrently mutated by pthread functions.
use super::*;
use core::sync::atomic::{AtomicU8, Ordering};

#[repr(C)]
struct Event { mutex: libc::pthread_mutex_t, cond: libc::pthread_cond_t, manual: bool, signaled: bool, waiters: usize }
#[repr(C)]
struct Mutex { native: libc::pthread_mutex_t }
#[repr(C)]
struct Thread { native: libc::pthread_t }
#[repr(C)]
struct Tls { native: libc::pthread_key_t }
struct Start { entry: Entry, arg: *mut c_void }

fn status(error: i32) -> u32 {
    match error {
        0 => OK, libc::ENOMEM | libc::EAGAIN => OUT_OF_MEMORY,
        libc::EINVAL => INVALID_ARGUMENT, libc::ETIMEDOUT => TIMEOUT,
        libc::EBUSY => BUSY, _ => OS_ERROR,
    }
}
unsafe fn alloc<T>() -> *mut T { unsafe { libc::calloc(1, mem::size_of::<T>()).cast() } }
fn handle<T>(p: *mut c_void) -> Option<*mut T> {
    if p.is_null() || (p as usize) % mem::align_of::<T>() != 0 { None } else { Some(p.cast()) }
}

pub unsafe fn event_create(manual: u32, initial: u32, out: *mut *mut c_void) -> u32 {
    let p = unsafe { alloc::<Event>() };
    if p.is_null() { return OUT_OF_MEMORY; }
    let mut attr = mem::MaybeUninit::<libc::pthread_condattr_t>::uninit();
    let mut rc = unsafe { libc::pthread_condattr_init(attr.as_mut_ptr()) };
    if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc); }
    rc = unsafe { libc::pthread_condattr_setclock(attr.as_mut_ptr(), libc::CLOCK_MONOTONIC) };
    if rc != 0 {
        unsafe { libc::pthread_condattr_destroy(attr.as_mut_ptr()); libc::free(p.cast()); }
        return status(rc);
    }
    rc = unsafe { libc::pthread_mutex_init(ptr::addr_of_mut!((*p).mutex), ptr::null()) };
    if rc != 0 {
        unsafe { libc::pthread_condattr_destroy(attr.as_mut_ptr()); libc::free(p.cast()); }
        return status(rc);
    }
    rc = unsafe { libc::pthread_cond_init(ptr::addr_of_mut!((*p).cond), attr.as_ptr()) };
    unsafe { libc::pthread_condattr_destroy(attr.as_mut_ptr()); }
    if rc != 0 {
        unsafe { libc::pthread_mutex_destroy(ptr::addr_of_mut!((*p).mutex)); libc::free(p.cast()); }
        return status(rc);
    }
    unsafe { (*p).manual = manual != 0; (*p).signaled = initial != 0; out.write(p.cast()); }
    OK
}
pub unsafe fn event_destroy(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Event>(h) else { return INVALID_ARGUMENT; };
    let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
    let rc = unsafe { libc::pthread_mutex_lock(mutex) };
    if rc != 0 { return status(rc); }
    if unsafe { (*p).waiters != 0 } {
        unsafe { libc::pthread_mutex_unlock(mutex); }
        return BUSY;
    }
    // Caller has exclusive lifecycle ownership, so no new waiter may enter here.
    let rc = unsafe { libc::pthread_cond_destroy(ptr::addr_of_mut!((*p).cond)) };
    let unlock = unsafe { libc::pthread_mutex_unlock(mutex) };
    if rc != 0 { return status(rc); }
    if unlock != 0 { return status(unlock); }
    let rc = unsafe { libc::pthread_mutex_destroy(mutex) };
    if rc == 0 { unsafe { libc::free(h) }; }
    status(rc)
}
pub unsafe fn event_set(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Event>(h) else { return INVALID_ARGUMENT; };
    let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
    let rc = unsafe { libc::pthread_mutex_lock(mutex) };
    if rc != 0 { return status(rc); }
    unsafe { (*p).signaled = true; }
    let rc = unsafe {
        if (*p).manual { libc::pthread_cond_broadcast(ptr::addr_of_mut!((*p).cond)) }
        else { libc::pthread_cond_signal(ptr::addr_of_mut!((*p).cond)) }
    };
    let unlock = unsafe { libc::pthread_mutex_unlock(mutex) };
    status(if rc != 0 { rc } else { unlock })
}
pub unsafe fn event_reset(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Event>(h) else { return INVALID_ARGUMENT; };
    let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
    let rc = unsafe { libc::pthread_mutex_lock(mutex) };
    if rc != 0 { return status(rc); }
    unsafe { (*p).signaled = false; }
    status(unsafe { libc::pthread_mutex_unlock(mutex) })
}
fn deadline(ns: u64) -> Result<libc::timespec, u32> {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 { return Err(OS_ERROR); }
    if now.tv_sec < 0 || !(0..1_000_000_000).contains(&now.tv_nsec) { return Err(OS_ERROR); }
    let nanos = now.tv_nsec as u64 + ns % 1_000_000_000;
    let seconds = (now.tv_sec as u64).checked_add(ns / 1_000_000_000)
        .and_then(|s| s.checked_add(nanos / 1_000_000_000)).ok_or(INVALID_ARGUMENT)?;
    let seconds = seconds.try_into().map_err(|_| INVALID_ARGUMENT)?;
    Ok(libc::timespec { tv_sec: seconds, tv_nsec: (nanos % 1_000_000_000) as _ })
}
pub unsafe fn event_wait(h: *mut c_void, ns: u64) -> u32 {
    let Some(p) = handle::<Event>(h) else { return INVALID_ARGUMENT; };
    let end = if ns != 0 && ns != INFINITE {
        match deadline(ns) { Ok(d) => d, Err(e) => return e }
    } else { libc::timespec { tv_sec: 0, tv_nsec: 0 } };
    let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
    let cond = unsafe { ptr::addr_of_mut!((*p).cond) };
    let mut rc = unsafe { libc::pthread_mutex_lock(mutex) };
    if rc != 0 { return status(rc); }
    unsafe { (*p).waiters += 1; }
    while !unsafe { (*p).signaled } {
        if ns == 0 { rc = libc::ETIMEDOUT; break; }
        rc = unsafe {
            if ns == INFINITE { libc::pthread_cond_wait(cond, mutex) }
            else { libc::pthread_cond_timedwait(cond, mutex, &end) }
        };
        if rc != 0 { break; } // spurious wakes recheck state against the SAME deadline
    }
    // A signal may have raced with timeout before the mutex was reacquired.
    if (rc == 0 || rc == libc::ETIMEDOUT) && unsafe { (*p).signaled } {
        rc = 0;
        if !unsafe { (*p).manual } { unsafe { (*p).signaled = false; } }
    }
    unsafe { (*p).waiters -= 1; }
    let unlock = unsafe { libc::pthread_mutex_unlock(mutex) };
    status(if unlock != 0 { unlock } else { rc })
}

pub unsafe fn mutex_create(recursive: u32, out: *mut *mut c_void) -> u32 {
    let p = unsafe { alloc::<Mutex>() };
    if p.is_null() { return OUT_OF_MEMORY; }
    let mut attr = mem::MaybeUninit::<libc::pthread_mutexattr_t>::uninit();
    let mut rc = unsafe { libc::pthread_mutexattr_init(attr.as_mut_ptr()) };
    if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc); }
    let kind = if recursive != 0 { libc::PTHREAD_MUTEX_RECURSIVE } else { libc::PTHREAD_MUTEX_ERRORCHECK };
    rc = unsafe { libc::pthread_mutexattr_settype(attr.as_mut_ptr(), kind) };
    if rc == 0 { rc = unsafe { libc::pthread_mutex_init(ptr::addr_of_mut!((*p).native), attr.as_ptr()) }; }
    unsafe { libc::pthread_mutexattr_destroy(attr.as_mut_ptr()); }
    if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc); }
    unsafe { out.write(p.cast()) };
    OK
}
pub unsafe fn mutex_destroy(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Mutex>(h) else { return INVALID_ARGUMENT; };
    let rc = unsafe { libc::pthread_mutex_destroy(ptr::addr_of_mut!((*p).native)) };
    if rc == 0 { unsafe { libc::free(h) }; }
    status(rc)
}
pub unsafe fn mutex_lock(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Mutex>(h) else { return INVALID_ARGUMENT; };
    status(unsafe { libc::pthread_mutex_lock(ptr::addr_of_mut!((*p).native)) })
}
pub unsafe fn mutex_unlock(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Mutex>(h) else { return INVALID_ARGUMENT; };
    status(unsafe { libc::pthread_mutex_unlock(ptr::addr_of_mut!((*p).native)) })
}
extern "C" fn thread_start(data: *mut c_void) -> *mut c_void {
    unsafe {
        let start = ptr::read(data.cast::<Start>());
        libc::free(data);
        (start.entry)(start.arg)
    }
}
pub unsafe fn thread_create(entry: Option<Entry>, arg: *mut c_void, stack: usize, out: *mut *mut c_void) -> u32 {
    let Some(entry) = entry else { return INVALID_ARGUMENT; };
    let handle = unsafe { alloc::<Thread>() };
    if handle.is_null() { return OUT_OF_MEMORY; }
    let start = unsafe { alloc::<Start>() };
    if start.is_null() { unsafe { libc::free(handle.cast()) }; return OUT_OF_MEMORY; }
    unsafe { start.write(Start { entry, arg }); }
    let mut attr = mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    let mut rc = unsafe { libc::pthread_attr_init(attr.as_mut_ptr()) };
    if rc == 0 {
        if stack != 0 { rc = unsafe { libc::pthread_attr_setstacksize(attr.as_mut_ptr(), stack) }; }
        if rc == 0 {
            rc = unsafe { libc::pthread_create(ptr::addr_of_mut!((*handle).native), attr.as_ptr(), thread_start, start.cast()) };
        }
        unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()); }
    }
    if rc != 0 {
        unsafe { libc::free(start.cast()); libc::free(handle.cast()); }
        return status(rc);
    }
    // The worker owns start now and may already have freed it. Do not access it.
    unsafe { out.write(handle.cast()) };
    OK
}
pub unsafe fn thread_join(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Thread>(h) else { return INVALID_ARGUMENT; };
    let rc = unsafe { libc::pthread_join((*p).native, ptr::null_mut()) };
    if rc == 0 { unsafe { libc::free(h) }; }
    status(rc)
}
pub unsafe fn thread_detach(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Thread>(h) else { return INVALID_ARGUMENT; };
    let rc = unsafe { libc::pthread_detach((*p).native) };
    if rc == 0 { unsafe { libc::free(h) }; }
    status(rc)
}
pub unsafe fn tls_create(dtor: Option<Destructor>, out: *mut *mut c_void) -> u32 {
    let p = unsafe { alloc::<Tls>() };
    if p.is_null() { return OUT_OF_MEMORY; }
    let rc = unsafe { libc::pthread_key_create(ptr::addr_of_mut!((*p).native), dtor) };
    if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc); }
    unsafe { out.write(p.cast()) }; OK
}
pub unsafe fn tls_destroy(h: *mut c_void) -> u32 {
    let Some(p) = handle::<Tls>(h) else { return INVALID_ARGUMENT; };
    let rc = unsafe { libc::pthread_key_delete((*p).native) };
    if rc == 0 { unsafe { libc::free(h) }; }
    status(rc)
}
pub unsafe fn tls_get(h: *mut c_void, out: *mut *mut c_void) -> u32 {
    let Some(p) = handle::<Tls>(h) else { return INVALID_ARGUMENT; };
    unsafe { out.write(libc::pthread_getspecific((*p).native)) }; OK
}
pub unsafe fn tls_set(h: *mut c_void, value: *mut c_void) -> u32 {
    let Some(p) = handle::<Tls>(h) else { return INVALID_ARGUMENT; };
    status(unsafe { libc::pthread_setspecific((*p).native, value) })
}
pub unsafe fn stack_bounds(low: *mut *mut c_void, high: *mut *mut c_void) -> u32 {
    let mut attr = mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    let rc = unsafe { libc::pthread_getattr_np(libc::pthread_self(), attr.as_mut_ptr()) };
    if rc != 0 { return status(rc); }
    let mut base = ptr::null_mut(); let mut size = 0;
    let rc = unsafe { libc::pthread_attr_getstack(attr.as_ptr(), &mut base, &mut size) };
    unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()); }
    if rc != 0 { return status(rc); }
    let Some(end) = (base as usize).checked_add(size) else { return OS_ERROR; };
    if base.is_null() || size == 0 { return OS_ERROR; }
    unsafe { low.write(base); high.write(end as *mut c_void); }; OK
}

// Linux UAPI membarrier.h. This is a process-wide barrier, NOT a local atomic fence.
const QUERY: i32 = 0;
const GLOBAL: i32 = 1;
const PRIVATE_EXPEDITED: i32 = 8;
const REGISTER_PRIVATE_EXPEDITED: i32 = 16;
static BARRIER_MODE: AtomicU8 = AtomicU8::new(0); // unknown / unavailable / private / global
fn barrier_mode() -> u8 {
    let cached = BARRIER_MODE.load(Ordering::Acquire);
    if cached != 0 { return cached; }
    let commands = unsafe { libc::syscall(libc::SYS_membarrier, QUERY, 0) };
    let mode = if commands < 0 { 1 }
    else if commands & (PRIVATE_EXPEDITED | REGISTER_PRIVATE_EXPEDITED) as libc::c_long == (PRIVATE_EXPEDITED | REGISTER_PRIVATE_EXPEDITED) as libc::c_long
        && unsafe { libc::syscall(libc::SYS_membarrier, REGISTER_PRIVATE_EXPEDITED, 0) } == 0 { 2 }
    else if commands & GLOBAL as libc::c_long != 0 { 3 }
    else { 1 };
    match BARRIER_MODE.compare_exchange(0, mode, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => mode, Err(actual) => actual,
    }
}
pub fn has_barrier() -> bool { barrier_mode() >= 2 }
pub unsafe fn process_barrier() -> u32 {
    let cmd = match barrier_mode() { 2 => PRIVATE_EXPEDITED, 3 => GLOBAL, _ => return crate::UNSUPPORTED };
    if unsafe { libc::syscall(libc::SYS_membarrier, cmd, 0) } == 0 { OK } else { OS_ERROR }
}
