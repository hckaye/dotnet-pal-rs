#![cfg(target_os = "windows")]
//! windows providers. No other platform implementation is shipped by this crate.
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

/// No signal context `faults.rs` can convert: the capability is absent.

mod faults { pub type Provider = dotnet_pal_rs::port::Absent; }

dotnet_pal_std_common::implement!(Std);




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
