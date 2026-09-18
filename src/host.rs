//! Providers backed by immutable C host tables (`dotnet_pal_host_*_v2`).
//!
//! An SDK provider written in C/C++ supplies the tables before runtime startup.
//! Each table is validated once at negotiation: version, declared size, the
//! capability bits the feature requires and every callback behind them. A
//! malformed required table rejects negotiation; nothing falls back to Linux.
//! Older host configurations need no new symbols: only the enabled `host-*`
//! features reference their provider function.
use crate::port::{self, Error, Result};
use crate::{Header, HostApi, ABI_VERSION, CAP_VM};
use core::{ffi::c_void, mem, ptr, sync::atomic::{AtomicPtr, Ordering}};

pub struct Host;

/// Reads a header-first table. The host contract guarantees a readable header
/// even for a rejected table, and `struct_size` readable bytes when it matches.
unsafe fn table<T>(raw: *const T, required: u64) -> Option<&'static T> {
    if raw.is_null() || (raw as usize) % mem::align_of::<T>() != 0 { return None; }
    let header = unsafe { ptr::read(raw.cast::<Header>()) };
    if header.abi_version != ABI_VERSION || (header.struct_size as usize) < mem::size_of::<T>()
        || header.capabilities & required != required { return None; }
    // Only form the full reference AFTER checking the advertised size.
    Some(unsafe { &*raw })
}
/// One validated table pointer, cached at negotiation. Signal-time callers read
/// the cache and never rediscover a foreign provider.
struct Cached<T>(AtomicPtr<T>);
impl<T> Cached<T> {
    const fn new() -> Self { Self(AtomicPtr::new(ptr::null_mut())) }
    fn get(&self) -> Option<&'static T> { unsafe { self.0.load(Ordering::Acquire).as_ref() } }
    fn set(&self, value: &'static T) { self.0.store(value as *const T as *mut T, Ordering::Release); }
}

extern "C" {
    fn dotnet_pal_host_v2() -> *const HostApi;
    fn dotnet_pal_host_abort() -> !;
}
static VM: Cached<HostApi> = Cached::new();
fn validate_vm() -> bool {
    let Some(api) = (unsafe { table(dotnet_pal_host_v2(), CAP_VM) }) else { return false; };
    let vm = &api.vm;
    if vm.page_size.is_none() || vm.reserve.is_none() || vm.commit.is_none()
        || vm.decommit.is_none() || vm.release.is_none() || vm.reset.is_none() { return false; }
    VM.set(api);
    true
}
impl port::VirtualMemory for Host {
    fn page_size() -> usize {
        match VM.get().and_then(|a| a.vm.page_size) { Some(f) => unsafe { f() }, None => 0 }
    }
    unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void> {
        let Some(f) = VM.get().and_then(|a| a.vm.reserve) else { return Err(Error::Unsupported); };
        // Do not expose an output accidentally written by a failing foreign callback.
        let mut result = ptr::null_mut();
        port::from_status(unsafe { f(size, alignment, 0, &mut result) })?;
        Ok(result)
    }
    unsafe fn commit(address: *mut c_void, size: usize) -> Result<()> {
        let Some(f) = VM.get().and_then(|a| a.vm.commit) else { return Err(Error::Unsupported); };
        port::from_status(unsafe { f(address, size) })
    }
    unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()> {
        let Some(f) = VM.get().and_then(|a| a.vm.decommit) else { return Err(Error::Unsupported); };
        port::from_status(unsafe { f(address, size) })
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        let Some(f) = VM.get().and_then(|a| a.vm.release) else { return Err(Error::Unsupported); };
        port::from_status(unsafe { f(address, size) })
    }
    unsafe fn reset(address: *mut c_void, size: usize) -> Result<()> {
        let Some(f) = VM.get().and_then(|a| a.vm.reset) else { return Err(Error::Unsupported); };
        port::from_status(unsafe { f(address, size) })
    }
}
impl port::Abort for Host { fn abort() -> ! { unsafe { dotnet_pal_host_abort() } } }

/// Validates every table the enabled features require. Called from the
/// standalone port's `validate`; a library port may reuse it.
pub fn validate() -> bool {
    #[cfg(feature = "host-machine")]
    if !machine_provider::validate() { return false; }
    if cfg!(not(feature = "linear")) && !validate_vm() { return false; }
    #[cfg(feature = "host-services")]
    if !services::validate() { return false; }
    #[cfg(feature = "host-kernel")]
    if !kernel::validate() { return false; }
    #[cfg(feature = "host-runtime")]
    if !runtime::validate() { return false; }
    #[cfg(feature = "host-support")]
    if !support::validate() { return false; }
    #[cfg(feature = "host-context")]
    if !context::validate() { return false; }
    true
}

#[cfg(feature = "host-services")]
pub mod services {
    use super::*;
    use crate::services::{CAP_CLOCK, CAP_SCHEDULER};
    #[repr(C)]
    pub struct Table {
        header: Header,
        clock: Option<unsafe extern "C" fn(*mut u64) -> u32>,
        sleep: Option<unsafe extern "C" fn(u64) -> u32>,
        yield_now: Option<unsafe extern "C" fn() -> u32>,
    }
    extern "C" { fn dotnet_pal_host_services_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_services_v2(), CAP_CLOCK | CAP_SCHEDULER) }) else { return false; };
        if t.clock.is_none() || t.sleep.is_none() || t.yield_now.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Clock for Host {
        fn monotonic_ns() -> Result<u64> {
            let Some(f) = TABLE.get().and_then(|t| t.clock) else { return Err(Error::Os); };
            let mut value = 0;
            port::from_status(unsafe { f(&mut value) })?;
            Ok(value)
        }
    }
    impl port::Scheduler for Host {
        fn sleep_ns(ns: u64) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.sleep) else { return Err(Error::Os); };
            port::from_status(unsafe { f(ns) })
        }
        fn yield_now() -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.yield_now) else { return Err(Error::Os); };
            port::from_status(unsafe { f() })
        }
    }
}

#[cfg(feature = "host-kernel")]
pub mod kernel {
    use super::*;
    use crate::kernel::{Destructor, Entry, HostKernel, ALL};
    extern "C" { fn dotnet_pal_host_kernel_v2() -> *const HostKernel; }
    static TABLE: Cached<HostKernel> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_kernel_v2(), ALL) }) else { return false; };
        let o = &t.ops;
        if o.event_create.is_none() || o.event_destroy.is_none() || o.event_set.is_none() || o.event_reset.is_none() || o.event_wait.is_none()
            || o.mutex_create.is_none() || o.mutex_destroy.is_none() || o.mutex_lock.is_none() || o.mutex_unlock.is_none()
            || o.thread_create.is_none() || o.thread_join.is_none() || o.thread_detach.is_none()
            || o.tls_create.is_none() || o.tls_destroy.is_none() || o.tls_get.is_none() || o.tls_set.is_none()
            || o.stack_bounds.is_none() || o.process_barrier.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    macro_rules! create {
        ($name:ident ($($arg:expr),*)) => {{
            let mut handle = ptr::null_mut();
            call!($name($($arg,)* &mut handle))?;
            Ok(handle)
        }};
    }
    impl port::Events for Host {
        unsafe fn create(manual: bool, initial: bool) -> Result<*mut c_void> { create!(event_create(manual as u32, initial as u32)) }
        unsafe fn destroy(h: *mut c_void) -> Result<()> { call!(event_destroy(h)) }
        unsafe fn set(h: *mut c_void) -> Result<()> { call!(event_set(h)) }
        unsafe fn reset(h: *mut c_void) -> Result<()> { call!(event_reset(h)) }
        unsafe fn wait(h: *mut c_void, ns: u64) -> Result<()> { call!(event_wait(h, ns)) }
    }
    impl port::Mutexes for Host {
        unsafe fn create(recursive: bool) -> Result<*mut c_void> { create!(mutex_create(recursive as u32)) }
        unsafe fn destroy(h: *mut c_void) -> Result<()> { call!(mutex_destroy(h)) }
        unsafe fn lock(h: *mut c_void) -> Result<()> { call!(mutex_lock(h)) }
        unsafe fn unlock(h: *mut c_void) -> Result<()> { call!(mutex_unlock(h)) }
    }
    impl port::Threads for Host {
        unsafe fn create(entry: Entry, arg: *mut c_void, stack: usize) -> Result<*mut c_void> { create!(thread_create(Some(entry), arg, stack)) }
        unsafe fn join(h: *mut c_void) -> Result<()> { call!(thread_join(h)) }
        unsafe fn detach(h: *mut c_void) -> Result<()> { call!(thread_detach(h)) }
    }
    impl port::ThreadLocal for Host {
        unsafe fn create(dtor: Option<Destructor>) -> Result<*mut c_void> { create!(tls_create(dtor)) }
        unsafe fn destroy(h: *mut c_void) -> Result<()> { call!(tls_destroy(h)) }
        unsafe fn get(h: *mut c_void) -> Result<*mut c_void> {
            let mut value = ptr::null_mut();
            call!(tls_get(h, &mut value))?;
            Ok(value)
        }
        unsafe fn set(h: *mut c_void, value: *mut c_void) -> Result<()> { call!(tls_set(h, value)) }
    }
    impl port::StackBounds for Host {
        fn current() -> Result<(*mut c_void, *mut c_void)> {
            let (mut low, mut high) = (ptr::null_mut(), ptr::null_mut());
            call!(stack_bounds(&mut low, &mut high))?;
            Ok((low, high))
        }
    }
    impl port::ProcessBarrier for Host {
        fn barrier() -> Result<()> { call!(process_barrier()) }
    }
}

#[cfg(feature = "host-runtime")]
pub mod runtime {
    use super::*;
    use crate::port::Lookup;
    use crate::runtime::{Host as Table, ModuleInfo, ALL};
    extern "C" { fn dotnet_pal_host_runtime_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_runtime_v2(), ALL) }) else { return false; };
        let ops = &t.ops;
        if ops.environment_get.is_none() || ops.process_id.is_none() || ops.thread_id.is_none()
            || ops.realtime_ns.is_none() || ops.random_bytes.is_none() || ops.mapping_allocate.is_none()
            || ops.mapping_release.is_none() || ops.mapping_protect.is_none() || ops.module_open.is_none()
            || ops.module_symbol.is_none() || ops.module_close.is_none() || ops.module_info.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    fn scalar(f: Option<unsafe extern "C" fn(*mut u64) -> u32>) -> Result<u64> {
        let Some(f) = f else { return Err(Error::Unsupported); };
        let mut value = 0;
        port::from_status(unsafe { f(&mut value) })?;
        Ok(value)
    }
    impl port::Environment for Host {
        unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.environment_get) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            match port::from_status(unsafe { f(name.as_ptr(), name.len(), out, capacity, &mut needed) }) {
                Ok(()) => Ok(Lookup::Copied(needed)),
                Err(Error::BufferTooSmall) => Ok(Lookup::TooSmall(needed)),
                Err(e) => Err(e),
            }
        }
    }
    impl port::Identity for Host {
        fn process_id() -> Result<u64> { scalar(TABLE.get().and_then(|t| t.ops.process_id)) }
        fn thread_id() -> Result<u64> { scalar(TABLE.get().and_then(|t| t.ops.thread_id)) }
    }
    impl port::Realtime for Host {
        fn realtime_ns() -> Result<u64> { scalar(TABLE.get().and_then(|t| t.ops.realtime_ns)) }
    }
    impl port::Entropy for Host {
        unsafe fn fill(out: *mut u8, size: usize) -> Result<()> { call!(random_bytes(out, size)) }
    }
    impl port::NativeMapping for Host {
        fn page_size() -> usize { <Host as port::VirtualMemory>::page_size() }
        unsafe fn allocate(size: usize, protection: u32) -> Result<*mut c_void> {
            let mut value = ptr::null_mut();
            call!(mapping_allocate(size, protection, &mut value))?;
            Ok(value)
        }
        unsafe fn release(address: *mut c_void, size: usize) -> Result<()> { call!(mapping_release(address, size)) }
        unsafe fn protect(address: *mut c_void, size: usize, protection: u32) -> Result<()> { call!(mapping_protect(address, size, protection)) }
    }
    impl port::Modules for Host {
        unsafe fn open(name: Option<&[u8]>) -> Result<*mut c_void> {
            let (p, len) = match name { Some(n) => (n.as_ptr(), n.len()), None => (ptr::null(), 0) };
            let mut handle = ptr::null_mut();
            call!(module_open(p, len, &mut handle))?;
            Ok(handle)
        }
        unsafe fn symbol(module: *mut c_void, name: &[u8]) -> Result<*mut c_void> {
            let mut value = ptr::null_mut();
            call!(module_symbol(module, name.as_ptr(), name.len(), &mut value))?;
            Ok(value)
        }
        unsafe fn close(module: *mut c_void) -> Result<()> { call!(module_close(module)) }
        unsafe fn info(address: *mut c_void) -> Result<ModuleInfo> {
            let mut value = ModuleInfo { base: ptr::null_mut(), name: ptr::null(), name_length: 0 };
            call!(module_info(address, &mut value))?;
            Ok(value)
        }
    }
}

#[cfg(feature = "host-support")]
pub mod support {
    use super::*;
    use crate::support::{Host as Table, ALL};
    extern "C" { fn dotnet_pal_host_support_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_support_v2(), ALL) }) else { return false; };
        let o = &t.ops;
        if o.allocate.is_none() || o.resize.is_none() || o.release.is_none() || o.rw_create.is_none()
            || o.rw_read.is_none() || o.rw_write.is_none() || o.rw_unlock.is_none() || o.rw_destroy.is_none()
            || o.write_stderr.is_none() || o.thread_name.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    impl port::NativeHeap for Host {
        unsafe fn allocate(size: usize, zero: bool) -> Result<*mut c_void> {
            let mut result = ptr::null_mut();
            call!(allocate(size, zero as u32, &mut result))?;
            Ok(result)
        }
        unsafe fn resize(address: *mut c_void, size: usize) -> Result<*mut c_void> {
            let mut result = ptr::null_mut();
            call!(resize(address, size, &mut result))?;
            Ok(result)
        }
        unsafe fn release(address: *mut c_void) -> Result<()> { call!(release(address)) }
    }
    impl port::RwLocks for Host {
        unsafe fn create() -> Result<*mut c_void> {
            let mut result = ptr::null_mut();
            call!(rw_create(&mut result))?;
            Ok(result)
        }
        unsafe fn read(h: *mut c_void) -> Result<()> { call!(rw_read(h)) }
        unsafe fn write(h: *mut c_void) -> Result<()> { call!(rw_write(h)) }
        unsafe fn unlock(h: *mut c_void) -> Result<()> { call!(rw_unlock(h)) }
        unsafe fn destroy(h: *mut c_void) -> Result<()> { call!(rw_destroy(h)) }
    }
    impl port::Diagnostics for Host {
        unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.write_stderr) else { return Err((0, Error::Unsupported)); };
            let mut done = 0;
            match port::from_status(unsafe { f(data, size, &mut done) }) {
                Ok(()) if done == size => Ok(()),
                Ok(()) => Err((0, Error::Os)), // short successful report is a broken host: progress is unknowable
                Err(e) => Err((done.min(size), e)),
            }
        }
    }
    impl port::ThreadName for Host {
        fn set(name: &[u8]) -> Result<()> { call!(thread_name(name.as_ptr(), name.len())) }
    }
}

#[cfg(feature = "host-context")]
pub mod context {
    use super::*;
    use crate::context::{Host as Table, Ops, CAP};
    extern "C" { fn dotnet_pal_host_context_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_context_v2(), CAP) }) else { return false; };
        let ops = &t.ops;
        if ops.abi_tag.is_none() || ops.action_size.is_none() || ops.action_alignment.is_none() || ops.install.is_none() || ops.restore.is_none()
            || ops.unblock_activation.is_none() || ops.request_activation.is_none() || ops.current_thread.is_none()
            || ops.process_id_async.is_none() || ops.ignore_broken_pipe.is_none() || ops.signal_number.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::SignalContext for Host {
        // Signal callbacks read the already-validated table; never call a host getter.
        fn ops() -> Option<&'static Ops> { TABLE.get().map(|t| &t.ops) }
    }
}


#[cfg(feature = "host-machine")]
mod machine_provider {
    use super::*;
    use crate::machine::{CpuList, Host as Table, CAP};
    static MACHINE: Cached<Table> = Cached::new();
    extern "C" { fn dotnet_pal_host_machine_v2() -> *const Table; }
    pub fn validate() -> bool {
        let Some(api) = (unsafe { table(dotnet_pal_host_machine_v2(), CAP) }) else { return false; };
        if api.ops.query.is_none() || api.ops.process_affinity.is_none() || api.ops.bind_current.is_none() || api.ops.current_cpu.is_none() { return false; }
        MACHINE.set(api); true
    }
    impl port::Machine for Host {
        fn query(kind: u32) -> Result<u64> {
            let f = MACHINE.get().and_then(|a| a.ops.query).ok_or(Error::Unsupported)?;
            let mut value = 0;
            port::from_status(unsafe { f(kind, &mut value) })?;
            Ok(value)
        }
        unsafe fn process_affinity(out: *mut u32, capacity: usize) -> Result<CpuList> {
            let f = MACHINE.get().and_then(|a| a.ops.process_affinity).ok_or(Error::Unsupported)?;
            let mut n = 0;
            match unsafe { f(out, capacity, &mut n) } {
                crate::OK => Ok(CpuList::Written(n)),
                crate::runtime::BUFFER_TOO_SMALL => Ok(CpuList::Required(n)),
                code => Err(Error::from_status(code).unwrap_or(Error::Os)),
            }
        }
        fn bind_current(cpu: u32) -> Result<()> {
            let f = MACHINE.get().and_then(|a| a.ops.bind_current).ok_or(Error::Unsupported)?;
            port::from_status(unsafe { f(cpu) })
        }
        fn current_cpu() -> Result<u32> {
            let f = MACHINE.get().and_then(|a| a.ops.current_cpu).ok_or(Error::Unsupported)?;
            let mut cpu = u32::MAX;
            port::from_status(unsafe { f(&mut cpu) })?; Ok(cpu)
        }
    }
}
