//! Topology, process and image providers of the desktop port.
//!
//! Linux reads procfs, sysfs, cgroup v2 limits and ELF program headers. macOS
//! uses sysconf/sysctl and Mach host statistics. Windows reports CPU and memory
//! figures from the system information APIs. Image inspection (unwind tables,
//! readability probes, build ids) exists on Linux only; elsewhere the capability
//! is absent rather than approximated.
use super::Std;
use dotnet_pal_rs::port::{self, Error, Result};

#[cfg(target_os = "linux")]
fn read(path: &str) -> Option<String> { std::fs::read_to_string(path).ok() }
#[cfg(target_os = "linux")]
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let digits = text.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 { return None; }
    let value: u64 = text[..digits].parse().ok()?;
    let multiplier = match text[digits..].trim_start().chars().next() { Some('k' | 'K') => 1024, Some('m' | 'M') => 1 << 20, Some('g' | 'G') => 1 << 30, _ => 1 };
    value.checked_mul(multiplier)
}
/// The cgroup v2 directory of this process, when the unified hierarchy is mounted at /sys/fs/cgroup.
#[cfg(target_os = "linux")]
fn cgroup_directory() -> Option<String> {
    let text = read("/proc/self/cgroup")?;
    let path = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    let directory = format!("/sys/fs/cgroup{}", path.trim());
    std::fs::metadata(&directory).ok().map(|_| directory)
}
#[cfg(target_os = "linux")]
fn cgroup_memory_limit() -> Option<u64> {
    let mut directory = cgroup_directory()?;
    let mut best: Option<u64> = None;
    loop {
        if let Some(limit) = read(&format!("{directory}/memory.max")).and_then(|t| parse_size(&t)) { best = Some(best.map_or(limit, |b| b.min(limit))); }
        if directory == "/sys/fs/cgroup" { break; }
        match directory.rfind('/') { Some(i) if i >= "/sys/fs/cgroup".len() => directory.truncate(i), _ => break }
    }
    best.filter(|l| *l < 0x7FFF_FFFF_0000_0000)
}
#[cfg(target_os = "linux")]
fn cgroup_memory_usage() -> Option<u64> {
    let directory = cgroup_directory()?;
    let usage = parse_size(&read(&format!("{directory}/memory.current"))?)?;
    let inactive = read(&format!("{directory}/memory.stat"))?.lines().find_map(|l| l.strip_prefix("inactive_file ")).and_then(|v| v.trim().parse::<u64>().ok())?;
    Some(usage.saturating_sub(inactive))
}
#[cfg(target_os = "linux")]
fn cgroup_cpu_limit() -> Option<u32> {
    let text = read(&format!("{}/cpu.max", cgroup_directory()?))?;
    let mut parts = text.split_whitespace();
    let (quota, period) = (parts.next()?, parts.next()?);
    if quota == "max" { return None; }
    let (quota, period): (u64, u64) = (quota.parse().ok()?, period.parse().ok()?);
    if quota == 0 || period == 0 { return None; }
    Some(if quota <= period { 1 } else { quota.div_ceil(period).min(u32::MAX as u64) as u32 })
}
#[cfg(unix)]
fn page() -> u64 { let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }; if v > 0 { v as u64 } else { 4096 } }
#[cfg(unix)]
fn physical_total() -> Result<u64> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    if pages > 0 { Ok(pages as u64 * page()) } else { Err(Error::Os) }
}
#[cfg(target_os = "linux")]
fn affinity() -> Result<[u64; 128]> {
    let mut mask = [0u64; 128];
    if unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&mask), mask.as_mut_ptr().cast()) } != 0 { return Err(Error::Os); }
    Ok(mask)
}

#[cfg(unix)]
impl port::Topology for Std {
    fn cpu_max() -> Result<u32> {
        #[cfg(target_os = "linux")]
        if let Some(text) = read("/sys/devices/system/cpu/possible") {
            let highest = text.trim().split(',').filter_map(|range| range.rsplit('-').next()?.parse::<u64>().ok()).max();
            if let Some(highest) = highest { return u32::try_from(highest + 1).map_err(|_| Error::Os); }
        }
        let value = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
        if value > 0 { u32::try_from(value).map_err(|_| Error::Os) } else { Err(Error::Os) }
    }
    fn cpu_count() -> Result<u32> {
        #[cfg(target_os = "linux")]
        {
            let mut count = affinity().map(|m| m.iter().map(|w| w.count_ones()).sum::<u32>()).unwrap_or(0);
            if count == 0 { count = std::thread::available_parallelism().map(|n| n.get() as u32).map_err(|_| Error::Os)?; }
            if let Some(limit) = cgroup_cpu_limit() { count = count.min(limit); }
            Ok(count.max(1))
        }
        #[cfg(not(target_os = "linux"))]
        { std::thread::available_parallelism().map(|n| n.get() as u32).map_err(|_| Error::Os) }
    }
    fn current_cpu() -> Result<u32> {
        #[cfg(target_os = "linux")]
        { let v = unsafe { libc::sched_getcpu() }; if v < 0 { Err(Error::Os) } else { Ok(v as u32) } }
        #[cfg(not(target_os = "linux"))]
        { Err(Error::Unsupported) }
    }
    unsafe fn process_affinity(mask: *mut u8, capacity: usize) -> Result<usize> {
        let needed = (Self::cpu_max()? as usize).div_ceil(8);
        #[cfg(target_os = "linux")]
        let words = affinity()?;
        #[cfg(not(target_os = "linux"))]
        let words = { let mut words = [0u64; 128]; for cpu in 0..Self::cpu_max()? as usize { words[cpu / 64] |= 1 << (cpu % 64); } words };
        let bytes = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), std::mem::size_of_val(&words)) };
        let copy = needed.min(capacity).min(bytes.len());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), mask, copy) };
        Ok(needed)
    }
    fn set_thread_affinity(cpu: u32) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mut mask = [0u64; 128];
            if cpu as usize >= mask.len() * 64 { return Err(Error::InvalidArgument); }
            mask[cpu as usize / 64] = 1 << (cpu % 64);
            if unsafe { libc::sched_setaffinity(0, std::mem::size_of_val(&mask), mask.as_ptr().cast()) } == 0 { Ok(()) } else { Err(Error::Os) }
        }
        #[cfg(not(target_os = "linux"))]
        { let _ = cpu; Err(Error::Unsupported) }
    }
    fn physical_memory() -> Result<(u64, u64)> {
        let machine = physical_total()?;
        #[cfg(target_os = "linux")]
        {
            if let Some(limit) = Self::memory_limit().ok().filter(|l| *l != 0) {
                return Ok((limit, limit.saturating_sub(cgroup_memory_usage().unwrap_or(0))));
            }
            let available = read("/proc/meminfo").and_then(|t| t.lines().find_map(|l| l.strip_prefix("MemAvailable:")).and_then(parse_size)).unwrap_or(0);
            Ok((machine, available.min(machine)))
        }
        #[cfg(target_os = "macos")]
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
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        { Ok((machine, 0)) }
    }
    fn memory_limit() -> Result<u64> {
        #[cfg(target_os = "linux")]
        {
            let Some(limit) = cgroup_memory_limit() else { return Ok(0); };
            let mut result = limit.min(physical_total()?);
            if let Ok(rlimit) = Self::virtual_limit() { if rlimit != 0 { result = result.min(rlimit); } }
            Ok(result)
        }
        #[cfg(not(target_os = "linux"))]
        { Ok(0) }
    }
    fn virtual_limit() -> Result<u64> {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_AS, limit.as_mut_ptr()) } != 0 { return Err(Error::Os); }
        let limit = unsafe { limit.assume_init() };
        Ok(if limit.rlim_cur == libc::RLIM_INFINITY { 0 } else { limit.rlim_cur })
    }
    fn cache_size() -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            let mut best = 0u64;
            for name in [libc::_SC_LEVEL1_DCACHE_SIZE, libc::_SC_LEVEL2_CACHE_SIZE, libc::_SC_LEVEL3_CACHE_SIZE, libc::_SC_LEVEL4_CACHE_SIZE] {
                let value = unsafe { libc::sysconf(name) };
                if value > 0 { best = best.max(value as u64); }
            }
            if best == 0 {
                for index in 0..4 {
                    if let Some(size) = read(&format!("/sys/devices/system/cpu/cpu0/cache/index{index}/size")).and_then(|t| parse_size(&t)) { best = best.max(size); }
                }
            }
            usize::try_from(best).map_err(|_| Error::Os)
        }
        #[cfg(target_os = "macos")]
        {
            let mut best = 0u64;
            for name in [c"hw.l3cachesize", c"hw.l2cachesize", c"hw.l1dcachesize"] {
                let mut value: u64 = 0;
                let mut size = std::mem::size_of::<u64>();
                if unsafe { libc::sysctlbyname(name.as_ptr(), (&mut value as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) } == 0 { best = best.max(value); }
            }
            usize::try_from(best).map_err(|_| Error::Os)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        { Ok(0) }
    }
    fn cache_level_size(level: u32) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            let name = match level {
                1 => libc::_SC_LEVEL1_DCACHE_SIZE, 2 => libc::_SC_LEVEL2_CACHE_SIZE, 3 => libc::_SC_LEVEL3_CACHE_SIZE, 4 => libc::_SC_LEVEL4_CACHE_SIZE,
                _ => return Err(Error::InvalidArgument),
            };
            let value = unsafe { libc::sysconf(name) };
            if value > 0 { return usize::try_from(value as u64).map_err(|_| Error::Os); }
            // The C library answers these from the CPU on x86 and from nothing at all on AArch64; sysfs has them there.
            let mut best = 0u64;
            for index in 0..8 {
                let directory = format!("/sys/devices/system/cpu/cpu0/cache/index{index}");
                if read(&format!("{directory}/level")).and_then(|t| t.trim().parse::<u32>().ok()) != Some(level) { continue; }
                // An instruction cache is not what a consumer sizing its data structures asks for.
                if !read(&format!("{directory}/type")).is_some_and(|t| t.starts_with("Data") || t.starts_with("Unified")) { continue; }
                if let Some(size) = read(&format!("{directory}/size")).and_then(|t| parse_size(&t)) { best = best.max(size); }
            }
            usize::try_from(best).map_err(|_| Error::Os)
        }
        #[cfg(target_os = "macos")]
        {
            // The system reports three levels; a fourth is not among its keys.
            let name = match level {
                1 => c"hw.l1dcachesize", 2 => c"hw.l2cachesize", 3 => c"hw.l3cachesize", 4 => return Ok(0), _ => return Err(Error::InvalidArgument),
            };
            let (mut value, mut size) = (0u64, std::mem::size_of::<u64>());
            if unsafe { libc::sysctlbyname(name.as_ptr(), (&mut value as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) } != 0 { return Ok(0); }
            usize::try_from(value).map_err(|_| Error::Os)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        { let _ = level; Err(Error::Unsupported) }
    }
    fn swap_memory() -> Result<(u64, u64)> {
        #[cfg(target_os = "linux")]
        {
            let mut info: libc::sysinfo = unsafe { std::mem::zeroed() };
            if unsafe { libc::sysinfo(&mut info) } != 0 { return Err(Error::Os); }
            // Older kernels report every figure in bytes, which they mark with a unit of zero.
            let unit = if info.mem_unit == 0 { 1u64 } else { info.mem_unit as u64 };
            let (mut total, mut available) = (info.totalswap as u64 * unit, info.freeswap as u64 * unit);
            // A container's own limit stands in for the machine's, as it does for physical memory. Only the unified
            // hierarchy separates swap from memory, and only the process's own group is read here.
            if let Some(directory) = cgroup_directory() {
                if let Some(limit) = read(&format!("{directory}/memory.swap.max")).and_then(|t| parse_size(&t)) {
                    let used = read(&format!("{directory}/memory.swap.current")).and_then(|t| parse_size(&t)).unwrap_or(0);
                    total = total.min(limit);
                    available = available.min(limit.saturating_sub(used));
                }
            }
            Ok((total, available.min(total)))
        }
        #[cfg(target_os = "macos")]
        {
            let (mut usage, mut size) = (unsafe { std::mem::zeroed::<libc::xsw_usage>() }, std::mem::size_of::<libc::xsw_usage>());
            let asked = unsafe { libc::sysctlbyname(c"vm.swapusage".as_ptr(), (&mut usage as *mut libc::xsw_usage).cast(), &mut size, std::ptr::null_mut(), 0) };
            if asked != 0 { return Err(Error::Os); }
            Ok((usage.xsu_total, usage.xsu_avail.min(usage.xsu_total)))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        { Err(Error::Unsupported) }
    }
    fn cpu_features() -> Result<(u64, u64)> {
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        { Ok((unsafe { libc::getauxval(libc::AT_HWCAP) } as u64, unsafe { libc::getauxval(libc::AT_HWCAP2) } as u64)) }
        #[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
        { Ok((0, 0)) }
    }
}
#[cfg(windows)]
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
        #[cfg(target_os = "linux")]
        {
            let text = read("/proc/self/status").ok_or(Error::Os)?;
            let value = text.lines().find_map(|l| l.strip_prefix("TracerPid:")).ok_or(Error::Os)?;
            Ok(value.trim().parse::<u64>().map_err(|_| Error::Os)? != 0)
        }
        // macOS would need the kinfo_proc layout, which the libc crate does not expose; report unsupported.
        #[cfg(not(target_os = "linux"))]
        { Err(Error::Unsupported) }
    }
    /// The desktop port ships no crash-dump utility contract; a runtime that
    /// asks for one learns that honestly instead of getting a silent no-op.
    unsafe fn crash_dump(_: &[*const u8], _: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) }
}

#[cfg(target_os = "linux")]
mod elf {
    use super::*;
    use dotnet_pal_rs::image::UnwindInfo;
    use std::ffi::c_void;
    const PT_LOAD: u32 = 1;
    const PT_NOTE: u32 = 4;
    const PT_GNU_EH_FRAME: u32 = 0x6474e550;
    #[repr(C)]
    struct Nhdr { namesz: u32, descsz: u32, kind: u32 }
    struct Search { address: usize, result: Option<UnwindInfo>, build_id: Option<Vec<u8>> }
    extern "C" fn callback(info: *mut libc::dl_phdr_info, _: usize, data: *mut c_void) -> i32 {
        let (info, search) = unsafe { (&*info, &mut *data.cast::<Search>()) };
        let base = info.dlpi_addr as usize;
        let headers = unsafe { std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize) };
        if search.build_id.is_none() && search.result.is_none() && search.address != 0 {
            // unwind lookup: the PT_LOAD containing the address plus PT_GNU_EH_FRAME
            let Some(text) = headers.iter().find(|h| h.p_type == PT_LOAD && search.address >= base + h.p_vaddr as usize && search.address < base + h.p_vaddr as usize + h.p_memsz as usize) else { return 0; };
            let Some(hdr) = headers.iter().rev().find(|h| h.p_type == PT_GNU_EH_FRAME) else { return 0; };
            search.result = Some(UnwindInfo { base, text_start: base + text.p_vaddr as usize, text_length: text.p_memsz as usize,
                eh_frame_hdr: base + hdr.p_vaddr as usize, eh_frame_hdr_length: hdr.p_memsz as usize, eh_frame: 0, eh_frame_length: 0 });
            return 1;
        }
        0
    }
    extern "C" fn note_callback(info: *mut libc::dl_phdr_info, _: usize, data: *mut c_void) -> i32 {
        let (info, search) = unsafe { (&*info, &mut *data.cast::<Search>()) };
        let bias = info.dlpi_addr as usize;
        let headers = unsafe { std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize) };
        if headers.iter().find(|h| h.p_type == PT_LOAD).map(|h| bias + h.p_vaddr as usize) != Some(search.address) { return 0; }
        for header in headers.iter().filter(|h| h.p_type == PT_NOTE) {
            let (start, size, align) = (bias + header.p_vaddr as usize, header.p_memsz as usize, (header.p_align as usize).max(4));
            let mut offset = 0;
            while offset + std::mem::size_of::<Nhdr>() <= size {
                let note = unsafe { &*((start + offset) as *const Nhdr) };
                let name = start + offset + std::mem::size_of::<Nhdr>();
                let desc = name + (note.namesz as usize).next_multiple_of(align);
                if note.namesz == 4 && note.kind == 3 && unsafe { std::slice::from_raw_parts(name as *const u8, 4) } == b"GNU\0" {
                    search.build_id = Some(unsafe { std::slice::from_raw_parts(desc as *const u8, note.descsz as usize) }.to_vec());
                    return 1;
                }
                offset += std::mem::size_of::<Nhdr>() + (note.namesz as usize).next_multiple_of(align) + (note.descsz as usize).next_multiple_of(align);
            }
        }
        0
    }
    impl port::Image for Std {
        unsafe fn unwind_info(address: usize) -> Result<UnwindInfo> {
            let mut search = Search { address, result: None, build_id: None };
            unsafe { libc::dl_iterate_phdr(Some(callback), (&mut search as *mut Search).cast()) };
            search.result.ok_or(Error::NotFound)
        }
        unsafe fn readable(address: usize, size: usize) -> Result<bool> {
            let page = page() as usize;
            let last = address.checked_add(size - 1).ok_or(Error::InvalidArgument)?;
            let mut probe = address & !(page - 1);
            loop {
                // rt_sigprocmask reads the new set before validating `how`: EFAULT means unreadable.
                let rc = unsafe { libc::syscall(libc::SYS_rt_sigprocmask, -1i32, probe as *const c_void, std::ptr::null::<c_void>(), 8usize) };
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                match (rc, errno) { (_, libc::EFAULT) => return Ok(false), (-1, libc::EINVAL) => {}, _ => return Err(Error::Os) }
                if probe + page > last { return Ok(true); }
                probe += page;
            }
        }
        unsafe fn build_id(base: usize, out: *mut u8, capacity: usize) -> Result<usize> {
            let mut search = Search { address: base, result: None, build_id: None };
            unsafe { libc::dl_iterate_phdr(Some(note_callback), (&mut search as *mut Search).cast()) };
            let id = search.build_id.filter(|id| !id.is_empty()).ok_or(Error::NotFound)?;
            unsafe { std::ptr::copy_nonoverlapping(id.as_ptr(), out, id.len().min(capacity)) };
            Ok(id.len())
        }
    }
}
/// The image provider of this host: `Std` on Linux, absent elsewhere.
#[cfg(target_os = "linux")]
pub type ImageProvider = Std;
#[cfg(not(target_os = "linux"))]
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
