//! Linux machine provider. Native layouts stay here, never in the public table.
use crate::machine::*;
use crate::port::{Error, Machine, Result};
use core::mem;
const WORDS: usize = MAX_CPUS / (mem::size_of::<libc::c_ulong>() * 8);
fn scalar(name: i32) -> Result<u64> {
    let n = unsafe { libc::sysconf(name) };
    if n < 0 { Err(Error::Unsupported) } else { Ok(n as u64) }
}
fn bytes(pages: i32) -> Result<u64> {
    scalar(pages)?.checked_mul(scalar(libc::_SC_PAGESIZE)?).ok_or(Error::Os)
}
/// Linux CPU lists may be sparse: a count is not the highest CPU index plus one.
fn possible_list(data: &[u8]) -> Result<u64> {
    let text = core::str::from_utf8(data).map_err(|_| Error::Os)?.trim();
    let mut highest = None;
    for range in text.split(',') {
        let mut parts = range.split('-');
        let low = parts.next().ok_or(Error::Os)?.parse::<u32>().map_err(|_| Error::Os)?;
        let high = match parts.next() { Some(v) => v.parse::<u32>().map_err(|_| Error::Os)?, None => low };
        if parts.next().is_some() || high < low || high as usize >= MAX_CPUS || highest.is_some_and(|last| low <= last) { return Err(Error::Os); }
        highest = Some(high);
    }
    highest.map(|last| last as u64 + 1).ok_or(Error::Os)
}
fn possible() -> Result<u64> {
    let fd = unsafe { libc::open(c"/sys/devices/system/cpu/possible".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 { return Err(Error::Unsupported); }
    let mut buf = [0u8; 8192];
    let mut used = 0;
    let result = loop {
        if used == buf.len() { break Err(Error::Unsupported); }
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().add(used).cast(), buf.len() - used) };
        if n < 0 {
            if unsafe { *libc::__errno_location() } == libc::EINTR { continue; }
            break Err(Error::Os);
        }
        if n == 0 { break possible_list(&buf[..used]); }
        used += n as usize;
    };
    // Never retry close: the descriptor may have been consumed even on EINTR.
    unsafe { libc::close(fd) };
    result
}
impl Machine for crate::linux::Linux {
    fn query(kind: u32) -> Result<u64> {
        match kind {
            ONLINE_CPUS => scalar(libc::_SC_NPROCESSORS_ONLN),
            POSSIBLE_CPUS => possible(),
            PHYSICAL_BYTES => bytes(libc::_SC_PHYS_PAGES),
            AVAILABLE_BYTES => bytes(libc::_SC_AVPHYS_PAGES),
            PAGE_BYTES => scalar(libc::_SC_PAGESIZE),
            CACHE_L1 => scalar(libc::_SC_LEVEL1_DCACHE_SIZE),
            CACHE_L2 => scalar(libc::_SC_LEVEL2_CACHE_SIZE),
            CACHE_L3 => scalar(libc::_SC_LEVEL3_CACHE_SIZE),
            CACHE_L4 => scalar(libc::_SC_LEVEL4_CACHE_SIZE),
            ADDRESS_LIMIT => {
                let mut limit = mem::MaybeUninit::<libc::rlimit>::uninit();
                if unsafe { libc::getrlimit(libc::RLIMIT_AS, limit.as_mut_ptr()) } != 0 { return Err(Error::Os); }
                let limit = unsafe { limit.assume_init() }.rlim_cur;
                Ok(if limit == libc::RLIM_INFINITY { u64::MAX } else { limit as u64 })
            }
            SWAP_BYTES => {
                let mut info = mem::MaybeUninit::<libc::sysinfo>::uninit();
                if unsafe { libc::sysinfo(info.as_mut_ptr()) } != 0 { return Err(Error::Os); }
                let info = unsafe { info.assume_init() };
                (info.freeswap as u64).checked_mul(info.mem_unit as u64).ok_or(Error::Os)
            }
            _ => Err(Error::Unsupported),
        }
    }
    unsafe fn process_affinity(out: *mut u32, capacity: usize) -> Result<CpuList> {
        let mut mask = [0 as libc::c_ulong; WORDS];
        if unsafe { libc::sched_getaffinity(libc::getpid(), mem::size_of_val(&mask), mask.as_mut_ptr().cast()) } != 0 { return Err(Error::Os); }
        let count: usize = mask.iter().map(|v| v.count_ones() as usize).sum();
        if count == 0 { return Err(Error::Os); }
        if count > capacity { return Ok(CpuList::Required(count)); }
        let width = libc::c_ulong::BITS as usize;
        let mut n = 0;
        for (word, &bits) in mask.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                unsafe { out.add(n).write((word * width + bit) as u32) };
                n += 1; bits &= bits - 1;
            }
        }
        Ok(CpuList::Written(n))
    }
    fn bind_current(cpu: u32) -> Result<()> {
        if cpu as usize >= MAX_CPUS { return Err(Error::InvalidArgument); }
        let mut mask = [0 as libc::c_ulong; WORDS];
        let width = libc::c_ulong::BITS as usize;
        mask[cpu as usize / width] = 1 << (cpu as usize % width);
        if unsafe { libc::sched_setaffinity(0, mem::size_of_val(&mask), mask.as_ptr().cast()) } == 0 { Ok(()) } else { Err(Error::Os) }
    }
    fn current_cpu() -> Result<u32> {
        let cpu = unsafe { libc::sched_getcpu() };
        if cpu < 0 { Err(Error::Os) } else { Ok(cpu as u32) }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sparse_possible_indices() {
        assert_eq!(possible_list(b"0-3,8,32-63\n"), Ok(64));
        for x in [b"".as_slice(), b"3-2", b"0-2,2", b"0--3", b"1,0", b"65536", b"0-1,", b"max"] { assert!(possible_list(x).is_err(), "{:?}", x); }
    }
}
