//! A `dotnet-pal-rs` port implemented with the Rust standard library.
//!
//! This crate is separate from the `no_std` core on purpose: it pulls in `std`,
//! `libc` (Unix) or `windows-sys` (Windows) and `getrandom`, none of which a
//! freestanding or Wasm port wants. It provides every capability a desktop
//! host can honestly supply and leaves the rest absent:
//!
//! | Capability | Provider |
//! | --- | --- |
//! | Virtual memory, native mappings | `mmap`/`mprotect` or `VirtualAlloc`/`VirtualProtect` |
//! | Clock, scheduler, realtime | `std::time`, `std::thread` |
//! | Events, mutexes, threads, rwlocks | `std::sync` and `std::thread` |
//! | Thread-local slots | a per-thread table with destructors run at thread exit |
//! | Stack bounds, thread ids, thread names | platform calls per OS |
//! | Process barrier | Linux `membarrier`, Windows `FlushProcessWriteBuffers`; absent elsewhere |
//! | Environment, entropy, native heap, diagnostics | `std::env`, `getrandom`, `std::alloc`, `std::io::stderr` |
//! | Modules | `dlopen`/`dlsym`/`dladdr` or `LoadLibrary`/`GetProcAddress` |
//! | Topology (CPU counts, affinity, memory figures, cache, feature words) | procfs/sysfs/cgroup v2, sysctl/Mach, or system information APIs |
//! | Process (exit, debugger presence) | `std::process`, procfs or sysctl; crash dumps are unsupported |
//! | Image (unwind tables, readability, build id) | ELF program headers on Linux; absent elsewhere |
//! | Standard streams | `std::io` stdin, stdout and stderr |
//! | Files and directories | `std::fs` with positional transfers; byte paths on Unix, UTF-8 paths on Windows |
//! | Fault reporting | the five fault signals on Linux and macOS (AArch64, x86-64), reported in the boundary's frame; absent on Windows and every other target |
//! | Sockets (TCP, UDP, readiness, name resolution) | `socket2`, poll(2) or `WSAPoll`; a wake channel is a socket pair, on Windows a loopback UDP socket |
//! | System information (environment enumeration, executable path, OS texts, user, CPU time, uptime, ids) | `std::env`, uname, the passwd entry, `getrusage`, the boot-time clock; `GetProcessTimes`, `GetTickCount64` and the user's variables on Windows |
//! | Notifications (interrupt, quit, terminate, hangup, continue, window change, job-control stops) | POSIX signals through a self-pipe and a dispatcher thread; console control events (interrupt, quit, terminate) on Windows |
//! | Child processes (spawn, pipes to the standard streams, timed waits, termination) | `std::process`; a wait watches the child without reaping it (`waitid` or the process handle), so another thread can end it meanwhile |
//! | Terminal (window size, line and raw input, readiness, editing characters) | termios and the window-size ioctl behind descriptor 0; the console API on Windows |
//! | Changes to files and directories (watches per directory or file, renames paired by cookie, timed reads) | inotify on Linux; elsewhere a thread per watcher that compares directory listings ten times a second and never reports an access |
//! | Files mapped into memory (shared and private mappings of a file handle, synchronization) | `mmap`, `munmap` and `msync` on the handle's descriptor; absent on Windows |
//! | Mounted volumes (mount points, capacity, free space, format name) | /proc/self/mounts and `statvfs` on Linux, `getfsstat` and `statfs` on macOS; absent elsewhere |
//! | Network (interfaces and their addresses, reverse lookup, multicast membership) | `getifaddrs` with the link-layer entries, the MTU ioctl and sysfs on Linux, `getnameinfo`, `socket2` on the sockets provider's handles; absent on Windows |
//! | Signal context, WASI transport | absent |
//!
//! With the `entry` feature the crate exports `dotnet_pal_get_api` itself, so
//! the static library links directly into a NativeAOT runtime build. Without it,
//! the providers can be composed into another port with `define_pal!`.
//! Handles handed across the C ABI are `Box`es; a caller that uses one after
//! destroying it, or destroys it twice, violates the boundary contract exactly as
//! with the Linux reference providers.
#![deny(unsafe_op_in_unsafe_fn)]
use dotnet_pal_rs::kernel::{Destructor, Entry, INFINITE};
use dotnet_pal_rs::port::{self, Error, Lookup, Result};
use dotnet_pal_rs::runtime::ModuleInfo;
use std::{cell::RefCell, ffi::c_void, io::Write, ptr, sync::{Condvar, Mutex, OnceLock}, time::{Duration, Instant, SystemTime}};

/// The desktop port. Each provider trait is implemented on this type.
pub struct Std;
mod system;
mod sysinfo;
mod files;
mod sockets;
mod notifications;
mod processes;
mod terminal;
mod watches;
mod mappings;
mod volumes;
mod network;
#[cfg(all(any(target_os = "linux", target_os = "macos"), any(target_arch = "aarch64", target_arch = "x86_64")))]
mod faults;
/// No signal context `faults.rs` can convert: the capability is absent.
#[cfg(not(all(any(target_os = "linux", target_os = "macos"), any(target_arch = "aarch64", target_arch = "x86_64"))))]
mod faults { pub type Provider = dotnet_pal_rs::port::Absent; }

fn boxed<T>(value: T) -> *mut c_void { Box::into_raw(Box::new(value)).cast() }
unsafe fn borrow<'a, T>(handle: *mut c_void) -> Result<&'a T> {
    if handle.is_null() || handle as usize % std::mem::align_of::<T>() != 0 { return Err(Error::InvalidArgument); }
    Ok(unsafe { &*handle.cast::<T>() })
}
unsafe fn take<T>(handle: *mut c_void) -> Result<Box<T>> {
    if handle.is_null() || handle as usize % std::mem::align_of::<T>() != 0 { return Err(Error::InvalidArgument); }
    Ok(unsafe { Box::from_raw(handle.cast::<T>()) })
}

impl port::Abort for Std { fn abort() -> ! { std::process::abort() } }

impl port::Clock for Std {
    fn monotonic_ns() -> Result<u64> {
        static START: OnceLock<Instant> = OnceLock::new();
        let start = *START.get_or_init(Instant::now);
        u64::try_from(start.elapsed().as_nanos()).map_err(|_| Error::Os)
    }
}
impl port::Scheduler for Std {
    fn sleep_ns(nanoseconds: u64) -> Result<()> { std::thread::sleep(Duration::from_nanos(nanoseconds)); Ok(()) }
    fn yield_now() -> Result<()> { std::thread::yield_now(); Ok(()) }
}
impl port::Realtime for Std {
    fn realtime_ns() -> Result<u64> {
        let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_err(|_| Error::Os)?;
        u64::try_from(since.as_nanos()).map_err(|_| Error::Os)
    }
}
impl port::Entropy for Std {
    unsafe fn fill(out: *mut u8, size: usize) -> Result<()> {
        let buffer = unsafe { std::slice::from_raw_parts_mut(out.cast::<std::mem::MaybeUninit<u8>>(), size) };
        getrandom::fill_uninit(buffer).map(|_| ()).map_err(|_| Error::Os)
    }
}
impl port::Environment for Std {
    unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup> {
        let name = std::str::from_utf8(name).map_err(|_| Error::InvalidArgument)?;
        let value = std::env::var(name).map_err(|e| match e { std::env::VarError::NotPresent => Error::NotFound, _ => Error::Os })?;
        let needed = value.len() + 1;
        if capacity < needed { return Ok(Lookup::TooSmall(needed)); }
        unsafe { ptr::copy_nonoverlapping(value.as_ptr(), out, value.len()); out.add(value.len()).write(0); }
        Ok(Lookup::Copied(needed))
    }
}
impl port::Diagnostics for Std {
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        let mut done = 0;
        let mut stderr = std::io::stderr().lock();
        while done < size {
            match stderr.write(&bytes[done..]) {
                Ok(0) => return Err((done, Error::Os)),
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err((done, Error::Os)),
            }
        }
        Ok(())
    }
}

/// Size-prefixed allocations so `release` needs no layout from the caller.
impl port::NativeHeap for Std {
    unsafe fn allocate(size: usize, zero: bool) -> Result<*mut c_void> {
        let layout = std::alloc::Layout::from_size_align(size.checked_add(HEADER).ok_or(Error::InvalidArgument)?, HEADER).map_err(|_| Error::InvalidArgument)?;
        let raw = unsafe { if zero { std::alloc::alloc_zeroed(layout) } else { std::alloc::alloc(layout) } };
        if raw.is_null() { return Err(Error::OutOfMemory); }
        unsafe { raw.cast::<usize>().write(size); }
        Ok(unsafe { raw.add(HEADER) }.cast())
    }
    unsafe fn resize(address: *mut c_void, size: usize) -> Result<*mut c_void> {
        if address.is_null() { return unsafe { <Self as port::NativeHeap>::allocate(size, false) }; }
        let old = unsafe { address.cast::<u8>().sub(HEADER) };
        let old_size = unsafe { old.cast::<usize>().read() };
        let new = unsafe { <Self as port::NativeHeap>::allocate(size, false) }?;
        unsafe { ptr::copy_nonoverlapping(address.cast::<u8>(), new.cast::<u8>(), old_size.min(size)); }
        unsafe { <Self as port::NativeHeap>::release(address) }?;
        Ok(new)
    }
    unsafe fn release(address: *mut c_void) -> Result<()> {
        if address.is_null() { return Ok(()); }
        let raw = unsafe { address.cast::<u8>().sub(HEADER) };
        let size = unsafe { raw.cast::<usize>().read() };
        let layout = std::alloc::Layout::from_size_align(size + HEADER, HEADER).map_err(|_| Error::InvalidArgument)?;
        unsafe { std::alloc::dealloc(raw, layout) };
        Ok(())
    }
}
const HEADER: usize = 16;

struct Event { state: Mutex<EventState>, signal: Condvar }
struct EventState { manual: bool, signaled: bool, waiters: usize }
impl port::Events for Std {
    unsafe fn create(manual_reset: bool, initially_set: bool) -> Result<*mut c_void> {
        Ok(boxed(Event { state: Mutex::new(EventState { manual: manual_reset, signaled: initially_set, waiters: 0 }), signal: Condvar::new() }))
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        if event.state.lock().map_err(|_| Error::Os)?.waiters != 0 { return Err(Error::Busy); }
        drop(unsafe { take::<Event>(handle) }?);
        Ok(())
    }
    unsafe fn set(handle: *mut c_void) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        let mut state = event.state.lock().map_err(|_| Error::Os)?;
        state.signaled = true;
        if state.manual { event.signal.notify_all(); } else { event.signal.notify_one(); }
        Ok(())
    }
    unsafe fn reset(handle: *mut c_void) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        event.state.lock().map_err(|_| Error::Os)?.signaled = false;
        Ok(())
    }
    unsafe fn wait(handle: *mut c_void, timeout_ns: u64) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        let mut state = event.state.lock().map_err(|_| Error::Os)?;
        state.waiters += 1;
        let deadline = (timeout_ns != INFINITE).then(|| Instant::now() + Duration::from_nanos(timeout_ns));
        let mut outcome = Ok(());
        while !state.signaled {
            match deadline {
                None => state = event.signal.wait(state).map_err(|_| Error::Os)?,
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline { outcome = Err(Error::Timeout); break; }
                    // Spurious wakes recheck the state against the SAME deadline.
                    state = event.signal.wait_timeout(state, deadline - now).map_err(|_| Error::Os)?.0;
                }
            }
        }
        if state.signaled {
            outcome = Ok(());
            if !state.manual { state.signaled = false; }
        }
        state.waiters -= 1;
        outcome
    }
}

struct RecursiveMutex { state: Mutex<MutexState>, released: Condvar, recursive: bool }
struct MutexState { owner: Option<std::thread::ThreadId>, depth: usize, users: usize }
impl port::Mutexes for Std {
    unsafe fn create(recursive: bool) -> Result<*mut c_void> {
        Ok(boxed(RecursiveMutex { state: Mutex::new(MutexState { owner: None, depth: 0, users: 0 }), released: Condvar::new(), recursive }))
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let mutex = unsafe { borrow::<RecursiveMutex>(handle) }?;
        if mutex.state.lock().map_err(|_| Error::Os)?.users != 0 { return Err(Error::Busy); }
        drop(unsafe { take::<RecursiveMutex>(handle) }?);
        Ok(())
    }
    unsafe fn lock(handle: *mut c_void) -> Result<()> {
        let mutex = unsafe { borrow::<RecursiveMutex>(handle) }?;
        let me = std::thread::current().id();
        let mut state = mutex.state.lock().map_err(|_| Error::Os)?;
        if state.owner == Some(me) {
            if !mutex.recursive || state.depth == usize::MAX { return Err(Error::Os); }
            state.depth += 1;
            state.users += 1;
            return Ok(());
        }
        state.users += 1;
        while state.owner.is_some() { state = mutex.released.wait(state).map_err(|_| Error::Os)?; }
        state.owner = Some(me);
        state.depth = 1;
        Ok(())
    }
    unsafe fn unlock(handle: *mut c_void) -> Result<()> {
        let mutex = unsafe { borrow::<RecursiveMutex>(handle) }?;
        let mut state = mutex.state.lock().map_err(|_| Error::Os)?;
        if state.owner != Some(std::thread::current().id()) { return Err(Error::Os); }
        state.depth -= 1;
        state.users -= 1;
        if state.depth == 0 { state.owner = None; mutex.released.notify_one(); }
        Ok(())
    }
}

struct Thread(Option<std::thread::JoinHandle<()>>);
struct Start(Entry, usize);
impl port::Threads for Std {
    unsafe fn create(entry: Entry, argument: *mut c_void, stack_size: usize) -> Result<*mut c_void> {
        let mut builder = std::thread::Builder::new();
        if stack_size != 0 { builder = builder.stack_size(stack_size); }
        let start = Start(entry, argument as usize);
        let handle = builder.spawn(move || { let Start(entry, arg) = start; unsafe { entry(arg as *mut c_void) }; }).map_err(|_| Error::OutOfMemory)?;
        Ok(boxed(Thread(Some(handle))))
    }
    unsafe fn join(handle: *mut c_void) -> Result<()> {
        let mut thread = unsafe { take::<Thread>(handle) }?;
        match thread.0.take() { Some(h) => h.join().map_err(|_| Error::Os), None => Err(Error::InvalidArgument) }
    }
    unsafe fn detach(handle: *mut c_void) -> Result<()> {
        let thread = unsafe { take::<Thread>(handle) }?;
        drop(thread); // dropping a JoinHandle detaches the thread
        Ok(())
    }
}

/// Dynamic thread-local slots. Keys index a process-wide table of destructors;
/// each thread keeps its own value vector and runs destructors when it exits.
static KEYS: Mutex<Vec<Option<Option<Destructor>>>> = Mutex::new(Vec::new());
struct Slots(Vec<*mut c_void>);
impl Drop for Slots {
    fn drop(&mut self) {
        // Destructors may set other slots; loop until every value is NULL, bounded.
        for _ in 0..4 {
            let mut pending = false;
            for (index, slot) in self.0.iter_mut().enumerate() {
                let value = std::mem::replace(slot, ptr::null_mut());
                if value.is_null() { continue; }
                let destructor = KEYS.lock().ok().and_then(|keys| keys.get(index).copied().flatten().flatten());
                if let Some(destructor) = destructor { pending = true; unsafe { destructor(value) }; }
            }
            if !pending { break; }
        }
    }
}
thread_local! { static SLOTS: RefCell<Slots> = const { RefCell::new(Slots(Vec::new())) }; }
impl port::ThreadLocal for Std {
    unsafe fn create(destructor: Option<Destructor>) -> Result<*mut c_void> {
        let mut keys = KEYS.lock().map_err(|_| Error::Os)?;
        let index = match keys.iter().position(|k| k.is_none()) {
            Some(index) => { keys[index] = Some(destructor); index }
            None => { keys.push(Some(destructor)); keys.len() - 1 }
        };
        Ok(boxed(index))
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let index = *unsafe { take::<usize>(handle) }?;
        let mut keys = KEYS.lock().map_err(|_| Error::Os)?;
        match keys.get_mut(index) { Some(slot) if slot.is_some() => { *slot = None; Ok(()) }, _ => Err(Error::InvalidArgument) }
    }
    unsafe fn get(handle: *mut c_void) -> Result<*mut c_void> {
        let index = *unsafe { borrow::<usize>(handle) }?;
        Ok(SLOTS.with(|slots| slots.borrow().0.get(index).copied().unwrap_or(ptr::null_mut())))
    }
    unsafe fn set(handle: *mut c_void, value: *mut c_void) -> Result<()> {
        let index = *unsafe { borrow::<usize>(handle) }?;
        if !KEYS.lock().map_err(|_| Error::Os)?.get(index).is_some_and(|k| k.is_some()) { return Err(Error::InvalidArgument); }
        SLOTS.with(|slots| {
            let mut slots = slots.borrow_mut();
            if slots.0.len() <= index { slots.0.resize(index + 1, ptr::null_mut()); }
            slots.0[index] = value;
        });
        Ok(())
    }
}

struct RwLock { state: Mutex<(usize, bool)>, changed: Condvar }
impl port::RwLocks for Std {
    unsafe fn create() -> Result<*mut c_void> { Ok(boxed(RwLock { state: Mutex::new((0, false)), changed: Condvar::new() })) }
    unsafe fn read(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let mut state = lock.state.lock().map_err(|_| Error::Os)?;
        while state.1 { state = lock.changed.wait(state).map_err(|_| Error::Os)?; }
        state.0 += 1;
        Ok(())
    }
    unsafe fn write(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let mut state = lock.state.lock().map_err(|_| Error::Os)?;
        while state.1 || state.0 != 0 { state = lock.changed.wait(state).map_err(|_| Error::Os)?; }
        state.1 = true;
        Ok(())
    }
    unsafe fn unlock(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let mut state = lock.state.lock().map_err(|_| Error::Os)?;
        if state.1 { state.1 = false; } else if state.0 != 0 { state.0 -= 1; } else { return Err(Error::Os); }
        lock.changed.notify_all();
        Ok(())
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let held = { let s = lock.state.lock().map_err(|_| Error::Os)?; s.1 || s.0 != 0 };
        if held { return Err(Error::Busy); }
        drop(unsafe { take::<RwLock>(handle) }?);
        Ok(())
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    fn page() -> usize { let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }; if v > 0 { v as usize } else { 0 } }
    fn errno() -> i32 { std::io::Error::last_os_error().raw_os_error().unwrap_or(0) }
    impl port::VirtualMemory for Std {
        fn page_size() -> usize { page() }
        unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void> {
            let page = page();
            let total = size.checked_add(alignment - page).ok_or(Error::Os)?;
            let raw = unsafe { libc::mmap(ptr::null_mut(), total, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
            if raw == libc::MAP_FAILED { return Err(Error::Os); }
            let aligned = (raw as usize + alignment - 1) & !(alignment - 1);
            let prefix = aligned - raw as usize;
            let suffix = total - prefix - size;
            if prefix != 0 { unsafe { libc::munmap(raw, prefix) }; }
            if suffix != 0 { unsafe { libc::munmap((aligned + size) as *mut c_void, suffix) }; }
            Ok(aligned as *mut c_void)
        }
        unsafe fn commit(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { libc::mprotect(address, size, libc::PROT_READ | libc::PROT_WRITE) } == 0 { Ok(()) } else { Err(Error::Os) }
        }
        unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()> {
            let result = unsafe { libc::mmap(address, size, libc::PROT_NONE, libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
            if result == libc::MAP_FAILED { Err(Error::Os) } else { Ok(()) }
        }
        unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { libc::munmap(address, size) } == 0 { Ok(()) } else { Err(Error::Os) }
        }
        unsafe fn reset(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { libc::madvise(address, size, libc::MADV_DONTNEED) } == 0 { Ok(()) } else { Err(Error::Os) }
        }
    }
    fn protection(value: u32) -> i32 {
        (if value & 1 != 0 { libc::PROT_READ } else { 0 }) | (if value & 2 != 0 { libc::PROT_WRITE } else { 0 }) | (if value & 4 != 0 { libc::PROT_EXEC } else { 0 })
    }
    impl port::NativeMapping for Std {
        fn page_size() -> usize { page() }
        unsafe fn allocate(size: usize, protection_bits: u32) -> Result<*mut c_void> {
            let value = unsafe { libc::mmap(ptr::null_mut(), size, protection(protection_bits), libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
            if value == libc::MAP_FAILED { Err(if errno() == libc::ENOMEM { Error::OutOfMemory } else { Error::Os }) } else { Ok(value) }
        }
        unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { libc::munmap(address, size) } == 0 { Ok(()) } else { Err(Error::Os) }
        }
        unsafe fn protect(address: *mut c_void, size: usize, protection_bits: u32) -> Result<()> {
            if unsafe { libc::mprotect(address, size, protection(protection_bits)) } == 0 { Ok(()) } else { Err(Error::Os) }
        }
    }
    impl port::Identity for Std {
        fn process_id() -> Result<u64> { Ok(std::process::id() as u64) }
        fn thread_id() -> Result<u64> {
            #[cfg(target_os = "linux")]
            { let id = unsafe { libc::syscall(libc::SYS_gettid) }; if id > 0 { Ok(id as u64) } else { Err(Error::Os) } }
            #[cfg(target_os = "macos")]
            { let mut id = 0u64; if unsafe { libc::pthread_threadid_np(0, &mut id) } == 0 && id != 0 { Ok(id) } else { Err(Error::Os) } }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            { Ok(unsafe { libc::pthread_self() } as u64) }
        }
    }
    impl port::Modules for Std {
        unsafe fn open(name: Option<&[u8]>) -> Result<*mut c_void> {
            let owned = name.map(|n| std::ffi::CString::new(n).map_err(|_| Error::InvalidArgument)).transpose()?;
            let module = unsafe { libc::dlopen(owned.as_ref().map_or(ptr::null(), |c| c.as_ptr()), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if module.is_null() { Err(Error::NotFound) } else { Ok(module) }
        }
        unsafe fn symbol(module: *mut c_void, name: &[u8]) -> Result<*mut c_void> {
            let name = std::ffi::CString::new(name).map_err(|_| Error::InvalidArgument)?;
            unsafe { libc::dlerror() };
            let symbol = unsafe { libc::dlsym(module, name.as_ptr()) };
            if !unsafe { libc::dlerror() }.is_null() { Err(Error::NotFound) } else { Ok(symbol) }
        }
        unsafe fn close(module: *mut c_void) -> Result<()> { if unsafe { libc::dlclose(module) } == 0 { Ok(()) } else { Err(Error::Os) } }
        unsafe fn info(address: *mut c_void) -> Result<ModuleInfo> {
            let mut info = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
            if unsafe { libc::dladdr(address, info.as_mut_ptr()) } == 0 { return Err(Error::NotFound); }
            let info = unsafe { info.assume_init() };
            if info.dli_fname.is_null() { return Err(Error::Os); }
            let name = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }.to_bytes();
            Ok(ModuleInfo { base: info.dli_fbase, name: info.dli_fname.cast(), name_length: name.len() })
        }
    }
    impl port::StackBounds for Std {
        fn current() -> Result<(*mut c_void, *mut c_void)> {
            #[cfg(target_os = "linux")]
            {
                let mut attr = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
                if unsafe { libc::pthread_getattr_np(libc::pthread_self(), attr.as_mut_ptr()) } != 0 { return Err(Error::Os); }
                let (mut base, mut size) = (ptr::null_mut(), 0);
                let rc = unsafe { libc::pthread_attr_getstack(attr.as_ptr(), &mut base, &mut size) };
                unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()) };
                if rc != 0 || base.is_null() || size == 0 { return Err(Error::Os); }
                Ok((base, (base as usize + size) as *mut c_void))
            }
            #[cfg(target_os = "macos")]
            {
                let me = unsafe { libc::pthread_self() };
                let high = unsafe { libc::pthread_get_stackaddr_np(me) };
                let size = unsafe { libc::pthread_get_stacksize_np(me) };
                if high.is_null() || size == 0 { return Err(Error::Os); }
                Ok(((high as usize - size) as *mut c_void, high))
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            { Err(Error::Unsupported) }
        }
    }
    impl port::ProcessBarrier for Std {
        fn available() -> bool {
            #[cfg(target_os = "linux")]
            { unsafe { libc::syscall(libc::SYS_membarrier, 16i32, 0i32) == 0 } }
            #[cfg(not(target_os = "linux"))]
            { false }
        }
        fn barrier() -> Result<()> {
            #[cfg(target_os = "linux")]
            { if unsafe { libc::syscall(libc::SYS_membarrier, 8i32, 0i32) } == 0 { Ok(()) } else { Err(Error::Os) } }
            #[cfg(not(target_os = "linux"))]
            { Err(Error::Unsupported) }
        }
    }
    impl port::ThreadName for Std {
        fn set(name: &[u8]) -> Result<()> {
            let limit = if cfg!(target_os = "linux") { 15 } else { 63 };
            if name.len() > limit { return Err(Error::InvalidArgument); }
            let name = std::ffi::CString::new(name).map_err(|_| Error::InvalidArgument)?;
            #[cfg(target_os = "linux")]
            let rc = unsafe { libc::pthread_setname_np(libc::pthread_self(), name.as_ptr()) };
            #[cfg(target_os = "macos")]
            let rc = unsafe { libc::pthread_setname_np(name.as_ptr()) };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let rc = { let _ = name; return Err(Error::Unsupported); };
            if rc == 0 { Ok(()) } else { Err(Error::Os) }
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::System::Memory::{VirtualAlloc, VirtualFree, VirtualProtect, MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, MEM_RESET,
        PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE};
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::{FlushProcessWriteBuffers, GetCurrentThread, GetCurrentThreadId, GetCurrentThreadStackLimits, SetThreadDescription};
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleExW, GetProcAddress, LoadLibraryA, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};
    fn page() -> usize { let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() }; unsafe { GetSystemInfo(&mut info) }; info.dwPageSize as usize }
    fn granularity() -> usize { let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() }; unsafe { GetSystemInfo(&mut info) }; info.dwAllocationGranularity as usize }
    impl port::VirtualMemory for Std {
        fn page_size() -> usize { page() }
        unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void> {
            // Reservations are released whole; over-reserve and retry to satisfy alignment.
            for _ in 0..16 {
                let total = size.checked_add(alignment).ok_or(Error::Os)?;
                let raw = unsafe { VirtualAlloc(ptr::null(), total, MEM_RESERVE, PAGE_NOACCESS) };
                if raw.is_null() { return Err(Error::Os); }
                let aligned = (raw as usize + alignment - 1) & !(alignment - 1);
                if aligned == raw as usize && alignment <= granularity() { return Ok(raw); }
                unsafe { VirtualFree(raw, 0, MEM_RELEASE) };
                let retry = unsafe { VirtualAlloc(aligned as *const c_void, size, MEM_RESERVE, PAGE_NOACCESS) };
                if !retry.is_null() { return Ok(retry); }
            }
            Err(Error::Os)
        }
        unsafe fn commit(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { VirtualAlloc(address, size, MEM_COMMIT, PAGE_READWRITE) }.is_null() { Err(Error::Os) } else { Ok(()) }
        }
        unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { VirtualFree(address, size, MEM_DECOMMIT) } == 0 { Err(Error::Os) } else { Ok(()) }
        }
        unsafe fn release(address: *mut c_void, _size: usize) -> Result<()> {
            if unsafe { VirtualFree(address, 0, MEM_RELEASE) } == 0 { Err(Error::Os) } else { Ok(()) }
        }
        unsafe fn reset(address: *mut c_void, size: usize) -> Result<()> {
            if unsafe { VirtualAlloc(address, size, MEM_RESET, PAGE_READWRITE) }.is_null() { Err(Error::Os) } else { Ok(()) }
        }
    }
    fn protection(bits: u32) -> u32 {
        match (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0) {
            (false, false, false) => PAGE_NOACCESS, (true, false, false) => PAGE_READONLY, (_, true, false) => PAGE_READWRITE,
            (false, false, true) => PAGE_EXECUTE, (true, false, true) => PAGE_EXECUTE_READ, (_, true, true) => PAGE_EXECUTE_READWRITE,
        }
    }
    impl port::NativeMapping for Std {
        fn page_size() -> usize { page() }
        unsafe fn allocate(size: usize, bits: u32) -> Result<*mut c_void> {
            let value = unsafe { VirtualAlloc(ptr::null(), size, MEM_RESERVE | MEM_COMMIT, protection(bits)) };
            if value.is_null() { Err(Error::OutOfMemory) } else { Ok(value) }
        }
        unsafe fn release(address: *mut c_void, _size: usize) -> Result<()> {
            if unsafe { VirtualFree(address, 0, MEM_RELEASE) } == 0 { Err(Error::Os) } else { Ok(()) }
        }
        unsafe fn protect(address: *mut c_void, size: usize, bits: u32) -> Result<()> {
            let mut old = 0;
            if unsafe { VirtualProtect(address, size, protection(bits), &mut old) } == 0 { Err(Error::Os) } else { Ok(()) }
        }
    }
    impl port::Identity for Std {
        fn process_id() -> Result<u64> { Ok(std::process::id() as u64) }
        fn thread_id() -> Result<u64> { Ok(unsafe { GetCurrentThreadId() } as u64) }
    }
    impl port::Modules for Std {
        unsafe fn open(name: Option<&[u8]>) -> Result<*mut c_void> {
            let module = match name {
                Some(name) => { let c = std::ffi::CString::new(name).map_err(|_| Error::InvalidArgument)?; unsafe { LoadLibraryA(c.as_ptr().cast()) } }
                None => { let mut m = ptr::null_mut(); unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, ptr::null(), &mut m) }; m }
            };
            if module.is_null() { Err(Error::NotFound) } else { Ok(module.cast()) }
        }
        unsafe fn symbol(module: *mut c_void, name: &[u8]) -> Result<*mut c_void> {
            let c = std::ffi::CString::new(name).map_err(|_| Error::InvalidArgument)?;
            match unsafe { GetProcAddress(module.cast(), c.as_ptr().cast()) } { Some(f) => Ok(f as *mut c_void), None => Err(Error::NotFound) }
        }
        unsafe fn close(module: *mut c_void) -> Result<()> { if unsafe { FreeLibrary(module.cast()) } == 0 { Err(Error::Os) } else { Ok(()) } }
        unsafe fn info(address: *mut c_void) -> Result<ModuleInfo> {
            let mut module = ptr::null_mut();
            let ok = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, address.cast(), &mut module) };
            if ok == 0 || module.is_null() { return Err(Error::NotFound); }
            // A stable name borrow is not available without allocation; report the base only.
            static NAME: &[u8] = b"module";
            Ok(ModuleInfo { base: module.cast(), name: NAME.as_ptr(), name_length: NAME.len() })
        }
    }
    impl port::StackBounds for Std {
        fn current() -> Result<(*mut c_void, *mut c_void)> {
            let (mut low, mut high) = (0usize, 0usize);
            unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
            if low == 0 || high <= low { Err(Error::Os) } else { Ok((low as *mut c_void, high as *mut c_void)) }
        }
    }
    impl port::ProcessBarrier for Std {
        fn barrier() -> Result<()> { unsafe { FlushProcessWriteBuffers() }; Ok(()) }
    }
    impl port::ThreadName for Std {
        fn set(name: &[u8]) -> Result<()> {
            let text = std::str::from_utf8(name).map_err(|_| Error::InvalidArgument)?;
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            if unsafe { SetThreadDescription(GetCurrentThread(), wide.as_ptr()) } < 0 { Err(Error::Os) } else { Ok(()) }
        }
    }
}

dotnet_pal_rs::declare_port! {
    pub struct StdPort;
    VirtualMemory = Std, Clock = Std, Scheduler = Std,
    Events = Std, Mutexes = Std, Threads = Std, ThreadLocal = Std, StackBounds = Std, ProcessBarrier = Std,
    Environment = Std, Identity = Std, Realtime = Std, Entropy = Std, NativeMapping = Std, Modules = Std,
    NativeHeap = Std, RwLocks = Std, ThreadName = Std, Diagnostics = Std, Abort = Std,
    Topology = Std, Process = Std, Image = system::ImageProvider, Streams = Std, Files = Std,
    Sockets = Std, Faults = faults::Provider, SystemInfo = Std, Notifications = Std, Processes = Std, Terminal = Std,
    Watches = Std, Mappings = Std, Volumes = Std, Network = Std
}

/// Negotiated table for [`StdPort`], usable from Rust without the C symbol.
pub fn api() -> *const dotnet_pal_rs::Api {
    static TABLE: dotnet_pal_rs::Slot = dotnet_pal_rs::Slot::new();
    dotnet_pal_rs::negotiate::<StdPort>(&TABLE, dotnet_pal_rs::ABI_VERSION)
}
#[cfg(feature = "entry")]
mod entry {
    static TABLE: dotnet_pal_rs::Slot = dotnet_pal_rs::Slot::new();
    /// The only runtime-facing PAL entry point. Valid before managed runtime startup.
    #[no_mangle]
    pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const dotnet_pal_rs::Api {
        dotnet_pal_rs::negotiate::<super::StdPort>(&TABLE, version)
    }
}
