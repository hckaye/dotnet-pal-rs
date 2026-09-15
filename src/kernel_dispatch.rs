use super::*;

// Caller guarantees output storage is live, writable and disjoint from handles.
unsafe fn create(out: *mut *mut c_void, counter: usize, operation: impl FnOnce(*mut *mut c_void) -> u32) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, counter); }
    unsafe { out.write(ptr::null_mut()) };
    let mut handle = ptr::null_mut();
    let mut status = operation(&mut handle);
    if status == OK {
        if handle.is_null() { status = OS_ERROR; }
        else { unsafe { out.write(handle) }; }
    }
    // TIMEOUT/BUSY cannot be a successful or expected create result.
    if status > OUT_OF_MEMORY { status = OS_ERROR; }
    record(status, counter)
}
unsafe extern "C" fn event_create(manual: u32, initial: u32, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 0, |out| {
        if manual > 1 || initial > 1 { return INVALID_ARGUMENT; }
        platform::event_create(manual, initial, out)
    }) }
}
unsafe extern "C" fn mutex_create(recursive: u32, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 4, |out| {
        if recursive > 1 { return INVALID_ARGUMENT; }
        platform::mutex_create(recursive, out)
    }) }
}
unsafe extern "C" fn thread_create(entry: Option<Entry>, arg: *mut c_void, stack: usize, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 6, |out| {
        if entry.is_none() || stack > isize::MAX as usize { return INVALID_ARGUMENT; }
        platform::thread_create(entry, arg, stack, out)
    }) }
}
unsafe extern "C" fn tls_create(dtor: Option<Destructor>, out: *mut *mut c_void) -> u32 {
    unsafe { create(out, 7, |out| platform::tls_create(dtor, out)) }
}
macro_rules! handle_operation {
    ($name:ident, $counter:expr) => {
        unsafe extern "C" fn $name(handle: *mut c_void) -> u32 {
            if handle.is_null() { return record(INVALID_ARGUMENT, $counter); }
            record(unsafe { platform::$name(handle) }, $counter)
        }
    };
}
// Counters below are a deliberately small activity sample, not a resource ledger.
// Close/join/get/reset successes do not count as create/set/lock activities.
unsafe fn simple(handle: *mut c_void, operation: unsafe fn(*mut c_void) -> u32) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 11); }
    let status = unsafe { operation(handle) };
    if status == OK { OK } else { record(status, 11) }
}
macro_rules! lifecycle {
    ($name:ident) => { unsafe extern "C" fn $name(h: *mut c_void) -> u32 { unsafe { simple(h, platform::$name) } } };
}
lifecycle!(event_destroy); lifecycle!(event_reset);
lifecycle!(mutex_destroy); lifecycle!(mutex_unlock);
lifecycle!(thread_join); lifecycle!(thread_detach); lifecycle!(tls_destroy);
handle_operation!(event_set, 3);
handle_operation!(mutex_lock, 5);
unsafe extern "C" fn event_wait(handle: *mut c_void, timeout_ns: u64) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 1); }
    record(unsafe { platform::event_wait(handle, timeout_ns) }, 1)
}
unsafe extern "C" fn tls_set(handle: *mut c_void, value: *mut c_void) -> u32 {
    if handle.is_null() { return record(INVALID_ARGUMENT, 8); }
    record(unsafe { platform::tls_set(handle, value) }, 8)
}
unsafe extern "C" fn tls_get(handle: *mut c_void, out: *mut *mut c_void) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 11); }
    unsafe { out.write(ptr::null_mut()) };
    if handle.is_null() { return record(INVALID_ARGUMENT, 11); }
    let mut result = ptr::null_mut();
    let status = unsafe { platform::tls_get(handle, &mut result) };
    if status == OK { unsafe { out.write(result) }; OK } else { record(status, 11) }
}
unsafe extern "C" fn stack_bounds(low: *mut *mut c_void, high: *mut *mut c_void) -> u32 {
    if !aligned_output(low) || !aligned_output(high) || low == high { return record(INVALID_ARGUMENT, 9); }
    unsafe { low.write(ptr::null_mut()); high.write(ptr::null_mut()); }
    let mut lo = ptr::null_mut(); let mut hi = ptr::null_mut();
    let mut status = unsafe { platform::stack_bounds(&mut lo, &mut hi) };
    if status == OK {
        if lo.is_null() || (lo as usize) >= (hi as usize) { status = OS_ERROR; }
        else { unsafe { low.write(lo); high.write(hi); } }
    }
    record(status, 9)
}
unsafe extern "C" fn process_barrier() -> u32 { record(unsafe { platform::process_barrier() }, 10) }
pub const OPS: Ops = Ops {
    event_create: Some(event_create), event_destroy: Some(event_destroy), event_set: Some(event_set),
    event_reset: Some(event_reset), event_wait: Some(event_wait),
    mutex_create: Some(mutex_create), mutex_destroy: Some(mutex_destroy), mutex_lock: Some(mutex_lock), mutex_unlock: Some(mutex_unlock),
    thread_create: Some(thread_create), thread_join: Some(thread_join), thread_detach: Some(thread_detach),
    tls_create: Some(tls_create), tls_destroy: Some(tls_destroy), tls_get: Some(tls_get), tls_set: Some(tls_set),
    stack_bounds: Some(stack_bounds), process_barrier: Some(process_barrier), read_stats: Some(read_stats),
};
