//! Topology, process and image providers of the desktop port.
//!
//! Linux reads procfs, sysfs, cgroup v2 limits and ELF program headers. macOS
//! uses sysconf/sysctl and Mach host statistics. Windows reports CPU and memory
//! figures from the system information APIs. Image inspection (unwind tables,
//! readability probes, build ids) exists on Linux only; elsewhere the capability
//! is absent rather than approximated.
use super::Std;
use dotnet_pal_rs::port::{self, Error, Result};



/// The cgroup v2 directory of this process, when the unified hierarchy is mounted at /sys/fs/cgroup.










impl port::Topology for Std {
    fn cpu_max() -> Result<u32> {
        let mut info: windows_sys::Win32::System::SystemInformation::SYSTEM_INFO = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetSystemInfo(&mut info) };
        if info.dwNumberOfProcessors == 0 { Err(Error::Os) } else { Ok(info.dwNumberOfProcessors) }
    }
    fn cpu_count() -> Result<u32> { std::thread::available_parallelism().map(|n| n.get() as u32).map_err(|_| Error::Os) }
    fn current_cpu() -> Result<u32> { Err(Error::Unsupported) }
    unsafe fn process_affinity(mask: *mut u8, capacity: usize) -> Result<usize> {
        let cpus = Self::cpu_max()? as usize;
        let needed = cpus.div_ceil(8);
        for index in 0..needed.min(capacity) {
            let bits = (cpus - index * 8).min(8);
            unsafe { mask.add(index).write(if bits == 8 { 0xff } else { (1u8 << bits) - 1 }) };
        }
        Ok(needed)
    }
    fn set_thread_affinity(_: u32) -> Result<()> { Err(Error::Unsupported) }
    fn physical_memory() -> Result<(u64, u64)> {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 { return Err(Error::Os); }
        Ok((status.ullTotalPhys, status.ullAvailPhys.min(status.ullTotalPhys)))
    }
    fn memory_limit() -> Result<u64> { Ok(0) }
    fn virtual_limit() -> Result<u64> { Ok(0) }
    fn cache_size() -> Result<usize> { Ok(0) }
    fn cache_level_size(_: u32) -> Result<usize> { Ok(0) }
    fn swap_memory() -> Result<(u64, u64)> {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 { return Err(Error::Os); }
        // The page file is what the commit limit has beyond physical memory; the system reports the two together.
        let total = status.ullTotalPageFile.saturating_sub(status.ullTotalPhys);
        Ok((total, status.ullAvailPageFile.saturating_sub(status.ullAvailPhys).min(total)))
    }
    fn cpu_features() -> Result<(u64, u64)> { Ok((0, 0)) }
}

impl port::Process for Std {
    fn exit(code: i32) -> ! { std::process::exit(code) }
    fn debugger_present() -> Result<bool> {

        // macOS would need the kinfo_proc layout, which the libc crate does not expose; report unsupported.

        { Err(Error::Unsupported) }
    }
    /// The desktop port ships no crash-dump utility contract; a runtime that
    /// asks for one learns that honestly instead of getting a silent no-op.
    unsafe fn crash_dump(_: &[*const u8], _: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) }
}


/// The image provider of this host: `Std` on Linux, absent elsewhere.


pub type ImageProvider = port::Absent;

impl port::Streams for Std {
    unsafe fn write(stream: u32, data: *const u8, size: usize) -> Result<usize> {
        use std::io::Write;
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        let result = if stream == 2 { std::io::stderr().lock().write(bytes) } else { std::io::stdout().lock().write(bytes) };
        result.map_err(|_| Error::Os)
    }
    unsafe fn read(_: u32, out: *mut u8, capacity: usize) -> Result<usize> {
        use std::io::Read;
        let bytes = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
        std::io::stdin().lock().read(bytes).map_err(|_| Error::Os)
    }
    fn is_terminal(stream: u32) -> Result<bool> {
        use std::io::IsTerminal;
        Ok(match stream { 0 => std::io::stdin().is_terminal(), 1 => std::io::stdout().is_terminal(), _ => std::io::stderr().is_terminal() })
    }
}
