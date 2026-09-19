//! Linux providers for the topology, process and image groups: the parts of
//! the reference backend that read procfs/sysfs, cgroups, resource limits and
//! ELF program headers. No Rust heap; bounded stack buffers only.
use crate::image::UnwindInfo;
use crate::linux::Linux;
use crate::port::{self, Error, Result};
use core::{ffi::c_void, mem, ptr, sync::atomic::{AtomicU8, AtomicUsize, Ordering}};

fn errno() -> i32 { unsafe { *libc::__errno_location() } }
fn page() -> u64 { let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }; if v > 0 { v as u64 } else { 4096 } }

/// Reads at most `buffer.len()` bytes of a file into the buffer.
fn read_file(path: &[u8], buffer: &mut [u8]) -> Option<usize> {
    debug_assert!(path.last() == Some(&0));
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 { return None; }
    let mut filled = 0;
    while filled < buffer.len() {
        let n = unsafe { libc::read(fd, buffer.as_mut_ptr().add(filled).cast(), buffer.len() - filled) };
        if n < 0 { if errno() == libc::EINTR { continue; } unsafe { libc::close(fd) }; return None; }
        if n == 0 { break; }
        filled += n as usize;
    }
    unsafe { libc::close(fd) };
    Some(filled)
}
/// Streams the lines of a file through a fixed buffer; a line longer than the
/// buffer is skipped rather than truncated into a wrong parse.
struct Lines { fd: i32, buffer: [u8; 2048], filled: usize, start: usize, eof: bool, skipping: bool }
impl Lines {
    fn open(path: &[u8]) -> Option<Self> {
        let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 { None } else { Some(Self { fd, buffer: [0; 2048], filled: 0, start: 0, eof: false, skipping: false }) }
    }
    fn fill(&mut self) -> bool {
        if self.start > 0 { self.buffer.copy_within(self.start..self.filled, 0); self.filled -= self.start; self.start = 0; }
        if self.filled == self.buffer.len() { return false; }
        loop {
            let n = unsafe { libc::read(self.fd, self.buffer.as_mut_ptr().add(self.filled).cast(), self.buffer.len() - self.filled) };
            if n < 0 && errno() == libc::EINTR { continue; }
            if n <= 0 { self.eof = true; return false; }
            self.filled += n as usize;
            return true;
        }
    }
    /// The next complete line without its terminator, copied into `out`.
    fn next<'a>(&mut self, out: &'a mut [u8; 2048]) -> Option<&'a [u8]> {
        loop {
            if let Some(offset) = self.buffer[self.start..self.filled].iter().position(|&b| b == b'\n') {
                let line = &self.buffer[self.start..self.start + offset];
                self.start += offset + 1;
                if self.skipping { self.skipping = false; continue; }
                out[..line.len()].copy_from_slice(line);
                return Some(&out[..line.len()]);
            }
            if self.eof {
                if self.start == self.filled || self.skipping { return None; }
                let line = &self.buffer[self.start..self.filled];
                let length = line.len();
                out[..length].copy_from_slice(line);
                self.start = self.filled;
                return Some(&out[..length]);
            }
            if !self.fill() {
                if self.eof { continue; }
                // The buffer is full without a terminator: discard this oversized line.
                self.start = 0; self.filled = 0; self.skipping = true;
                if !self.fill() { return None; }
            }
        }
    }
}
impl Drop for Lines { fn drop(&mut self) { unsafe { libc::close(self.fd) }; } }

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |i| i + 1);
    &bytes[start..end.max(start)]
}
/// Parses a decimal prefix; returns the value and the unparsed remainder.
fn parse_u64(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let bytes = trim(bytes);
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 { return None; }
    let mut value: u64 = 0;
    for &b in &bytes[..digits] { value = value.checked_mul(10)?.checked_add((b - b'0') as u64)?; }
    Some((value, &bytes[digits..]))
}
/// Parses a size with an optional k/m/g suffix, as the cgroup files use.
fn parse_size(bytes: &[u8]) -> Option<u64> {
    let (value, rest) = parse_u64(bytes)?;
    let multiplier = match rest.first() { Some(b'k' | b'K') => 1024u64, Some(b'm' | b'M') => 1 << 20, Some(b'g' | b'G') => 1 << 30, _ => 1 };
    value.checked_mul(multiplier)
}
fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> { line.split(|b| *b == b' ').filter(|f| !f.is_empty()) }
fn starts_with(bytes: &[u8], prefix: &[u8]) -> bool { bytes.len() >= prefix.len() && &bytes[..prefix.len()] == prefix }

/// Fixed-capacity path buffer, always NUL-terminated.
struct Path { bytes: [u8; 512], length: usize }
impl Path {
    const fn new() -> Self { Self { bytes: [0; 512], length: 0 } }
    fn set(&mut self, value: &[u8]) -> bool {
        if value.len() >= self.bytes.len() { return false; }
        self.bytes[..value.len()].copy_from_slice(value); self.bytes[value.len()] = 0; self.length = value.len(); true
    }
    fn push(&mut self, value: &[u8]) -> bool {
        if self.length + value.len() >= self.bytes.len() { return false; }
        self.bytes[self.length..self.length + value.len()].copy_from_slice(value); self.length += value.len(); self.bytes[self.length] = 0; true
    }
    fn as_slice(&self) -> &[u8] { &self.bytes[..self.length] }
    fn c_str(&self) -> &[u8] { &self.bytes[..=self.length] }
}

/// The memory and cpu cgroup directories of this process, discovered once.
struct CGroups { version: u8, memory: Path, memory_mount: Path, cpu: Path }
struct CGroupCell(core::cell::UnsafeCell<CGroups>, AtomicU8);
unsafe impl Sync for CGroupCell {}
static CGROUPS: CGroupCell = CGroupCell(core::cell::UnsafeCell::new(CGroups { version: 0, memory: Path::new(), memory_mount: Path::new(), cpu: Path::new() }), AtomicU8::new(0));
fn cgroups() -> &'static CGroups {
    loop {
        match CGROUPS.1.load(Ordering::Acquire) {
            2 => return unsafe { &*CGROUPS.0.get() },
            1 => core::hint::spin_loop(),
            _ => {
                if CGROUPS.1.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() { continue; }
                unsafe { discover_cgroups(&mut *CGROUPS.0.get()) };
                CGROUPS.1.store(2, Ordering::Release);
            }
        }
    }
}
fn cgroup_version() -> u8 {
    let mut stats = mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c"/sys/fs/cgroup".as_ptr(), stats.as_mut_ptr()) } != 0 { return 0; }
    if unsafe { stats.assume_init() }.f_type as u64 == 0x63677270 { 2 } else { 1 }
}
/// Finds the mount point and root of the cgroup hierarchy carrying `controller`
/// (v1) or the unified hierarchy (v2) in /proc/self/mountinfo.
fn hierarchy_mount(version: u8, controller: &[u8], mount: &mut Path, root: &mut Path) -> bool {
    let Some(mut lines) = Lines::open(b"/proc/self/mountinfo\0") else { return false; };
    let mut line = [0u8; 2048];
    while let Some(text) = lines.next(&mut line) {
        let Some(separator) = text.windows(3).position(|w| w == b" - ") else { continue; };
        let mut tail = fields(&text[separator + 3..]);
        let (Some(fs_type), Some(_source), Some(options)) = (tail.next(), tail.next(), tail.next()) else { continue; };
        let matches = match version {
            2 => fs_type == b"cgroup2",
            _ => fs_type == b"cgroup" && options.split(|b| *b == b',').any(|o| o == controller),
        };
        if !matches { continue; }
        let mut head = fields(&text[..separator]);
        let (Some(mount_root), Some(mount_point)) = (head.nth(3), head.next()) else { continue; };
        if root.set(mount_root) && mount.set(mount_point) { return true; }
    }
    false
}
/// The cgroup path of this process relative to the hierarchy root, from /proc/self/cgroup.
fn cgroup_relative(version: u8, controller: &[u8], out: &mut Path) -> bool {
    let Some(mut lines) = Lines::open(b"/proc/self/cgroup\0") else { return false; };
    let mut line = [0u8; 2048];
    while let Some(text) = lines.next(&mut line) {
        let mut parts = text.splitn(3, |b| *b == b':');
        let (Some(id), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue; };
        let matches = match version { 2 => id == b"0" && controllers.is_empty(), _ => controllers.split(|b| *b == b',').any(|c| c == controller) };
        if matches { return out.set(path); }
    }
    false
}
fn cgroup_directory(version: u8, controller: &[u8], out: &mut Path, mount_out: Option<&mut Path>) -> bool {
    let (mut mount, mut root, mut relative) = (Path::new(), Path::new(), Path::new());
    if !hierarchy_mount(version, controller, &mut mount, &mut root) || !cgroup_relative(version, controller, &mut relative) { return false; }
    let root_len = if root.length == 1 || !starts_with(relative.as_slice(), root.as_slice()) { 0 } else { root.length };
    if !out.set(mount.as_slice()) || !out.push(&relative.as_slice()[root_len..]) { return false; }
    if let Some(m) = mount_out { m.set(mount.as_slice()); }
    true
}
fn discover_cgroups(groups: &mut CGroups) {
    groups.version = cgroup_version();
    if groups.version == 0 { return; }
    let mut memory = Path::new(); let mut mount = Path::new(); let mut cpu = Path::new();
    if cgroup_directory(groups.version, b"memory", &mut memory, Some(&mut mount)) { groups.memory = memory; groups.memory_mount = mount; }
    if cgroup_directory(groups.version, b"cpu", &mut cpu, None) { groups.cpu = cpu; }
}
fn file_size_value(directory: &[u8], name: &[u8]) -> Option<u64> {
    let mut path = Path::new();
    if !path.set(directory) || !path.push(name) { return None; }
    let mut buffer = [0u8; 128];
    let length = read_file(path.c_str(), &mut buffer)?;
    parse_size(&buffer[..length])
}
fn stat_field(directory: &[u8], field: &[u8]) -> Option<u64> {
    let mut path = Path::new();
    if !path.set(directory) || !path.push(b"/memory.stat") { return None; }
    let mut lines = Lines::open(path.c_str())?;
    let mut line = [0u8; 2048];
    while let Some(text) = lines.next(&mut line) {
        if starts_with(text, field) { return parse_u64(&text[field.len()..]).map(|v| v.0); }
    }
    None
}
/// The cgroup memory limit in bytes, `None` when no limit applies.
fn cgroup_memory_limit() -> Option<u64> {
    let groups = cgroups();
    if groups.memory.length == 0 { return None; }
    match groups.version {
        1 => {
            if file_size_value(groups.memory.as_slice(), b"/memory.use_hierarchy").unwrap_or(0) != 0 {
                return stat_field(groups.memory.as_slice(), b"hierarchical_memory_limit ");
            }
            file_size_value(groups.memory.as_slice(), b"/memory.limit_in_bytes")
        }
        2 => {
            // Walk from the process cgroup up to the hierarchy mount; the tightest limit wins.
            let mut directory = Path::new();
            directory.set(groups.memory.as_slice());
            let mut best: Option<u64> = None;
            loop {
                if let Some(limit) = file_size_value(directory.as_slice(), b"/memory.max") { best = Some(best.map_or(limit, |b| b.min(limit))); }
                if directory.length <= groups.memory_mount.length { break; }
                let Some(slash) = directory.as_slice().iter().rposition(|b| *b == b'/') else { break; };
                if slash < groups.memory_mount.length { break; }
                directory.length = slash; directory.bytes[slash] = 0;
            }
            best
        }
        _ => None,
    }
}
/// The tightest swap limit and the swap charged, walking from the process cgroup up to the
/// hierarchy mount as the memory limit does. Only the unified hierarchy separates swap from
/// memory; version 1 counts the two together, so nothing is reported for it.
fn cgroup_swap() -> Option<(u64, u64)> {
    let groups = cgroups();
    if groups.version != 2 || groups.memory.length == 0 { return None; }
    let mut directory = Path::new();
    directory.set(groups.memory.as_slice());
    let (mut limit, mut used) = (None::<u64>, None::<u64>);
    loop {
        if let Some(value) = file_size_value(directory.as_slice(), b"/memory.swap.max") { limit = Some(limit.map_or(value, |b: u64| b.min(value))); }
        if used.is_none() { used = file_size_value(directory.as_slice(), b"/memory.swap.current"); }
        if directory.length <= groups.memory_mount.length { break; }
        let Some(slash) = directory.as_slice().iter().rposition(|b| *b == b'/') else { break; };
        if slash < groups.memory_mount.length { break; }
        directory.length = slash; directory.bytes[slash] = 0;
    }
    limit.map(|limit| (limit, used.unwrap_or(0)))
}
/// One value of one cache of CPU 0, as sysfs spells it out.
fn cache_attribute(index: u8, name: &[u8], buffer: &mut [u8]) -> Option<usize> {
    let mut path = Path::new();
    let mut directory = *b"/sys/devices/system/cpu/cpu0/cache/index0";
    *directory.last_mut()? = b'0' + index;
    if !path.set(&directory) || !path.push(name) { return None; }
    read_file(path.c_str(), buffer)
}
/// Bytes charged to the cgroup, excluding inactive file cache, like the GC's reading.
fn cgroup_memory_usage() -> Option<u64> {
    let groups = cgroups();
    if groups.memory.length == 0 { return None; }
    let (usage_file, inactive): (&[u8], &[u8]) = match groups.version {
        1 => (b"/memory.usage_in_bytes", b"total_inactive_file "),
        2 => (b"/memory.current", b"inactive_file "),
        _ => return None,
    };
    let usage = file_size_value(groups.memory.as_slice(), usage_file)?;
    let inactive = stat_field(groups.memory.as_slice(), inactive)?;
    Some(usage.saturating_sub(inactive))
}
fn cgroup_cpu_limit() -> Option<u32> {
    let groups = cgroups();
    if groups.cpu.length == 0 { return None; }
    let (quota, period) = match groups.version {
        1 => {
            let quota = file_size_value(groups.cpu.as_slice(), b"/cpu.cfs_quota_us")?; // -1 fails to parse: no limit
            let period = file_size_value(groups.cpu.as_slice(), b"/cpu.cfs_period_us")?;
            (quota, period)
        }
        2 => {
            let mut path = Path::new();
            if !path.set(groups.cpu.as_slice()) || !path.push(b"/cpu.max") { return None; }
            let mut buffer = [0u8; 128];
            let length = read_file(path.c_str(), &mut buffer)?;
            let mut parts = fields(&buffer[..length]);
            let (quota, period) = (parts.next()?, parts.next()?);
            if quota == b"max" { return None; }
            (parse_u64(quota)?.0, parse_u64(period)?.0)
        }
        _ => return None,
    };
    if quota == 0 || period == 0 { return None; }
    if quota <= period { return Some(1); }
    Some(quota.div_ceil(period).min(u32::MAX as u64) as u32)
}
fn physical_total() -> u64 {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    if pages > 0 { pages as u64 * page() } else { 0 }
}
fn mem_available() -> Option<u64> {
    let mut lines = Lines::open(b"/proc/meminfo\0")?;
    let mut line = [0u8; 2048];
    while let Some(text) = lines.next(&mut line) {
        if starts_with(text, b"MemAvailable:") { return parse_size(&text[13..]); }
    }
    None
}
fn affinity_mask(mask: &mut [u64; 128]) -> Result<usize> {
    let size = mem::size_of_val(mask);
    let rc = unsafe { libc::sched_getaffinity(0, size, mask.as_mut_ptr().cast()) };
    if rc != 0 { return Err(Error::Os); }
    Ok(size)
}

impl port::Topology for Linux {
    fn cpu_max() -> Result<u32> {
        let mut buffer = [0u8; 256];
        if let Some(length) = read_file(b"/sys/devices/system/cpu/possible\0", &mut buffer) {
            // "0-3" or "0,2-5": the highest index plus one.
            let mut highest: Option<u64> = None;
            for range in trim(&buffer[..length]).split(|b| *b == b',') {
                let mut ends = range.split(|b| *b == b'-');
                let first = ends.next().and_then(|e| parse_u64(e).map(|v| v.0));
                let last = ends.next().and_then(|e| parse_u64(e).map(|v| v.0)).or(first);
                if let Some(last) = last { highest = Some(highest.map_or(last, |h| h.max(last))); }
            }
            if let Some(highest) = highest { return u32::try_from(highest + 1).map_err(|_| Error::Os); }
        }
        let value = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
        if value > 0 { u32::try_from(value).map_err(|_| Error::Os) } else { Err(Error::Os) }
    }
    fn cpu_count() -> Result<u32> {
        let mut mask = [0u64; 128];
        let mut count = match affinity_mask(&mut mask) {
            Ok(_) => mask.iter().map(|w| w.count_ones()).sum::<u32>(),
            Err(_) => 0,
        };
        if count == 0 {
            let online = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
            count = if online > 0 { online as u32 } else { return Err(Error::Os); };
        }
        if let Some(limit) = cgroup_cpu_limit() { count = count.min(limit); }
        Ok(count.max(1))
    }
    fn current_cpu() -> Result<u32> {
        let value = unsafe { libc::sched_getcpu() };
        if value < 0 { Err(Error::Os) } else { Ok(value as u32) }
    }
    unsafe fn process_affinity(out: *mut u8, capacity: usize) -> Result<usize> {
        let mut mask = [0u64; 128];
        affinity_mask(&mut mask)?;
        let needed = (Self::cpu_max()? as usize).div_ceil(8);
        let bytes = unsafe { core::slice::from_raw_parts(mask.as_ptr().cast::<u8>(), mem::size_of_val(&mask)) };
        let copy = needed.min(capacity).min(bytes.len());
        // Little-endian words are already the LSB-first byte layout the ABI describes.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, copy) };
        Ok(needed)
    }
    fn set_thread_affinity(cpu: u32) -> Result<()> {
        let mut mask = [0u64; 128];
        if cpu as usize >= mask.len() * 64 { return Err(Error::InvalidArgument); }
        mask[cpu as usize / 64] = 1 << (cpu % 64);
        if unsafe { libc::sched_setaffinity(0, mem::size_of_val(&mask), mask.as_ptr().cast()) } == 0 { Ok(()) } else { Err(Error::Os) }
    }
    fn physical_memory() -> Result<(u64, u64)> {
        let machine = physical_total();
        if machine == 0 { return Err(Error::Os); }
        if let Some(limit) = Self::memory_limit().ok().filter(|l| *l != 0) {
            let used = cgroup_memory_usage().unwrap_or(0);
            return Ok((limit, limit.saturating_sub(used).min(limit)));
        }
        let available = match mem_available() {
            Some(v) => v,
            None => { let pages = unsafe { libc::sysconf(libc::_SC_AVPHYS_PAGES) }; if pages > 0 { pages as u64 * page() } else { 0 } }
        };
        Ok((machine, available.min(machine)))
    }
    fn memory_limit() -> Result<u64> {
        let Some(limit) = cgroup_memory_limit() else { return Ok(0); };
        if limit > 0x7FFF_FFFF_0000_0000 { return Ok(0); }
        let mut result = limit;
        if let Ok(rlimit) = Self::virtual_limit() { if rlimit != 0 { result = result.min(rlimit); } }
        let machine = physical_total();
        if machine != 0 { result = result.min(machine); }
        Ok(result)
    }
    fn virtual_limit() -> Result<u64> {
        let mut limit = mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_AS, limit.as_mut_ptr()) } != 0 { return Err(Error::Os); }
        let limit = unsafe { limit.assume_init() };
        Ok(if limit.rlim_cur == libc::RLIM_INFINITY { 0 } else { limit.rlim_cur })
    }
    fn cache_size() -> Result<usize> {
        let mut best: u64 = 0;
        for name in [libc::_SC_LEVEL1_DCACHE_SIZE, libc::_SC_LEVEL2_CACHE_SIZE, libc::_SC_LEVEL3_CACHE_SIZE, libc::_SC_LEVEL4_CACHE_SIZE] {
            let value = unsafe { libc::sysconf(name) };
            if value > 0 { best = best.max(value as u64); }
        }
        if best == 0 {
            for index in 0..4u8 {
                let mut path = *b"/sys/devices/system/cpu/cpu0/cache/index0/size\0";
                path[41] = b'0' + index;
                let mut buffer = [0u8; 64];
                if let Some(length) = read_file(&path, &mut buffer) {
                    if let Some(value) = parse_size(&buffer[..length]) { best = best.max(value); }
                }
            }
        }
        usize::try_from(best).map_err(|_| Error::Os)
    }
    fn cache_level_size(level: u32) -> Result<usize> {
        let name = match level {
            1 => libc::_SC_LEVEL1_DCACHE_SIZE, 2 => libc::_SC_LEVEL2_CACHE_SIZE, 3 => libc::_SC_LEVEL3_CACHE_SIZE, 4 => libc::_SC_LEVEL4_CACHE_SIZE,
            _ => return Err(Error::InvalidArgument),
        };
        let value = unsafe { libc::sysconf(name) };
        if value > 0 { return usize::try_from(value as u64).map_err(|_| Error::Os); }
        // The C library answers these from the CPU on x86 and from nothing at all on AArch64; sysfs has them there.
        let mut best: u64 = 0;
        for index in 0..8u8 {
            let mut buffer = [0u8; 64];
            let Some(found) = cache_attribute(index, b"/level", &mut buffer) else { continue; };
            if parse_size(&buffer[..found]) != Some(level as u64) { continue; }
            let Some(kind) = cache_attribute(index, b"/type", &mut buffer) else { continue; };
            // An instruction cache is not what a consumer sizing its data structures asks for.
            if !(starts_with(&buffer[..kind], b"Data") || starts_with(&buffer[..kind], b"Unified")) { continue; }
            let Some(size) = cache_attribute(index, b"/size", &mut buffer) else { continue; };
            if let Some(found) = parse_size(&buffer[..size]) { best = best.max(found); }
        }
        usize::try_from(best).map_err(|_| Error::Os)
    }
    fn swap_memory() -> Result<(u64, u64)> {
        let mut info = mem::MaybeUninit::<libc::sysinfo>::uninit();
        if unsafe { libc::sysinfo(info.as_mut_ptr()) } != 0 { return Err(Error::Os); }
        let info = unsafe { info.assume_init() };
        // Older kernels report every figure in bytes, which they mark with a unit of zero.
        let unit = if info.mem_unit == 0 { 1u64 } else { info.mem_unit as u64 };
        let (mut total, mut available) = (info.totalswap as u64 * unit, info.freeswap as u64 * unit);
        // A container's own limit stands in for the machine's, as it does for physical memory.
        if let Some((limit, used)) = cgroup_swap() {
            total = total.min(limit);
            available = available.min(limit.saturating_sub(used));
        }
        Ok((total, available.min(total)))
    }
    fn cpu_features() -> Result<(u64, u64)> {
        #[cfg(target_arch = "aarch64")]
        { Ok((unsafe { libc::getauxval(libc::AT_HWCAP) } as u64, unsafe { libc::getauxval(libc::AT_HWCAP2) } as u64)) }
        #[cfg(not(target_arch = "aarch64"))]
        { Ok((0, 0)) }
    }
}

fn message(out: *mut u8, capacity: usize, text: &[u8]) {
    if capacity == 0 || out.is_null() { return; }
    let n = text.len().min(capacity - 1);
    unsafe { ptr::copy_nonoverlapping(text.as_ptr(), out, n); out.add(n).write(0) };
}
impl port::Process for Linux {
    fn exit(code: i32) -> ! { unsafe { libc::exit(code) } }
    fn debugger_present() -> Result<bool> {
        let mut lines = Lines::open(b"/proc/self/status\0").ok_or(Error::Os)?;
        let mut line = [0u8; 2048];
        while let Some(text) = lines.next(&mut line) {
            if starts_with(text, b"TracerPid:") { return Ok(parse_u64(&text[10..]).map_or(false, |v| v.0 != 0)); }
        }
        Err(Error::Os)
    }
    unsafe fn crash_dump(argv: &[*const u8], error: *mut u8, capacity: usize) -> Result<()> {
        let mut arguments = [ptr::null::<libc::c_char>(); crate::process::MAX_ARGUMENTS + 1];
        for (slot, argument) in arguments.iter_mut().zip(argv) { *slot = argument.cast(); }
        let mut pipes = [0i32; 4];
        if unsafe { libc::pipe2(pipes.as_mut_ptr(), libc::O_CLOEXEC) } != 0 { message(error, capacity, b"crash dump: pipe failed"); return Err(Error::Os); }
        if unsafe { libc::pipe2(pipes.as_mut_ptr().add(2), libc::O_CLOEXEC) } != 0 {
            unsafe { libc::close(pipes[0]); libc::close(pipes[1]) };
            message(error, capacity, b"crash dump: pipe failed"); return Err(Error::Os);
        }
        let (child_read, parent_write, parent_read, child_write) = (pipes[0], pipes[1], pipes[2], pipes[3]);
        let child = unsafe { libc::fork() };
        if child < 0 {
            for fd in pipes { unsafe { libc::close(fd) }; }
            message(error, capacity, b"crash dump: fork failed"); return Err(Error::Os);
        }
        if child == 0 {
            // Child: only async-signal-safe calls. Wait for the parent's go signal,
            // then route diagnostics to the parent when it wants them.
            unsafe { libc::close(parent_read); libc::close(parent_write) };
            let mut byte = 0u8;
            let got = loop {
                let n = unsafe { libc::read(child_read, (&mut byte as *mut u8).cast(), 1) };
                if n < 0 && errno() == libc::EINTR { continue; }
                break n;
            };
            unsafe { libc::close(child_read) };
            if got != 1 { unsafe { libc::_exit(255) }; }
            if capacity != 0 { unsafe { libc::dup2(child_write, libc::STDERR_FILENO) }; }
            unsafe { libc::execv(arguments[0], arguments.as_ptr()) };
            unsafe { libc::_exit(255) };
        }
        unsafe { libc::close(child_read); libc::close(child_write) };
        // Allow the utility to attach to this process on Yama-restricted systems.
        unsafe { libc::prctl(libc::PR_SET_PTRACER, child as libc::c_ulong, 0, 0, 0) };
        let written = loop {
            let n = unsafe { libc::write(parent_write, b"S".as_ptr().cast(), 1) };
            if n < 0 && errno() == libc::EINTR { continue; }
            break n;
        };
        unsafe { libc::close(parent_write) };
        if written != 1 {
            unsafe { libc::close(parent_read) };
            let mut status = 0;
            unsafe { libc::waitpid(child, &mut status, 0) };
            message(error, capacity, b"crash dump: cannot signal the utility"); return Err(Error::Os);
        }
        let mut filled = 0usize;
        if capacity > 1 {
            loop {
                let n = unsafe { libc::read(parent_read, error.add(filled).cast(), capacity - 1 - filled) };
                if n < 0 && errno() == libc::EINTR { continue; }
                if n <= 0 { break; }
                filled += n as usize;
                if filled >= capacity - 1 { break; }
            }
            unsafe { error.add(filled).write(0) };
        }
        unsafe { libc::close(parent_read) };
        let mut status = 0;
        let waited = unsafe { libc::waitpid(child, &mut status, 0) };
        if waited != child { message(error, capacity, b"crash dump: waitpid failed"); return Err(Error::Os); }
        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) != 0 {
            if filled == 0 { message(error, capacity, b"crash dump: the utility failed"); }
            return Err(Error::Os);
        }
        Ok(())
    }
}

const PT_LOAD: u32 = 1;
const PT_NOTE: u32 = 4;
const PT_GNU_EH_FRAME: u32 = 0x6474e550;
const NT_GNU_BUILD_ID: u32 = 3;
#[repr(C)]
struct Nhdr { namesz: u32, descsz: u32, kind: u32 }
struct UnwindSearch { address: usize, result: Option<UnwindInfo> }
extern "C" fn unwind_callback(info: *mut libc::dl_phdr_info, _size: usize, data: *mut c_void) -> i32 {
    let (info, search) = unsafe { (&*info, &mut *data.cast::<UnwindSearch>()) };
    let base = info.dlpi_addr as usize;
    if info.dlpi_phnum == 0 || search.address < base { return 0; }
    let headers = unsafe { core::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize) };
    let mut result = UnwindInfo { base, ..UnwindInfo::default() };
    let Some(text) = headers.iter().find(|h| h.p_type == PT_LOAD && (base + h.p_vaddr as usize..).contains(&search.address)
        && search.address < base + h.p_vaddr as usize + h.p_memsz as usize) else { return 0; };
    result.text_start = base + text.p_vaddr as usize;
    result.text_length = text.p_memsz as usize;
    let Some(hdr) = headers.iter().rev().find(|h| h.p_type == PT_GNU_EH_FRAME) else { return 0; };
    result.eh_frame_hdr = base + hdr.p_vaddr as usize;
    result.eh_frame_hdr_length = hdr.p_memsz as usize;
    search.result = Some(result);
    1
}
struct BuildIdSearch { base: usize, note: *const u8, length: usize }
extern "C" fn build_id_callback(info: *mut libc::dl_phdr_info, _size: usize, data: *mut c_void) -> i32 {
    let (info, search) = unsafe { (&*info, &mut *data.cast::<BuildIdSearch>()) };
    let bias = info.dlpi_addr as usize;
    let headers = unsafe { core::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize) };
    // The module base is the first PT_LOAD start, as the runtime computes it from dladdr.
    if headers.iter().find(|h| h.p_type == PT_LOAD).map(|h| bias + h.p_vaddr as usize) != Some(search.base) { return 0; }
    for header in headers.iter().filter(|h| h.p_type == PT_NOTE) {
        let start = bias + header.p_vaddr as usize;
        let align = (header.p_align as usize).max(4);
        let mut offset = 0usize;
        let size = header.p_memsz as usize;
        while offset + mem::size_of::<Nhdr>() <= size {
            let note = unsafe { &*((start + offset) as *const Nhdr) };
            let name = start + offset + mem::size_of::<Nhdr>();
            let desc = name + (note.namesz as usize).next_multiple_of(align);
            if note.namesz == 4 && note.kind == NT_GNU_BUILD_ID && unsafe { core::slice::from_raw_parts(name as *const u8, 4) } == b"GNU\0" {
                search.note = desc as *const u8; search.length = note.descsz as usize; return 1;
            }
            offset += mem::size_of::<Nhdr>() + (note.namesz as usize).next_multiple_of(align) + (note.descsz as usize).next_multiple_of(align);
        }
    }
    0
}
static PROBE_FAILURES: AtomicUsize = AtomicUsize::new(0);
impl port::Image for Linux {
    unsafe fn unwind_info(address: usize) -> Result<UnwindInfo> {
        let mut search = UnwindSearch { address, result: None };
        unsafe { libc::dl_iterate_phdr(Some(unwind_callback), (&mut search as *mut UnwindSearch).cast()) };
        search.result.ok_or(Error::NotFound)
    }
    unsafe fn readable(address: usize, size: usize) -> Result<bool> {
        // rt_sigprocmask copies the new set from user space before validating `how`:
        // an invalid `how` yields EINVAL for readable memory and EFAULT otherwise.
        let page = page() as usize;
        let last = address.checked_add(size - 1).ok_or(Error::InvalidArgument)?;
        let mut probe = address & !(page - 1);
        loop {
            let rc = unsafe { libc::syscall(libc::SYS_rt_sigprocmask, -1i32, probe as *const c_void, ptr::null::<c_void>(), 8usize) };
            match (rc, errno()) {
                (0, _) => { PROBE_FAILURES.fetch_add(1, Ordering::Relaxed); return Err(Error::Os); } // never expected
                (_, libc::EFAULT) => return Ok(false),
                (_, libc::EINVAL) => {}
                _ => return Err(Error::Os),
            }
            if probe + page > last || probe.checked_add(page).is_none() { return Ok(true); }
            probe += page;
        }
    }
    unsafe fn build_id(base: usize, out: *mut u8, capacity: usize) -> Result<usize> {
        let mut search = BuildIdSearch { base, note: ptr::null(), length: 0 };
        unsafe { libc::dl_iterate_phdr(Some(build_id_callback), (&mut search as *mut BuildIdSearch).cast()) };
        if search.note.is_null() || search.length == 0 { return Err(Error::NotFound); }
        unsafe { ptr::copy_nonoverlapping(search.note, out, search.length.min(capacity)) };
        Ok(search.length)
    }
}

impl port::Streams for Linux {
    unsafe fn write(stream: u32, data: *const u8, size: usize) -> Result<usize> {
        loop {
            let n = unsafe { libc::write(stream as i32, data.cast(), size) };
            if n < 0 { if errno() == libc::EINTR { continue; } return Err(Error::Os); }
            return Ok(n as usize);
        }
    }
    unsafe fn read(stream: u32, out: *mut u8, capacity: usize) -> Result<usize> {
        loop {
            let n = unsafe { libc::read(stream as i32, out.cast(), capacity) };
            if n < 0 { if errno() == libc::EINTR { continue; } return Err(Error::Os); }
            return Ok(n as usize);
        }
    }
    fn is_terminal(stream: u32) -> Result<bool> {
        if unsafe { libc::isatty(stream as i32) } == 1 { return Ok(true); }
        // isatty reports ENOTTY for redirected streams and EBADF for closed ones.
        match errno() { libc::ENOTTY | libc::EINVAL => Ok(false), _ => Err(Error::Os) }
    }
}
