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
    #[cfg(feature = "host-topology")]
    if !topology::validate() { return false; }
    #[cfg(feature = "host-process")]
    if !process::validate() { return false; }
    #[cfg(feature = "host-image")]
    if !image::validate() { return false; }
    #[cfg(feature = "host-streams")]
    if !streams::validate() { return false; }
    #[cfg(feature = "host-files")]
    if !files::validate() { return false; }
    #[cfg(feature = "host-sockets")]
    if !sockets::validate() { return false; }
    #[cfg(feature = "host-faults")]
    if !faults::validate() { return false; }
    #[cfg(feature = "host-system")]
    if !system::validate() { return false; }
    #[cfg(feature = "host-notifications")]
    if !notifications::validate() { return false; }
    #[cfg(feature = "host-processes")]
    if !processes::validate() { return false; }
    #[cfg(feature = "host-terminal")]
    if !terminal::validate() { return false; }
    #[cfg(feature = "host-watches")]
    if !watches::validate() { return false; }
    #[cfg(feature = "host-mappings")]
    if !mappings::validate() { return false; }
    #[cfg(feature = "host-volumes")]
    if !volumes::validate() { return false; }
    #[cfg(feature = "host-network")]
    if !network::validate() { return false; }
    #[cfg(feature = "host-local-sockets")]
    if !local_sockets::validate() { return false; }
    #[cfg(feature = "host-accounts")]
    if !accounts::validate() { return false; }
    #[cfg(feature = "host-priority")]
    if !priority::validate() { return false; }
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

#[cfg(feature = "host-topology")]
pub mod topology {
    use super::*;
    use crate::topology::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_topology_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_topology_v2(), CAP) }) else { return false; };
        let o = &t.ops;
        if o.cpu_max.is_none() || o.cpu_count.is_none() || o.current_cpu.is_none() || o.process_affinity.is_none()
            || o.set_thread_affinity.is_none() || o.physical_memory.is_none() || o.memory_limit.is_none()
            || o.virtual_limit.is_none() || o.cache_size.is_none() || o.cpu_features.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    fn count(f: Option<unsafe extern "C" fn(*mut u32) -> u32>) -> Result<u32> {
        let Some(f) = f else { return Err(Error::Unsupported); };
        let mut value = 0;
        port::from_status(unsafe { f(&mut value) })?;
        Ok(value)
    }
    fn limit(f: Option<unsafe extern "C" fn(*mut u64) -> u32>) -> Result<u64> {
        let Some(f) = f else { return Err(Error::Unsupported); };
        let mut value = 0;
        port::from_status(unsafe { f(&mut value) })?;
        Ok(value)
    }
    impl port::Topology for Host {
        fn cpu_max() -> Result<u32> { count(TABLE.get().and_then(|t| t.ops.cpu_max)) }
        fn cpu_count() -> Result<u32> { count(TABLE.get().and_then(|t| t.ops.cpu_count)) }
        fn current_cpu() -> Result<u32> { count(TABLE.get().and_then(|t| t.ops.current_cpu)) }
        unsafe fn process_affinity(mask: *mut u8, capacity: usize) -> Result<usize> {
            let mut needed = 0;
            match call!(process_affinity(mask, capacity, &mut needed)) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        fn set_thread_affinity(cpu: u32) -> Result<()> { call!(set_thread_affinity(cpu)) }
        fn physical_memory() -> Result<(u64, u64)> {
            let (mut total, mut available) = (0, 0);
            call!(physical_memory(&mut total, &mut available))?;
            Ok((total, available))
        }
        fn memory_limit() -> Result<u64> { limit(TABLE.get().and_then(|t| t.ops.memory_limit)) }
        fn virtual_limit() -> Result<u64> { limit(TABLE.get().and_then(|t| t.ops.virtual_limit)) }
        fn cache_size() -> Result<usize> {
            let mut value = 0;
            call!(cache_size(&mut value))?;
            Ok(value)
        }
        fn cpu_features() -> Result<(u64, u64)> {
            let (mut first, mut second) = (0, 0);
            call!(cpu_features(&mut first, &mut second))?;
            Ok((first, second))
        }
    }
}

#[cfg(feature = "host-process")]
pub mod process {
    use super::*;
    use crate::process::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_process_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_process_v2(), CAP) }) else { return false; };
        // A host may omit crash dumps (UNSUPPORTED per call) but never exit.
        if t.ops.exit.is_none() || t.ops.debugger_present.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Process for Host {
        fn exit(code: i32) -> ! {
            if let Some(f) = TABLE.get().and_then(|t| t.ops.exit) { unsafe { f(code) }; }
            <Host as port::Abort>::abort()
        }
        fn debugger_present() -> Result<bool> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.debugger_present) else { return Err(Error::Unsupported); };
            let mut value = 0;
            port::from_status(unsafe { f(&mut value) })?;
            Ok(value != 0)
        }
        unsafe fn crash_dump(argv: &[*const u8], error: *mut u8, capacity: usize) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.crash_dump) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(argv.as_ptr(), argv.len(), error, capacity) })
        }
    }
}

#[cfg(feature = "host-image")]
pub mod image {
    use super::*;
    use crate::image::{Host as Table, UnwindInfo, CAP};
    extern "C" { fn dotnet_pal_host_image_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_image_v2(), CAP) }) else { return false; };
        if t.ops.unwind_info.is_none() || t.ops.readable.is_none() || t.ops.build_id.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Image for Host {
        unsafe fn unwind_info(address: usize) -> Result<UnwindInfo> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.unwind_info) else { return Err(Error::Unsupported); };
            let mut value = UnwindInfo::default();
            port::from_status(unsafe { f(address, &mut value, mem::size_of::<UnwindInfo>()) })?;
            Ok(value)
        }
        unsafe fn readable(address: usize, size: usize) -> Result<bool> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.readable) else { return Err(Error::Unsupported); };
            crate::image::readable_from_status(unsafe { f(address, size) })
        }
        unsafe fn build_id(base: usize, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.build_id) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            match port::from_status(unsafe { f(base, out, capacity, &mut needed) }) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
    }
}

#[cfg(feature = "host-streams")]
pub mod streams {
    use super::*;
    use crate::streams::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_streams_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_streams_v2(), CAP) }) else { return false; };
        if t.ops.write.is_none() || t.ops.read.is_none() || t.ops.is_terminal.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Streams for Host {
        unsafe fn write(stream: u32, data: *const u8, size: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.write) else { return Err(Error::Unsupported); };
            let mut written = 0;
            port::from_status(unsafe { f(stream, data, size, &mut written) })?;
            Ok(written)
        }
        unsafe fn read(stream: u32, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.read) else { return Err(Error::Unsupported); };
            let mut got = 0;
            port::from_status(unsafe { f(stream, out, capacity, &mut got) })?;
            Ok(got)
        }
        fn is_terminal(stream: u32) -> Result<bool> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.is_terminal) else { return Err(Error::Unsupported); };
            let mut value = 0;
            port::from_status(unsafe { f(stream, &mut value) })?;
            Ok(value != 0)
        }
    }
}

#[cfg(feature = "host-files")]
pub mod files {
    use super::*;
    use crate::files::{Host as Table, Status, CAP};
    extern "C" { fn dotnet_pal_host_files_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_files_v2(), CAP) }) else { return false; };
        let o = &t.ops;
        if o.open.is_none() || o.close.is_none() || o.read_at.is_none() || o.write_at.is_none() || o.set_size.is_none() || o.flush.is_none()
            || o.status.is_none() || o.path_status.is_none() || o.remove.is_none() || o.rename.is_none() || o.directory_create.is_none()
            || o.directory_remove.is_none() || o.directory_open.is_none() || o.directory_read.is_none() || o.directory_close.is_none()
            || o.current_directory.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    impl port::Files for Host {
        unsafe fn open(path: &[u8], flags: u32, mode: u32) -> Result<*mut c_void> {
            let mut file = ptr::null_mut();
            call!(open(path.as_ptr(), path.len(), flags, mode, &mut file))?;
            Ok(file)
        }
        unsafe fn close(file: *mut c_void) -> Result<()> { call!(close(file)) }
        unsafe fn read_at(file: *mut c_void, offset: u64, out: *mut u8, capacity: usize) -> Result<usize> {
            let mut got = 0;
            call!(read_at(file, offset, out, capacity, &mut got))?;
            Ok(got)
        }
        unsafe fn write_at(file: *mut c_void, offset: u64, data: *const u8, size: usize) -> Result<usize> {
            let mut written = 0;
            call!(write_at(file, offset, data, size, &mut written))?;
            Ok(written)
        }
        unsafe fn set_size(file: *mut c_void, size: u64) -> Result<()> { call!(set_size(file, size)) }
        unsafe fn flush(file: *mut c_void) -> Result<()> { call!(flush(file)) }
        unsafe fn status(file: *mut c_void) -> Result<Status> {
            let mut value = Status::default();
            call!(status(file, &mut value, mem::size_of::<Status>()))?;
            Ok(value)
        }
        unsafe fn path_status(path: &[u8], follow: bool) -> Result<Status> {
            let mut value = Status::default();
            call!(path_status(path.as_ptr(), path.len(), follow as u32, &mut value, mem::size_of::<Status>()))?;
            Ok(value)
        }
        unsafe fn remove(path: &[u8]) -> Result<()> { call!(remove(path.as_ptr(), path.len())) }
        unsafe fn rename(from: &[u8], to: &[u8]) -> Result<()> { call!(rename(from.as_ptr(), from.len(), to.as_ptr(), to.len())) }
        unsafe fn create_directory(path: &[u8], mode: u32) -> Result<()> { call!(directory_create(path.as_ptr(), path.len(), mode)) }
        unsafe fn remove_directory(path: &[u8]) -> Result<()> { call!(directory_remove(path.as_ptr(), path.len())) }
        unsafe fn open_directory(path: &[u8]) -> Result<*mut c_void> {
            let mut directory = ptr::null_mut();
            call!(directory_open(path.as_ptr(), path.len(), &mut directory))?;
            Ok(directory)
        }
        unsafe fn read_directory(directory: *mut c_void, name: *mut u8, capacity: usize) -> Result<(usize, u32)> {
            let (mut length, mut kind) = (0, 0);
            call!(directory_read(directory, name, capacity, &mut length, &mut kind))?;
            Ok((length, kind))
        }
        unsafe fn close_directory(directory: *mut c_void) -> Result<()> { call!(directory_close(directory)) }
        unsafe fn current_directory(out: *mut u8, capacity: usize) -> Result<usize> {
            let mut needed = 0;
            match call!(current_directory(out, capacity, &mut needed)) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        // Optional operations: a host that leaves the callback NULL has no such facility.
        unsafe fn set_mode(path: &[u8], mode: u32) -> Result<()> { call!(set_mode(path.as_ptr(), path.len(), mode)) }
        unsafe fn set_file_mode(file: *mut c_void, mode: u32) -> Result<()> { call!(set_file_mode(file, mode)) }
        unsafe fn set_times(path: &[u8], follow: bool, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
            call!(set_times(path.as_ptr(), path.len(), follow as u32, accessed_ns.unwrap_or(crate::files::TIME_KEEP), modified_ns.unwrap_or(crate::files::TIME_KEEP)))
        }
        unsafe fn set_file_times(file: *mut c_void, accessed_ns: Option<u64>, modified_ns: Option<u64>) -> Result<()> {
            call!(set_file_times(file, accessed_ns.unwrap_or(crate::files::TIME_KEEP), modified_ns.unwrap_or(crate::files::TIME_KEEP)))
        }
        unsafe fn link(existing: &[u8], created: &[u8]) -> Result<()> { call!(link(existing.as_ptr(), existing.len(), created.as_ptr(), created.len())) }
        unsafe fn symlink(target: &[u8], created: &[u8]) -> Result<()> { call!(symlink(target.as_ptr(), target.len(), created.as_ptr(), created.len())) }
        unsafe fn read_link(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
            let mut needed = 0;
            match call!(read_link(path.as_ptr(), path.len(), out, capacity, &mut needed)) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        unsafe fn real_path(path: &[u8], out: *mut u8, capacity: usize) -> Result<usize> {
            let mut needed = 0;
            match call!(real_path(path.as_ptr(), path.len(), out, capacity, &mut needed)) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        unsafe fn set_current_directory(path: &[u8]) -> Result<()> { call!(set_current_directory(path.as_ptr(), path.len())) }
        unsafe fn lock(file: *mut c_void, mode: u32, wait: bool) -> Result<()> { call!(lock(file, mode, wait as u32)) }
        unsafe fn lock_range(file: *mut c_void, offset: u64, length: u64, mode: u32) -> Result<()> { call!(lock_range(file, offset, length, mode)) }
    }
}

#[cfg(feature = "host-sockets")]
pub mod sockets {
    use super::*;
    use crate::sockets::{Address, Host as Table, PollEntry, CAP};
    extern "C" { fn dotnet_pal_host_sockets_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_sockets_v2(), CAP) }) else { return false; };
        let o = &t.ops;
        // Name resolution and the host name are per-call UNSUPPORTED for a host without them; the rest is the capability.
        if o.create.is_none() || o.close.is_none() || o.bind.is_none() || o.listen.is_none() || o.accept.is_none() || o.connect.is_none()
            || o.send.is_none() || o.receive.is_none() || o.shutdown.is_none() || o.local_address.is_none() || o.peer_address.is_none()
            || o.set_blocking.is_none() || o.get_option.is_none() || o.set_option.is_none() || o.poll.is_none() || o.wake.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    fn optional(address: Option<&Address>) -> *const Address { address.map_or(ptr::null(), |a| a as *const Address) }
    impl port::Sockets for Host {
        unsafe fn create(family: u32, kind: u32) -> Result<*mut c_void> {
            let mut socket = ptr::null_mut();
            call!(create(family, kind, &mut socket))?;
            Ok(socket)
        }
        unsafe fn close(socket: *mut c_void) -> Result<()> { call!(close(socket)) }
        unsafe fn bind(socket: *mut c_void, address: &Address) -> Result<()> { call!(bind(socket, address)) }
        unsafe fn listen(socket: *mut c_void, backlog: u32) -> Result<()> { call!(listen(socket, backlog)) }
        unsafe fn accept(socket: *mut c_void) -> Result<(*mut c_void, Address)> {
            let (mut accepted, mut peer) = (ptr::null_mut(), Address::default());
            call!(accept(socket, &mut accepted, &mut peer))?;
            Ok((accepted, peer))
        }
        unsafe fn connect(socket: *mut c_void, address: &Address) -> Result<()> { call!(connect(socket, address)) }
        unsafe fn send(socket: *mut c_void, data: *const u8, size: usize, to: Option<&Address>) -> Result<usize> {
            let mut sent = 0;
            call!(send(socket, data, size, optional(to), &mut sent))?;
            Ok(sent)
        }
        unsafe fn receive(socket: *mut c_void, out: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<Address>)> {
            let (mut got, mut from) = (0, Address::default());
            call!(receive(socket, out, capacity, flags, &mut from, &mut got))?;
            Ok((got, (from.family != 0).then_some(from)))
        }
        unsafe fn shutdown(socket: *mut c_void, how: u32) -> Result<()> { call!(shutdown(socket, how)) }
        unsafe fn local_address(socket: *mut c_void) -> Result<Address> {
            let mut value = Address::default();
            call!(local_address(socket, &mut value))?;
            Ok(value)
        }
        unsafe fn peer_address(socket: *mut c_void) -> Result<Address> {
            let mut value = Address::default();
            call!(peer_address(socket, &mut value))?;
            Ok(value)
        }
        unsafe fn set_blocking(socket: *mut c_void, blocking: bool) -> Result<()> { call!(set_blocking(socket, blocking as u32)) }
        unsafe fn get_option(socket: *mut c_void, option: u32) -> Result<u64> {
            let mut value = 0;
            call!(get_option(socket, option, &mut value))?;
            Ok(value)
        }
        unsafe fn set_option(socket: *mut c_void, option: u32, value: u64) -> Result<()> { call!(set_option(socket, option, value)) }
        unsafe fn poll(entries: &mut [PollEntry], timeout_ns: u64, channel: Option<u32>) -> Result<()> {
            // The front end recounts from the entries; the host's own count is not trusted.
            let mut ready = 0;
            call!(poll(entries.as_mut_ptr(), entries.len(), timeout_ns, channel.unwrap_or(crate::sockets::NO_CHANNEL), &mut ready))
        }
        fn wake(channel: u32) -> Result<()> { call!(wake(channel)) }
        unsafe fn resolve(name: &[u8], family: u32, out: *mut Address, capacity: usize) -> Result<usize> {
            let mut count = 0;
            call!(resolve(name.as_ptr(), name.len(), family, out, capacity, &mut count))?;
            Ok(count)
        }
        unsafe fn host_name(out: *mut u8, capacity: usize) -> Result<usize> {
            let mut needed = 0;
            match call!(host_name(out, capacity, &mut needed)) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
    }
}

#[cfg(feature = "host-faults")]
pub mod faults {
    use super::*;
    use crate::faults::{deliver_raw, Frame, Host as Table, CAP, FRAME_TAG};
    extern "C" { fn dotnet_pal_host_faults_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    /// The host must describe exactly the frame this build of the boundary reads.
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_faults_v2(), CAP) }) else { return false; };
        let (Some(tag), Some(size), Some(_)) = (t.ops.frame_tag, t.ops.frame_size, t.ops.enable) else { return false; };
        if FRAME_TAG == 0 || tag() != FRAME_TAG || size() != mem::size_of::<Frame>() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Faults for Host {
        fn enable() -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.enable) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(Some(deliver_raw)) })
        }
    }
}

#[cfg(feature = "host-system")]
pub mod system {
    use super::*;
    use crate::system::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_system_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    /// Every question is optional per call, so a host may leave any callback NULL.
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_system_v2(), CAP) }) else { return false; };
        TABLE.set(t);
        true
    }
    fn text_result(status: Result<()>, needed: usize) -> Result<usize> { match status { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) } }
    impl port::SystemInfo for Host {
        unsafe fn environment_entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.environment_entry) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            text_result(port::from_status(unsafe { f(index, out, capacity, &mut needed) }), needed)
        }
        unsafe fn text(what: u32, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.text) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            text_result(port::from_status(unsafe { f(what, out, capacity, &mut needed) }), needed)
        }
        fn process_times() -> Result<(u64, u64)> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.process_times) else { return Err(Error::Unsupported); };
            let (mut user, mut kernel) = (0, 0);
            port::from_status(unsafe { f(&mut user, &mut kernel) })?;
            Ok((user, kernel))
        }
        fn uptime_ns() -> Result<u64> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.uptime_ns) else { return Err(Error::Unsupported); };
            let mut value = 0;
            port::from_status(unsafe { f(&mut value) })?;
            Ok(value)
        }
        fn user_ids() -> Result<(u32, u32)> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.user_ids) else { return Err(Error::Unsupported); };
            let (mut user, mut group) = (0, 0);
            port::from_status(unsafe { f(&mut user, &mut group) })?;
            Ok((user, group))
        }
    }
}

#[cfg(feature = "host-notifications")]
pub mod notifications {
    use super::*;
    use crate::notifications::{deliver_raw, Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_notifications_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_notifications_v2(), CAP) }) else { return false; };
        if t.ops.start.is_none() || t.ops.enable.is_none() || t.ops.disable.is_none() || t.ops.default_action.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    impl port::Notifications for Host {
        fn start() -> Result<()> { call!(start(Some(deliver_raw))) }
        fn enable(kind: u32) -> Result<()> { call!(enable(kind)) }
        fn disable(kind: u32) -> Result<()> { call!(disable(kind)) }
        fn default_action(kind: u32) -> Result<()> { call!(default_action(kind)) }
    }
}

#[cfg(feature = "host-processes")]
pub mod processes {
    use super::*;
    use crate::processes::{Host as Table, Spawned, CAP};
    extern "C" { fn dotnet_pal_host_processes_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_processes_v2(), CAP) }) else { return false; };
        let o = &t.ops;
        if o.spawn.is_none() || o.wait.is_none() || o.terminate.is_none() || o.release.is_none() || o.pipe_read.is_none() || o.pipe_write.is_none()
            || o.pipe_close.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    impl port::Processes for Host {
        unsafe fn spawn(program: &[u8], arguments: &[*const u8], environment: Option<&[*const u8]>, directory: Option<&[u8]>, pipes: u32) -> Result<Spawned> {
            let mut spawned = Spawned::EMPTY;
            let (environment, environment_count) = environment.map_or((ptr::null(), 0), |list| (list.as_ptr(), list.len()));
            let (directory, directory_length) = directory.map_or((ptr::null(), 0), |path| (path.as_ptr(), path.len()));
            call!(spawn(program.as_ptr(), program.len(), arguments.as_ptr(), arguments.len(), environment, environment_count, directory, directory_length,
                pipes, &mut spawned, mem::size_of::<Spawned>()))?;
            Ok(spawned)
        }
        unsafe fn wait(process: *mut c_void, timeout_ns: u64) -> Result<i32> {
            let mut code = 0;
            call!(wait(process, timeout_ns, &mut code))?;
            Ok(code)
        }
        unsafe fn terminate(process: *mut c_void, forceful: bool) -> Result<()> { call!(terminate(process, forceful as u32)) }
        unsafe fn release(process: *mut c_void) -> Result<()> { call!(release(process)) }
        unsafe fn pipe_read(pipe: *mut c_void, out: *mut u8, capacity: usize) -> Result<usize> {
            let mut got = 0;
            call!(pipe_read(pipe, out, capacity, &mut got))?;
            Ok(got)
        }
        unsafe fn pipe_write(pipe: *mut c_void, data: *const u8, size: usize) -> Result<usize> {
            let mut written = 0;
            call!(pipe_write(pipe, data, size, &mut written))?;
            Ok(written)
        }
        unsafe fn pipe_close(pipe: *mut c_void) -> Result<()> { call!(pipe_close(pipe)) }
    }
}

#[cfg(feature = "host-terminal")]
pub mod terminal {
    use super::*;
    use crate::terminal::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_terminal_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_terminal_v2(), CAP) }) else { return false; };
        if t.ops.window_size.is_none() || t.ops.set_input_mode.is_none() || t.ops.input_ready.is_none() || t.ops.control_character.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Terminal for Host {
        fn window_size(stream: u32) -> Result<(u32, u32)> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.window_size) else { return Err(Error::Unsupported); };
            let (mut columns, mut rows) = (0, 0);
            port::from_status(unsafe { f(stream, &mut columns, &mut rows) })?;
            Ok((columns, rows))
        }
        fn set_input_mode(raw: bool, min_bytes: u8, timeout_ds: u8, interrupt_as_input: bool) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.set_input_mode) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(raw as u32, min_bytes as u32, timeout_ds as u32, interrupt_as_input as u32) })
        }
        fn input_ready() -> Result<bool> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.input_ready) else { return Err(Error::Unsupported); };
            let mut ready = 0;
            port::from_status(unsafe { f(&mut ready) })?;
            Ok(ready != 0)
        }
        fn control_character(which: u32) -> Result<u8> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.control_character) else { return Err(Error::Unsupported); };
            let mut value = 0;
            port::from_status(unsafe { f(which, &mut value) })?;
            u8::try_from(value).map_err(|_| Error::Os)
        }
    }
}

#[cfg(feature = "host-watches")]
pub mod watches {
    use super::*;
    use crate::watches::{Event, Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_watches_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_watches_v2(), CAP) }) else { return false; };
        if t.ops.open.is_none() || t.ops.close.is_none() || t.ops.add.is_none() || t.ops.remove.is_none() || t.ops.read.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    impl port::Watches for Host {
        fn open() -> Result<*mut c_void> {
            let mut watcher = ptr::null_mut();
            call!(open(&mut watcher))?;
            Ok(watcher)
        }
        unsafe fn close(watcher: *mut c_void) -> Result<()> { call!(close(watcher)) }
        unsafe fn add(watcher: *mut c_void, path: &[u8], events: u32) -> Result<u32> {
            let mut watch = 0;
            call!(add(watcher, path.as_ptr(), path.len(), events, &mut watch))?;
            Ok(watch)
        }
        unsafe fn remove(watcher: *mut c_void, watch: u32) -> Result<()> { call!(remove(watcher, watch)) }
        unsafe fn read(watcher: *mut c_void, timeout_ns: u64, event: &mut Event) -> Result<()> {
            call!(read(watcher, timeout_ns, event, core::mem::size_of::<Event>()))
        }
    }
}

#[cfg(feature = "host-mappings")]
pub mod mappings {
    use super::*;
    use crate::mappings::{Host as Table, CAP, PRIVATE, SHARED};
    extern "C" { fn dotnet_pal_host_mappings_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_mappings_v2(), CAP) }) else { return false; };
        if t.ops.map.is_none() || t.ops.unmap.is_none() || t.ops.sync.is_none() { return false; }
        TABLE.set(t);
        true
    }
    macro_rules! call {
        ($name:ident ($($arg:expr),*)) => {
            match TABLE.get().and_then(|t| t.ops.$name) { Some(f) => port::from_status(unsafe { f($($arg),*) }), None => Err(Error::Unsupported) }
        };
    }
    impl port::Mappings for Host {
        unsafe fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8> {
            let mut address = ptr::null_mut();
            call!(map(file, offset, length, access, if shared { SHARED } else { PRIVATE }, &mut address))?;
            Ok(address.cast())
        }
        unsafe fn unmap(address: *mut u8, length: usize) -> Result<()> { call!(unmap(address.cast(), length)) }
        unsafe fn sync(address: *mut u8, length: usize) -> Result<()> { call!(sync(address.cast(), length)) }
    }
}

#[cfg(feature = "host-volumes")]
pub mod volumes {
    use super::*;
    use crate::volumes::{Host as Table, Status, CAP};
    extern "C" { fn dotnet_pal_host_volumes_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_volumes_v2(), CAP) }) else { return false; };
        if t.ops.entry.is_none() || t.ops.status.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Volumes for Host {
        unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.entry) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            match port::from_status(unsafe { f(index, out, capacity, &mut needed) }) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        fn status(path: &[u8]) -> Result<Status> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.status) else { return Err(Error::Unsupported); };
            let mut status = Status::EMPTY;
            port::from_status(unsafe { f(path.as_ptr(), path.len(), &mut status, core::mem::size_of::<Status>()) })?;
            Ok(status)
        }
    }
}

#[cfg(feature = "host-network")]
pub mod network {
    use super::*;
    use crate::network::{Host as Table, Interface, InterfaceAddress, CAP};
    use crate::sockets::Address;
    extern "C" { fn dotnet_pal_host_network_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    /// A host answers what it can: enumeration, lookup and membership are separate facilities, so any callback may be NULL.
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_network_v2(), CAP) }) else { return false; };
        TABLE.set(t);
        true
    }
    impl port::Network for Host {
        fn interface_entry(index: usize) -> Result<Interface> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.interface_entry) else { return Err(Error::Unsupported); };
            let mut entry = Interface::EMPTY;
            port::from_status(unsafe { f(index, &mut entry, core::mem::size_of::<Interface>()) })?;
            Ok(entry)
        }
        fn address_entry(index: usize) -> Result<InterfaceAddress> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.address_entry) else { return Err(Error::Unsupported); };
            let mut entry = InterfaceAddress::EMPTY;
            port::from_status(unsafe { f(index, &mut entry, core::mem::size_of::<InterfaceAddress>()) })?;
            Ok(entry)
        }
        unsafe fn reverse_lookup(address: &Address, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.reverse_lookup) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            match port::from_status(unsafe { f(address, out, capacity, &mut needed) }) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        unsafe fn membership(socket: *mut c_void, group: &Address, interface_index: u32, join: bool) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.membership) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(socket, group, interface_index, join as u32) })
        }
    }
}

#[cfg(feature = "host-local-sockets")]
pub mod local_sockets {
    use super::*;
    use crate::local_sockets::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_local_sockets_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_local_sockets_v2(), CAP) }) else { return false; };
        if t.ops.bind.is_none() || t.ops.connect.is_none() || t.ops.address.is_none() || t.ops.peer_user.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::LocalSockets for Host {
        unsafe fn bind(socket: *mut c_void, path: &[u8]) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.bind) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(socket, path.as_ptr(), path.len()) })
        }
        unsafe fn connect(socket: *mut c_void, path: &[u8]) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.connect) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(socket, path.as_ptr(), path.len()) })
        }
        unsafe fn address(socket: *mut c_void, peer: bool, out: *mut u8, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.address) else { return Err(Error::Unsupported); };
            let mut needed = 0;
            match port::from_status(unsafe { f(socket, peer as u32, out, capacity, &mut needed) }) { Ok(()) | Err(Error::BufferTooSmall) => Ok(needed), Err(e) => Err(e) }
        }
        unsafe fn peer_user(socket: *mut c_void) -> Result<u32> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.peer_user) else { return Err(Error::Unsupported); };
            let mut user = u32::MAX;
            port::from_status(unsafe { f(socket, &mut user) })?;
            Ok(user)
        }
    }
}

#[cfg(feature = "host-accounts")]
pub mod accounts {
    use super::*;
    use crate::accounts::{Account, Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_accounts_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    /// Lookups and group lists are separate facilities, so a host may leave any callback NULL.
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_accounts_v2(), CAP) }) else { return false; };
        TABLE.set(t);
        true
    }
    /// A host that calls a list too small for a buffer it fits in has written nothing the caller could use.
    fn listed(status: Result<()>, count: usize, capacity: usize) -> Result<usize> {
        match status { Ok(()) => Ok(count), Err(Error::BufferTooSmall) if count > capacity => Ok(count), Err(Error::BufferTooSmall) => Err(Error::Os), Err(e) => Err(e) }
    }
    impl port::Accounts for Host {
        fn user_by_id(user_id: u32) -> Result<Account> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.user_by_id) else { return Err(Error::Unsupported); };
            let mut account = Account::EMPTY;
            port::from_status(unsafe { f(user_id, &mut account, core::mem::size_of::<Account>()) })?;
            Ok(account)
        }
        fn user_by_name(name: &[u8]) -> Result<Account> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.user_by_name) else { return Err(Error::Unsupported); };
            let mut account = Account::EMPTY;
            port::from_status(unsafe { f(name.as_ptr(), name.len(), &mut account, core::mem::size_of::<Account>()) })?;
            Ok(account)
        }
        unsafe fn process_groups(out: *mut u32, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.process_groups) else { return Err(Error::Unsupported); };
            let mut count = 0;
            listed(port::from_status(unsafe { f(out, capacity, &mut count) }), count, capacity)
        }
        unsafe fn user_groups(name: &[u8], primary_group: u32, out: *mut u32, capacity: usize) -> Result<usize> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.user_groups) else { return Err(Error::Unsupported); };
            let mut count = 0;
            listed(port::from_status(unsafe { f(name.as_ptr(), name.len(), primary_group, out, capacity, &mut count) }), count, capacity)
        }
    }
}

#[cfg(feature = "host-priority")]
pub mod priority {
    use super::*;
    use crate::priority::{Host as Table, CAP};
    extern "C" { fn dotnet_pal_host_priority_v2() -> *const Table; }
    static TABLE: Cached<Table> = Cached::new();
    pub fn validate() -> bool {
        let Some(t) = (unsafe { table(dotnet_pal_host_priority_v2(), CAP) }) else { return false; };
        if t.ops.get.is_none() || t.ops.set.is_none() { return false; }
        TABLE.set(t);
        true
    }
    impl port::Priority for Host {
        fn get(process: u64) -> Result<i32> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.get) else { return Err(Error::Unsupported); };
            let mut value = 0;
            port::from_status(unsafe { f(process, &mut value) })?;
            Ok(value)
        }
        fn set(process: u64, value: i32) -> Result<()> {
            let Some(f) = TABLE.get().and_then(|t| t.ops.set) else { return Err(Error::Unsupported); };
            port::from_status(unsafe { f(process, value) })
        }
    }
}
