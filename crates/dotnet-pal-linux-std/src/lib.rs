#![cfg(target_os = "linux")]
//! linux providers. No other platform implementation is shipped by this crate.
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
mod local_sockets;
mod accounts;
mod priority;
mod packets;
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod faults;
/// No signal context `faults.rs` can convert: the capability is absent.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod faults { pub type Provider = dotnet_pal_rs::port::Absent; }

dotnet_pal_std_common::implement!(Std);


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

            { let id = unsafe { libc::syscall(libc::SYS_gettid) }; if id > 0 { Ok(id as u64) } else { Err(Error::Os) } }


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

            {
                let mut attr = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
                if unsafe { libc::pthread_getattr_np(libc::pthread_self(), attr.as_mut_ptr()) } != 0 { return Err(Error::Os); }
                let (mut base, mut size) = (ptr::null_mut(), 0);
                let rc = unsafe { libc::pthread_attr_getstack(attr.as_ptr(), &mut base, &mut size) };
                unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()) };
                if rc != 0 || base.is_null() || size == 0 { return Err(Error::Os); }
                Ok((base, (base as usize + size) as *mut c_void))
            }


        }
    }
    impl port::ProcessBarrier for Std {
        fn available() -> bool {

            { unsafe { libc::syscall(libc::SYS_membarrier, 16i32, 0i32) == 0 } }

        }
        fn barrier() -> Result<()> {

            { if unsafe { libc::syscall(libc::SYS_membarrier, 8i32, 0i32) } == 0 { Ok(()) } else { Err(Error::Os) } }

        }
    }
    impl port::ThreadName for Std {
        fn set(name: &[u8]) -> Result<()> {
            let limit = if cfg!(target_os = "linux") { 15 } else { 63 };
            if name.len() > limit { return Err(Error::InvalidArgument); }
            let name = std::ffi::CString::new(name).map_err(|_| Error::InvalidArgument)?;

            let rc = unsafe { libc::pthread_setname_np(libc::pthread_self(), name.as_ptr()) };


            if rc == 0 { Ok(()) } else { Err(Error::Os) }
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
    Watches = Std, Mappings = Std, Volumes = Std, Network = Std, LocalSockets = Std,
    Accounts = Std, Priority = Std, Packets = Std, SpawnAs = Std
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
