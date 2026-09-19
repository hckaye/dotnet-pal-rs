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





fn page() -> u64 { let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }; if v > 0 { v as u64 } else { 4096 } }

fn physical_total() -> Result<u64> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    if pages > 0 { Ok(pages as u64 * page()) } else { Err(Error::Os) }
}



impl port::Topology for Std {
    fn cpu_max() -> Result<u32> {

        let value = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
        if value > 0 { u32::try_from(value).map_err(|_| Error::Os) } else { Err(Error::Os) }
    }
    fn cpu_count() -> Result<u32> {


        { std::thread::available_parallelism().map(|n| n.get() as u32).map_err(|_| Error::Os) }
    }
    fn current_cpu() -> Result<u32> {


        { Err(Error::Unsupported) }
    }
    unsafe fn process_affinity(mask: *mut u8, capacity: usize) -> Result<usize> {
        let needed = (Self::cpu_max()? as usize).div_ceil(8);


        let words = { let mut words = [0u64; 128]; for cpu in 0..Self::cpu_max()? as usize { words[cpu / 64] |= 1 << (cpu % 64); } words };
        let bytes = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), std::mem::size_of_val(&words)) };
        let copy = needed.min(capacity).min(bytes.len());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), mask, copy) };
        Ok(needed)
    }
    fn set_thread_affinity(cpu: u32) -> Result<()> {


        { let _ = cpu; Err(Error::Unsupported) }
    }
    fn physical_memory() -> Result<(u64, u64)> {
        let machine = physical_total()?;


        #[allow(deprecated)] // mach_host_self is the documented way to reach host_statistics64 without another crate
        {
            let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::uninit();
            let mut count = (std::mem::size_of::<libc::vm_statistics64>() / std::mem::size_of::<libc::integer_t>()) as libc::mach_msg_type_number_t;
            let rc = unsafe { libc::host_statistics64(libc::mach_host_self(), libc::HOST_VM_INFO64, stats.as_mut_ptr().cast(), &mut count) };
            if rc != libc::KERN_SUCCESS { return Err(Error::Os); }
            let stats = unsafe { stats.assume_init() };
            let available = (stats.free_count as u64 + stats.inactive_count as u64) * page();
            Ok((machine, available.min(machine)))
        }

    }
    fn memory_limit() -> Result<u64> {


        { Ok(0) }
    }
    fn virtual_limit() -> Result<u64> {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_AS, limit.as_mut_ptr()) } != 0 { return Err(Error::Os); }
        let limit = unsafe { limit.assume_init() };
        Ok(if limit.rlim_cur == libc::RLIM_INFINITY { 0 } else { limit.rlim_cur })
    }
    fn cache_size() -> Result<usize> {


        {
            let mut best = 0u64;
            for name in [c"hw.l3cachesize", c"hw.l2cachesize", c"hw.l1dcachesize"] {
                let mut value: u64 = 0;
                let mut size = std::mem::size_of::<u64>();
                if unsafe { libc::sysctlbyname(name.as_ptr(), (&mut value as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) } == 0 { best = best.max(value); }
            }
            usize::try_from(best).map_err(|_| Error::Os)
        }

    }
    fn cache_level_size(level: u32) -> Result<usize> {


        {
            // The system reports three levels; a fourth is not among its keys.
            let name = match level {
                1 => c"hw.l1dcachesize", 2 => c"hw.l2cachesize", 3 => c"hw.l3cachesize", 4 => return Ok(0), _ => return Err(Error::InvalidArgument),
            };
            let (mut value, mut size) = (0u64, std::mem::size_of::<u64>());
            if unsafe { libc::sysctlbyname(name.as_ptr(), (&mut value as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) } != 0 { return Ok(0); }
            usize::try_from(value).map_err(|_| Error::Os)
        }

    }
    fn swap_memory() -> Result<(u64, u64)> {


        {
            let (mut usage, mut size) = (unsafe { std::mem::zeroed::<libc::xsw_usage>() }, std::mem::size_of::<libc::xsw_usage>());
            let asked = unsafe { libc::sysctlbyname(c"vm.swapusage".as_ptr(), (&mut usage as *mut libc::xsw_usage).cast(), &mut size, std::ptr::null_mut(), 0) };
            if asked != 0 { return Err(Error::Os); }
            Ok((usage.xsu_total, usage.xsu_avail.min(usage.xsu_total)))
        }

    }
    fn cpu_features() -> Result<(u64, u64)> {


        { Ok((0, 0)) }
    }
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
