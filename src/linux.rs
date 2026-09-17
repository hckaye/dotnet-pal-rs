//! Reference Linux providers, not a generic Unix or console implementation.
//! No Rust heap allocator; native resources use the CRT. Never move a pthread
//! object after initialization and never form Rust references covering storage
//! concurrently mutated by pthread functions.
use crate::port::{self, Error, Lookup, Result};
use crate::kernel::{Destructor, Entry, INFINITE};
use crate::runtime::{ModuleInfo, EXECUTE, MAX_NAME, READ, WRITE};
use core::{ffi::{c_void, CStr}, mem, ptr, sync::atomic::{AtomicU8, AtomicUsize, Ordering}};

pub struct Linux;
fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn status(error: i32) -> Result<()> {
    match error {
        0 => Ok(()), libc::ENOMEM | libc::EAGAIN => Err(Error::OutOfMemory),
        libc::EINVAL => Err(Error::InvalidArgument), libc::ETIMEDOUT => Err(Error::Timeout),
        libc::EBUSY => Err(Error::Busy), _ => Err(Error::Os),
    }
}
fn os(result: i32) -> Result<()> { if result == 0 { Ok(()) } else { Err(Error::Os) } }
unsafe fn alloc<T>() -> *mut T { unsafe { libc::calloc(1, mem::size_of::<T>()).cast() } }
fn handle<T>(p: *mut c_void) -> Result<*mut T> {
    if p.is_null() || (p as usize) % mem::align_of::<T>() != 0 { Err(Error::InvalidArgument) } else { Ok(p.cast()) }
}
fn page() -> usize {
    let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if value > 0 { value as usize } else { 0 }
}

impl port::VirtualMemory for Linux {
    fn page_size() -> usize { page() }
    unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void> {
        let page = page();
        let Some(total) = size.checked_add(alignment - page) else { return Err(Error::Os); };
        let raw = unsafe { libc::mmap(ptr::null_mut(), total, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
        if raw == libc::MAP_FAILED { return Err(Error::Os); }
        let Some(rounded) = (raw as usize).checked_add(alignment - 1) else {
            unsafe { libc::munmap(raw, total) }; return Err(Error::Os);
        };
        let aligned = rounded & !(alignment - 1);
        let prefix = aligned - raw as usize;
        let suffix = total - prefix - size;
        if prefix != 0 && unsafe { libc::munmap(raw, prefix) } != 0 {
            unsafe { libc::munmap(raw, total) }; return Err(Error::Os);
        }
        if suffix != 0 && unsafe { libc::munmap((aligned + size) as *mut c_void, suffix) } != 0 {
            unsafe { libc::munmap(aligned as *mut c_void, size + suffix) }; return Err(Error::Os);
        }
        // Initial PROT_NONE mapping owns the address range but not a physical-memory promise.
        Ok(aligned as *mut c_void)
    }
    unsafe fn commit(address: *mut c_void, size: usize) -> Result<()> {
        // Re-committing already committed pages preserves data.
        os(unsafe { libc::mprotect(address, size, libc::PROT_READ | libc::PROT_WRITE) })
    }
    unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()> {
        // Replace ONLY a caller-owned reservation. Fresh anonymous pages guarantee
        // zeroes after recommit; mprotect alone would leave the old contents behind.
        let result = unsafe { libc::mmap(address, size, libc::PROT_NONE,
            libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
        if result == libc::MAP_FAILED { Err(Error::Os) } else { Ok(()) }
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> { os(unsafe { libc::munmap(address, size) }) }
    unsafe fn reset(address: *mut c_void, size: usize) -> Result<()> {
        // Contents may be discarded, but the pages stay accessible. Distinct from decommit.
        os(unsafe { libc::madvise(address, size, libc::MADV_DONTNEED) })
    }
}
impl port::Abort for Linux { fn abort() -> ! { unsafe { libc::abort() } } }

impl port::Clock for Linux {
    fn monotonic_ns() -> Result<u64> {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 { return Err(Error::Os); }
        if ts.tv_sec < 0 || !(0..1_000_000_000).contains(&ts.tv_nsec) { return Err(Error::Os); }
        (ts.tv_sec as u64).checked_mul(1_000_000_000).and_then(|s| s.checked_add(ts.tv_nsec as u64)).ok_or(Error::Os)
    }
}
impl port::Scheduler for Linux {
    fn sleep_ns(ns: u64) -> Result<()> {
        let Ok(seconds) = (ns / 1_000_000_000).try_into() else { return Err(Error::InvalidArgument); };
        let mut request = libc::timespec { tv_sec: seconds, tv_nsec: (ns % 1_000_000_000) as _ };
        loop {
            let mut remaining = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            if unsafe { libc::nanosleep(&request, &mut remaining) } == 0 { return Ok(()); }
            if errno() != libc::EINTR { return Err(Error::Os); }
            request = remaining;
        }
    }
    fn yield_now() -> Result<()> { os(unsafe { libc::sched_yield() }) }
}

#[repr(C)]
struct Event { mutex: libc::pthread_mutex_t, cond: libc::pthread_cond_t, manual: bool, signaled: bool, waiters: usize }
#[repr(C)]
struct Mutex {
    native: libc::pthread_mutex_t,
    users: AtomicUsize, // includes lock acquisitions waiting in pthread
    owner: AtomicUsize,
    depth: usize,      // accessed by the owner only, before native unlock
    recursive: bool,
}
#[repr(C)]
struct Thread { native: libc::pthread_t }
#[repr(C)]
struct Tls { native: libc::pthread_key_t }
struct Start { entry: Entry, arg: *mut c_void }

impl port::Events for Linux {
    unsafe fn create(manual: bool, initial: bool) -> Result<*mut c_void> {
        let p = unsafe { alloc::<Event>() };
        if p.is_null() { return Err(Error::OutOfMemory); }
        let mut attr = mem::MaybeUninit::<libc::pthread_condattr_t>::uninit();
        let mut rc = unsafe { libc::pthread_condattr_init(attr.as_mut_ptr()) };
        if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc).map(|_| ptr::null_mut()); }
        rc = unsafe { libc::pthread_condattr_setclock(attr.as_mut_ptr(), libc::CLOCK_MONOTONIC) };
        if rc != 0 {
            unsafe { libc::pthread_condattr_destroy(attr.as_mut_ptr()); libc::free(p.cast()); }
            return status(rc).map(|_| ptr::null_mut());
        }
        rc = unsafe { libc::pthread_mutex_init(ptr::addr_of_mut!((*p).mutex), ptr::null()) };
        if rc != 0 {
            unsafe { libc::pthread_condattr_destroy(attr.as_mut_ptr()); libc::free(p.cast()); }
            return status(rc).map(|_| ptr::null_mut());
        }
        rc = unsafe { libc::pthread_cond_init(ptr::addr_of_mut!((*p).cond), attr.as_ptr()) };
        unsafe { libc::pthread_condattr_destroy(attr.as_mut_ptr()); }
        if rc != 0 {
            unsafe { libc::pthread_mutex_destroy(ptr::addr_of_mut!((*p).mutex)); libc::free(p.cast()); }
            return status(rc).map(|_| ptr::null_mut());
        }
        unsafe { (*p).manual = manual; (*p).signaled = initial; }
        Ok(p.cast())
    }
    unsafe fn destroy(h: *mut c_void) -> Result<()> {
        let p = handle::<Event>(h)?;
        let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
        status(unsafe { libc::pthread_mutex_lock(mutex) })?;
        if unsafe { (*p).waiters != 0 } {
            unsafe { libc::pthread_mutex_unlock(mutex); }
            return Err(Error::Busy);
        }
        // Caller has exclusive lifecycle ownership, so no new waiter may enter here.
        let rc = unsafe { libc::pthread_cond_destroy(ptr::addr_of_mut!((*p).cond)) };
        let unlock = unsafe { libc::pthread_mutex_unlock(mutex) };
        status(rc)?;
        status(unlock)?;
        let rc = unsafe { libc::pthread_mutex_destroy(mutex) };
        if rc == 0 { unsafe { libc::free(h) }; }
        status(rc)
    }
    unsafe fn set(h: *mut c_void) -> Result<()> {
        let p = handle::<Event>(h)?;
        let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
        status(unsafe { libc::pthread_mutex_lock(mutex) })?;
        unsafe { (*p).signaled = true; }
        let rc = unsafe {
            if (*p).manual { libc::pthread_cond_broadcast(ptr::addr_of_mut!((*p).cond)) }
            else { libc::pthread_cond_signal(ptr::addr_of_mut!((*p).cond)) }
        };
        let unlock = unsafe { libc::pthread_mutex_unlock(mutex) };
        status(if rc != 0 { rc } else { unlock })
    }
    unsafe fn reset(h: *mut c_void) -> Result<()> {
        let p = handle::<Event>(h)?;
        let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
        status(unsafe { libc::pthread_mutex_lock(mutex) })?;
        unsafe { (*p).signaled = false; }
        status(unsafe { libc::pthread_mutex_unlock(mutex) })
    }
    unsafe fn wait(h: *mut c_void, ns: u64) -> Result<()> {
        let p = handle::<Event>(h)?;
        let end = if ns != 0 && ns != INFINITE { deadline(ns)? } else { libc::timespec { tv_sec: 0, tv_nsec: 0 } };
        let mutex = unsafe { ptr::addr_of_mut!((*p).mutex) };
        let cond = unsafe { ptr::addr_of_mut!((*p).cond) };
        let mut rc = unsafe { libc::pthread_mutex_lock(mutex) };
        status(rc)?;
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
}
fn deadline(ns: u64) -> Result<libc::timespec> {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 { return Err(Error::Os); }
    if now.tv_sec < 0 || !(0..1_000_000_000).contains(&now.tv_nsec) { return Err(Error::Os); }
    let nanos = now.tv_nsec as u64 + ns % 1_000_000_000;
    let seconds = (now.tv_sec as u64).checked_add(ns / 1_000_000_000)
        .and_then(|s| s.checked_add(nanos / 1_000_000_000)).ok_or(Error::InvalidArgument)?;
    let seconds = seconds.try_into().map_err(|_| Error::InvalidArgument)?;
    Ok(libc::timespec { tv_sec: seconds, tv_nsec: (nanos % 1_000_000_000) as _ })
}
// Overflow-checked increment without the nightly-deprecated fetch_update helper.
fn checked_increment(counter: &AtomicUsize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(1) else { return false; };
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}
impl port::Mutexes for Linux {
    unsafe fn create(recursive: bool) -> Result<*mut c_void> {
        let p = unsafe { alloc::<Mutex>() };
        if p.is_null() { return Err(Error::OutOfMemory); }
        let mut attr = mem::MaybeUninit::<libc::pthread_mutexattr_t>::uninit();
        let mut rc = unsafe { libc::pthread_mutexattr_init(attr.as_mut_ptr()) };
        if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc).map(|_| ptr::null_mut()); }
        let kind = if recursive { libc::PTHREAD_MUTEX_RECURSIVE } else { libc::PTHREAD_MUTEX_ERRORCHECK };
        rc = unsafe { libc::pthread_mutexattr_settype(attr.as_mut_ptr(), kind) };
        if rc == 0 { rc = unsafe { libc::pthread_mutex_init(ptr::addr_of_mut!((*p).native), attr.as_ptr()) }; }
        unsafe { libc::pthread_mutexattr_destroy(attr.as_mut_ptr()); }
        if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc).map(|_| ptr::null_mut()); }
        unsafe {
            ptr::addr_of_mut!((*p).users).write(AtomicUsize::new(0));
            ptr::addr_of_mut!((*p).owner).write(AtomicUsize::new(0));
            (*p).recursive = recursive;
        }
        Ok(p.cast())
    }
    unsafe fn destroy(h: *mut c_void) -> Result<()> {
        let p = handle::<Mutex>(h)?;
        // Destroying a locked pthread mutex is undefined by POSIX even when glibc
        // happens to return EBUSY. Check our ownership state BEFORE entering libc.
        if unsafe { (*p).users.load(Ordering::Acquire) } != 0 { return Err(Error::Busy); }
        let rc = unsafe { libc::pthread_mutex_destroy(ptr::addr_of_mut!((*p).native)) };
        if rc == 0 { unsafe { libc::free(h) }; }
        status(rc)
    }
    unsafe fn lock(h: *mut c_void) -> Result<()> {
        let p = handle::<Mutex>(h)?;
        let current = unsafe { libc::pthread_self() as usize };
        if unsafe { (*p).owner.load(Ordering::Acquire) } == current &&
            (!unsafe { (*p).recursive } || unsafe { (*p).depth } == usize::MAX) { return Err(Error::Os); }
        if !unsafe { checked_increment(&(*p).users) } { return Err(Error::Os); }
        let rc = unsafe { libc::pthread_mutex_lock(ptr::addr_of_mut!((*p).native)) };
        if rc == 0 {
            unsafe { (*p).depth += 1; (*p).owner.store(current, Ordering::Release); }
        } else { unsafe { (*p).users.fetch_sub(1, Ordering::Release); } }
        status(rc)
    }
    unsafe fn unlock(h: *mut c_void) -> Result<()> {
        let p = handle::<Mutex>(h)?;
        let current = unsafe { libc::pthread_self() as usize };
        if unsafe { (*p).owner.load(Ordering::Acquire) } != current { return Err(Error::Os); }
        unsafe {
            (*p).depth -= 1;
            if (*p).depth == 0 { (*p).owner.store(0, Ordering::Release); }
        }
        let rc = unsafe { libc::pthread_mutex_unlock(ptr::addr_of_mut!((*p).native)) };
        if rc == 0 { unsafe { (*p).users.fetch_sub(1, Ordering::Release); } }
        else { unsafe { (*p).depth += 1; (*p).owner.store(current, Ordering::Release); } }
        status(rc)
    }
}
extern "C" fn thread_start(data: *mut c_void) -> *mut c_void {
    unsafe {
        let start = ptr::read(data.cast::<Start>());
        libc::free(data);
        (start.entry)(start.arg)
    }
}
impl port::Threads for Linux {
    unsafe fn create(entry: Entry, arg: *mut c_void, stack: usize) -> Result<*mut c_void> {
        let handle = unsafe { alloc::<Thread>() };
        if handle.is_null() { return Err(Error::OutOfMemory); }
        let start = unsafe { alloc::<Start>() };
        if start.is_null() { unsafe { libc::free(handle.cast()) }; return Err(Error::OutOfMemory); }
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
            return status(rc).map(|_| ptr::null_mut());
        }
        // The worker owns start now and may already have freed it. Do not access it.
        Ok(handle.cast())
    }
    unsafe fn join(h: *mut c_void) -> Result<()> {
        let p = handle::<Thread>(h)?;
        let rc = unsafe { libc::pthread_join((*p).native, ptr::null_mut()) };
        if rc == 0 { unsafe { libc::free(h) }; }
        status(rc)
    }
    unsafe fn detach(h: *mut c_void) -> Result<()> {
        let p = handle::<Thread>(h)?;
        let rc = unsafe { libc::pthread_detach((*p).native) };
        if rc == 0 { unsafe { libc::free(h) }; }
        status(rc)
    }
}
impl port::ThreadLocal for Linux {
    unsafe fn create(dtor: Option<Destructor>) -> Result<*mut c_void> {
        let p = unsafe { alloc::<Tls>() };
        if p.is_null() { return Err(Error::OutOfMemory); }
        let rc = unsafe { libc::pthread_key_create(ptr::addr_of_mut!((*p).native), dtor) };
        if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc).map(|_| ptr::null_mut()); }
        Ok(p.cast())
    }
    unsafe fn destroy(h: *mut c_void) -> Result<()> {
        let p = handle::<Tls>(h)?;
        let rc = unsafe { libc::pthread_key_delete((*p).native) };
        if rc == 0 { unsafe { libc::free(h) }; }
        status(rc)
    }
    unsafe fn get(h: *mut c_void) -> Result<*mut c_void> {
        let p = handle::<Tls>(h)?;
        Ok(unsafe { libc::pthread_getspecific((*p).native) })
    }
    unsafe fn set(h: *mut c_void, value: *mut c_void) -> Result<()> {
        let p = handle::<Tls>(h)?;
        status(unsafe { libc::pthread_setspecific((*p).native, value) })
    }
}
impl port::StackBounds for Linux {
    fn current() -> Result<(*mut c_void, *mut c_void)> {
        let mut attr = mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
        status(unsafe { libc::pthread_getattr_np(libc::pthread_self(), attr.as_mut_ptr()) })?;
        let mut base = ptr::null_mut(); let mut size = 0;
        let rc = unsafe { libc::pthread_attr_getstack(attr.as_ptr(), &mut base, &mut size) };
        unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()); }
        status(rc)?;
        let Some(end) = (base as usize).checked_add(size) else { return Err(Error::Os); };
        if base.is_null() || size == 0 { return Err(Error::Os); }
        Ok((base, end as *mut c_void))
    }
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
impl port::ProcessBarrier for Linux {
    fn available() -> bool { barrier_mode() >= 2 }
    fn barrier() -> Result<()> {
        let cmd = match barrier_mode() { 2 => PRIVATE_EXPEDITED, 3 => GLOBAL, _ => return Err(Error::Unsupported) };
        if unsafe { libc::syscall(libc::SYS_membarrier, cmd, 0) } == 0 { Ok(()) } else { Err(Error::Os) }
    }
}

fn name_buffer(name: &[u8]) -> [u8; MAX_NAME + 1] {
    let mut result = [0u8; MAX_NAME + 1];
    result[..name.len()].copy_from_slice(name);
    result
}
impl port::Environment for Linux {
    unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup> {
        let name = name_buffer(name);
        let value = unsafe { libc::getenv(name.as_ptr().cast()) };
        if value.is_null() { return Err(Error::NotFound); }
        // Caller must prevent concurrent environment mutation for the full call.
        let bytes = unsafe { CStr::from_ptr(value) }.to_bytes_with_nul();
        if capacity < bytes.len() { return Ok(Lookup::TooSmall(bytes.len())); }
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
        Ok(Lookup::Copied(bytes.len()))
    }
}
impl port::Identity for Linux {
    fn process_id() -> Result<u64> {
        let value = unsafe { libc::getpid() };
        if value <= 0 { Err(Error::Os) } else { Ok(value as u64) }
    }
    fn thread_id() -> Result<u64> {
        let value = unsafe { libc::syscall(libc::SYS_gettid) };
        if value <= 0 { Err(Error::Os) } else { Ok(value as u64) }
    }
}
impl port::Realtime for Linux {
    fn realtime_ns() -> Result<u64> {
        let mut value = mem::MaybeUninit::<libc::timespec>::uninit();
        if unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, value.as_mut_ptr()) } != 0 { return Err(Error::Os); }
        let value = unsafe { value.assume_init() };
        if value.tv_sec < 0 || !(0..1_000_000_000).contains(&value.tv_nsec) { return Err(Error::Os); }
        (value.tv_sec as u64).checked_mul(1_000_000_000).and_then(|x| x.checked_add(value.tv_nsec as u64)).ok_or(Error::Os)
    }
}
impl port::Entropy for Linux {
    unsafe fn fill(out: *mut u8, size: usize) -> Result<()> {
        let mut done = 0;
        while done < size {
            let result = unsafe { libc::getrandom(out.add(done).cast(), (size - done).min(262144), 0) };
            if result < 0 {
                if errno() == libc::EINTR { continue; }
                return Err(Error::Os);
            }
            if result == 0 { return Err(Error::Os); }
            done += result as usize;
        }
        Ok(())
    }
}
fn protection(value: u32) -> i32 {
    (if value & READ != 0 { libc::PROT_READ } else { 0 }) |
    (if value & WRITE != 0 { libc::PROT_WRITE } else { 0 }) |
    (if value & EXECUTE != 0 { libc::PROT_EXEC } else { 0 })
}
fn page_aligned(address: *mut c_void) -> bool {
    let page = page();
    page.is_power_of_two() && address as usize & (page - 1) == 0
}
impl port::NativeMapping for Linux {
    fn page_size() -> usize { page() }
    unsafe fn allocate(size: usize, permissions: u32) -> Result<*mut c_void> {
        let value = unsafe { libc::mmap(ptr::null_mut(), size, protection(permissions), libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
        if value == libc::MAP_FAILED {
            return Err(if errno() == libc::ENOMEM { Error::OutOfMemory } else { Error::Os });
        }
        Ok(value)
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        if !page_aligned(address) { return Err(Error::InvalidArgument); }
        os(unsafe { libc::munmap(address, size) })
    }
    unsafe fn protect(address: *mut c_void, size: usize, permissions: u32) -> Result<()> {
        if !page_aligned(address) { return Err(Error::InvalidArgument); }
        os(unsafe { libc::mprotect(address, size, protection(permissions)) })
    }
}
impl port::Modules for Linux {
    unsafe fn open(name: Option<&[u8]>) -> Result<*mut c_void> {
        let buffer = name.map(name_buffer);
        let name = match &buffer { Some(b) => b.as_ptr().cast(), None => ptr::null() };
        let module = unsafe { libc::dlopen(name, libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if module.is_null() { Err(Error::NotFound) } else { Ok(module) }
    }
    unsafe fn symbol(module: *mut c_void, name: &[u8]) -> Result<*mut c_void> {
        let name = name_buffer(name);
        unsafe { libc::dlerror() };
        let symbol = unsafe { libc::dlsym(module, name.as_ptr().cast()) };
        if !unsafe { libc::dlerror() }.is_null() { Err(Error::NotFound) } else { Ok(symbol) }
    }
    unsafe fn close(module: *mut c_void) -> Result<()> { os(unsafe { libc::dlclose(module) }) }
    unsafe fn info(address: *mut c_void) -> Result<ModuleInfo> {
        let mut value = mem::MaybeUninit::<libc::Dl_info>::uninit();
        if unsafe { libc::dladdr(address, value.as_mut_ptr()) } == 0 { return Err(Error::NotFound); }
        let value = unsafe { value.assume_init() };
        if value.dli_fname.is_null() { return Err(Error::Os); }
        let name = unsafe { CStr::from_ptr(value.dli_fname) }.to_bytes();
        Ok(ModuleInfo { base: value.dli_fbase, name: value.dli_fname.cast(), name_length: name.len() })
    }
}

impl port::NativeHeap for Linux {
    unsafe fn allocate(size: usize, zero: bool) -> Result<*mut c_void> {
        let p = unsafe { if zero { libc::calloc(1, size) } else { libc::malloc(size) } };
        if p.is_null() { Err(Error::OutOfMemory) } else { Ok(p) }
    }
    unsafe fn resize(p: *mut c_void, size: usize) -> Result<*mut c_void> {
        let result = unsafe { libc::realloc(p, size) };
        if result.is_null() { Err(Error::OutOfMemory) } else { Ok(result) }
    }
    unsafe fn release(p: *mut c_void) -> Result<()> { unsafe { libc::free(p) }; Ok(()) }
}
impl port::RwLocks for Linux {
    unsafe fn create() -> Result<*mut c_void> {
        let p = unsafe { libc::malloc(mem::size_of::<libc::pthread_rwlock_t>()) }.cast::<libc::pthread_rwlock_t>();
        if p.is_null() { return Err(Error::OutOfMemory); }
        let rc = unsafe { libc::pthread_rwlock_init(p, ptr::null()) };
        if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc).map(|_| ptr::null_mut()); }
        Ok(p.cast())
    }
    unsafe fn read(p: *mut c_void) -> Result<()> { status(unsafe { libc::pthread_rwlock_rdlock(p.cast()) }) }
    unsafe fn write(p: *mut c_void) -> Result<()> { status(unsafe { libc::pthread_rwlock_wrlock(p.cast()) }) }
    unsafe fn unlock(p: *mut c_void) -> Result<()> { status(unsafe { libc::pthread_rwlock_unlock(p.cast()) }) }
    unsafe fn destroy(p: *mut c_void) -> Result<()> {
        let rc = unsafe { libc::pthread_rwlock_destroy(p.cast()) };
        if rc == 0 { unsafe { libc::free(p) }; }
        status(rc)
    }
}
impl port::Diagnostics for Linux {
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
        let mut done = 0;
        while done < size {
            let n = unsafe { libc::write(2, data.add(done).cast(), size - done) };
            if n > 0 { done += n as usize; }
            else if n < 0 && errno() == libc::EINTR { continue; }
            else { return Err((done, Error::Os)); }
        }
        Ok(())
    }
}
impl port::ThreadName for Linux {
    fn set(name: &[u8]) -> Result<()> {
        if name.len() > 15 { return Err(Error::InvalidArgument); }
        let mut buffer = [0u8; 16];
        buffer[..name.len()].copy_from_slice(name);
        status(unsafe { libc::pthread_setname_np(libc::pthread_self(), buffer.as_ptr().cast()) })
    }
}

pub mod context {
    //! Linux signal substrate: SIGRTMIN activation plus hardware fault signals.
    use super::*;
    use crate::context::{Callback, Ops, ACTIVATION, BUS, FLOATING_POINT, ILLEGAL_INSTRUCTION, SEGMENTATION};
    use crate::{INVALID_ARGUMENT, OK, OS_ERROR};
    use core::sync::atomic::AtomicI32;
    struct Slot {claimed:AtomicUsize,signal:AtomicI32,callback:AtomicUsize,data:AtomicUsize}
    impl Slot {const fn new()->Self{Self{claimed:AtomicUsize::new(0),signal:AtomicI32::new(0),callback:AtomicUsize::new(0),data:AtomicUsize::new(0)}}}
    static SLOTS:[Slot;5]=[const {Slot::new()};5];
    extern "C" fn signal_number(kind:u32)->i32{signal(kind).unwrap_or(-1)}
    fn signal(kind:u32)->Option<i32>{match kind{
        ACTIVATION=>Some(libc::SIGRTMIN()),SEGMENTATION=>Some(libc::SIGSEGV),BUS=>Some(libc::SIGBUS),
        FLOATING_POINT=>Some(libc::SIGFPE),ILLEGAL_INSTRUCTION=>Some(libc::SIGILL),_=>None}}
    // Invoked by the kernel. No allocation, locking, API negotiation or Rust unwinding.
    extern "C" fn trampoline(code:i32,info:*mut libc::siginfo_t,context:*mut c_void){
        let errno=unsafe{libc::__errno_location()};let saved=unsafe{errno.read()};
        for slot in &SLOTS {
            if slot.signal.load(Ordering::Acquire)==code {
                let address=slot.callback.load(Ordering::Acquire);
                if address!=0 {
                    // Only an actual Callback supplied at registration can enter this slot.
                    let callback:Callback=unsafe{mem::transmute(address)};
                    unsafe{callback(code,info.cast(),context,slot.data.load(Ordering::Relaxed) as *mut c_void)};
                }
                break;
            }
        }
        unsafe{errno.write(saved)};
    }
    extern "C" fn abi_tag()->u64 {
        #[cfg(target_arch="x86_64")]return 0x4c4e580000000001;
        #[cfg(target_arch="aarch64")]return 0x4c4e580000000002;
        #[cfg(not(any(target_arch="x86_64",target_arch="aarch64")))]0
    }
    extern "C" fn action_size()->usize{mem::size_of::<libc::sigaction>()}
    extern "C" fn action_alignment()->usize{mem::align_of::<libc::sigaction>()}
    unsafe extern "C" fn install(kind:u32,callback:Option<Callback>,data:*mut c_void,previous:*mut c_void,_size:usize)->u32 {
        let Some(number)=signal(kind) else{return INVALID_ARGUMENT};let Some(callback)=callback else{return INVALID_ARGUMENT};
        let slot=&SLOTS[kind as usize];
        if slot.claimed.compare_exchange(0,1,Ordering::AcqRel,Ordering::Acquire).is_err(){return 6}
        let previous=previous.cast::<libc::sigaction>();
        // Publish the old action BEFORE enabling the callback: it may run immediately.
        if unsafe{libc::sigaction(number,ptr::null(),previous)}!=0 {slot.claimed.store(0,Ordering::Release);return OS_ERROR}
        let old=unsafe{previous.read()};
        if old.sa_sigaction==trampoline as *const () as usize {slot.claimed.store(0,Ordering::Release);return 6}
        let mut action:libc::sigaction=unsafe{mem::zeroed()};
        action.sa_flags=libc::SA_RESTART|libc::SA_SIGINFO;
        action.sa_sigaction=trampoline as *const () as usize;
        unsafe{libc::sigemptyset(&mut action.sa_mask)};
        if old.sa_flags & libc::SA_ONSTACK!=0 {action.sa_flags|=libc::SA_ONSTACK;action.sa_mask=old.sa_mask;}
        // Warm these libc entry points before a first asynchronous callback.
        unsafe{libc::__errno_location();libc::getpid();}
        slot.data.store(data as usize,Ordering::Relaxed);slot.callback.store(callback as usize,Ordering::Release);
        slot.signal.store(number,Ordering::Release);
        if unsafe{libc::sigaction(number,&action,ptr::null_mut())}!=0 {
            slot.signal.store(0,Ordering::Release);slot.callback.store(0,Ordering::Release);slot.claimed.store(0,Ordering::Release);return OS_ERROR;
        }
        OK
    }
    unsafe extern "C" fn restore(kind:u32,previous:*const c_void,_size:usize)->u32 {
        let Some(number)=signal(kind) else{return INVALID_ARGUMENT};
        if SLOTS[kind as usize].claimed.load(Ordering::Acquire)==0{return INVALID_ARGUMENT}
        if unsafe{libc::sigaction(number,previous.cast(),ptr::null_mut())}!=0{return OS_ERROR}
        // Keep immutable callback data for an already-running handler. Registrations
        // are process-lifetime; restoration does not make the slot reusable.
        OK
    }
    unsafe extern "C" fn unblock_activation()->u32 {
        let mut set:libc::sigset_t=unsafe{mem::zeroed()};
        unsafe{libc::sigemptyset(&mut set);libc::sigaddset(&mut set,libc::SIGRTMIN());}
        if unsafe{libc::pthread_sigmask(libc::SIG_UNBLOCK,&set,ptr::null_mut())}==0{OK}else{OS_ERROR}
    }
    unsafe extern "C" fn request_activation(token:usize)->u32 {
        match unsafe{libc::pthread_kill(token as libc::pthread_t,libc::SIGRTMIN())}{0=>OK,libc::EAGAIN=>6,libc::ESRCH=>8,_=>OS_ERROR}
    }
    unsafe extern "C" fn current_thread(out:*mut usize)->u32{unsafe{out.write(libc::pthread_self() as usize)};OK}
    unsafe extern "C" fn process_id_async(out:*mut u64)->u32{unsafe{out.write(libc::getpid() as u64)};OK}
    unsafe extern "C" fn ignore_broken_pipe()->u32 {
        let mut action:libc::sigaction=unsafe{mem::zeroed()};action.sa_sigaction=libc::SIG_IGN;
        unsafe{libc::sigemptyset(&mut action.sa_mask)};
        if unsafe{libc::sigaction(libc::SIGPIPE,&action,ptr::null_mut())}==0{OK}else{OS_ERROR}
    }
    static OPS:Ops=Ops {abi_tag:Some(abi_tag),action_size:Some(action_size),action_alignment:Some(action_alignment),
        install:Some(install),restore:Some(restore),unblock_activation:Some(unblock_activation),request_activation:Some(request_activation),
        current_thread:Some(current_thread),process_id_async:Some(process_id_async),ignore_broken_pipe:Some(ignore_broken_pipe),read_stats:None,signal_number:Some(signal_number)};
    impl port::SignalContext for Linux { fn ops() -> Option<&'static Ops> { Some(&OPS) } }
}
