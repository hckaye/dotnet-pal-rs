//! CPU placement and machine measurements in platform-neutral units.
//! No cpu_set_t, sysinfo, rlimit, or platform selector numbers cross this ABI.
use crate::port::{self, Machine, Port};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR};
use core::{mem, ptr};

pub const CAP: u64 = 1 << 23;
pub const MAX_CPUS: usize = 65536;
pub const ONLINE_CPUS: u32 = 1;
pub const POSSIBLE_CPUS: u32 = 2;
pub const PHYSICAL_BYTES: u32 = 3;
pub const AVAILABLE_BYTES: u32 = 4;
pub const SWAP_BYTES: u32 = 5;
pub const ADDRESS_LIMIT: u32 = 6;
pub const CACHE_L1: u32 = 7;
pub const CACHE_L2: u32 = 8;
pub const CACHE_L3: u32 = 9;
pub const CACHE_L4: u32 = 10;
pub const PAGE_BYTES: u32 = 11;

/// Affinity is a snapshot of the process leader's allowed CPUs, not a promise
/// against concurrent administrative changes. The list is strictly increasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuList { Written(usize), Required(usize) }
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub query: Option<unsafe extern "C" fn(u32, *mut u64) -> u32>,
    pub process_affinity: Option<unsafe extern "C" fn(*mut u32, usize, *mut usize) -> u32>,
    pub bind_current: Option<unsafe extern "C" fn(u32) -> u32>,
    pub current_cpu: Option<unsafe extern "C" fn(*mut u32) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub query_ok: u64, pub affinity_ok: u64, pub bind_ok: u64,
    pub current_ok: u64, pub rejected: u64,
}
static COUNTERS: [Counter; 5] = [const { Counter::new() }; 5];
fn record(code: u32, index: usize) -> u32 {
    let code = match code {
        OK | INVALID_ARGUMENT | OS_ERROR | crate::UNSUPPORTED | crate::OUT_OF_MEMORY => code,
        crate::runtime::BUFFER_TOO_SMALL if index == 1 => code,
        _ => OS_ERROR,
    };
    COUNTERS[if code == OK { index } else { 4 }].increment();
    code
}
unsafe extern "C" fn query<M: Machine>(kind: u32, out: *mut u64) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(0) };
    if !(ONLINE_CPUS..=PAGE_BYTES).contains(&kind) { return record(crate::UNSUPPORTED, 0); }
    let code = match M::query(kind) {
        Ok(v) if matches!(kind, ONLINE_CPUS | POSSIBLE_CPUS) && (v == 0 || v > MAX_CPUS as u64) => OS_ERROR,
        Ok(v) if kind == PAGE_BYTES && (!v.is_power_of_two() || v > isize::MAX as u64) => OS_ERROR,
        Ok(0) if kind == PHYSICAL_BYTES => OS_ERROR,
        Ok(v) => { unsafe { out.write(v) }; OK }
        Err(e) => e.status(),
    };
    record(code, 0)
}
unsafe extern "C" fn affinity<M: Machine>(out: *mut u32, capacity: usize, count: *mut usize) -> u32 {
    if !aligned_output(count) { return record(INVALID_ARGUMENT, 1); }
    // Output objects may not overlap: zeroing an array must not overwrite count.
    if capacity > MAX_CPUS || (capacity != 0 && !aligned_output(out)) { return record(INVALID_ARGUMENT, 1); }
    let Some(end) = (out as usize).checked_add(capacity * mem::size_of::<u32>()) else { return record(INVALID_ARGUMENT, 1); };
    let Some(count_end) = (count as usize).checked_add(mem::size_of::<usize>()) else { return record(INVALID_ARGUMENT, 1); };
    if capacity != 0 && (out as usize) < count_end && (count as usize) < end { return record(INVALID_ARGUMENT, 1); }
    unsafe { count.write(0); if capacity != 0 { ptr::write_bytes(out, 0, capacity); } }
    let mut length = 0;
    let mut code = match unsafe { M::process_affinity(out, capacity) } {
        Ok(CpuList::Written(n)) if n > 0 && n <= capacity => {
            let values = unsafe { core::slice::from_raw_parts(out, n) };
            if values.iter().any(|&cpu| cpu as usize >= MAX_CPUS) || values.windows(2).any(|p| p[0] >= p[1]) { OS_ERROR }
            else { length = n; OK }
        }
        Ok(CpuList::Required(n)) if n > capacity && n <= MAX_CPUS => {
            length = n; crate::runtime::BUFFER_TOO_SMALL
        }
        Ok(_) => OS_ERROR,
        Err(e) => e.status(),
    };
    if code != OK && code != crate::runtime::BUFFER_TOO_SMALL { length = 0; }
    if code != OK && capacity != 0 { unsafe { ptr::write_bytes(out, 0, capacity); } }
    if length == 0 && code == crate::runtime::BUFFER_TOO_SMALL { code = OS_ERROR; }
    unsafe { count.write(length) };
    record(code, 1)
}
unsafe extern "C" fn bind<M: Machine>(cpu: u32) -> u32 {
    if cpu as usize >= MAX_CPUS { return record(INVALID_ARGUMENT, 2); }
    record(port::status(M::bind_current(cpu)), 2)
}
unsafe extern "C" fn current<M: Machine>(out: *mut u32) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 3); }
    unsafe { out.write(u32::MAX) };
    let code = match M::current_cpu() {
        Ok(v) if (v as usize) < MAX_CPUS => { unsafe { out.write(v) }; OK }
        Ok(_) => OS_ERROR,
        Err(e) => e.status(),
    };
    record(code, 3)
}
unsafe extern "C" fn stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { query_ok: COUNTERS[0].load(), affinity_ok: COUNTERS[1].load(),
        bind_ok: COUNTERS[2].load(), current_ok: COUNTERS[3].load(), rejected: COUNTERS[4].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { query: None, process_affinity: None, bind_current: None, current_cpu: None, read_stats: Some(stats) };
pub fn negotiate<P: Port>() -> (u64, Ops) {
    if !P::Machine::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { query: Some(query::<P::Machine>), process_affinity: Some(affinity::<P::Machine>),
        bind_current: Some(bind::<P::Machine>), current_cpu: Some(current::<P::Machine>), read_stats: Some(stats) })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{Error, Machine, Result};
    struct Model<const MODE: u8>;
    impl<const MODE: u8> Machine for Model<MODE> {
        fn query(kind: u32) -> Result<u64> {
            Ok(match MODE { 1 => 0, 2 => MAX_CPUS as u64 + 1,
                _ => if kind == PAGE_BYTES { 4096 } else { 8 } })
        }
        unsafe fn process_affinity(out: *mut u32, capacity: usize) -> Result<CpuList> {
            if MODE == 3 { return Ok(CpuList::Written(capacity + 1)); }
            if MODE == 4 { return Ok(CpuList::Required(MAX_CPUS + 1)); }
            if MODE == 5 { return Ok(CpuList::Required(capacity)); }
            if capacity < 2 { return Ok(CpuList::Required(2)); }
            unsafe { out.write(1); out.add(1).write(if MODE == 6 { 1 } else { 7 }); }
            if MODE == 7 { return Err(Error::Os); }
            Ok(CpuList::Written(2))
        }
        fn bind_current(_: u32) -> Result<()> { Ok(()) }
        fn current_cpu() -> Result<u32> { Ok(if MODE == 8 { u32::MAX } else { 7 }) }
    }
    #[test] fn query_validation_and_failure_output() {
        let mut n = 99;
        unsafe {
            assert_eq!(query::<Model<0>>(PAGE_BYTES, &mut n), OK); assert_eq!(n, 4096);
            assert_eq!(query::<Model<1>>(ONLINE_CPUS, &mut n), OS_ERROR); assert_eq!(n, 0);
            assert_eq!(query::<Model<2>>(ONLINE_CPUS, &mut n), OS_ERROR); assert_eq!(n, 0);
            assert_eq!(query::<Model<0>>(99, &mut n), crate::UNSUPPORTED); assert_eq!(n, 0);
            assert_eq!(query::<Model<0>>(ONLINE_CPUS, ptr::null_mut()), INVALID_ARGUMENT);
        }
    }
    #[test] fn affinity_sizing_and_copy() {
        let mut n = 99; let mut list = [99; 3];
        unsafe {
            assert_eq!(affinity::<Model<0>>(ptr::null_mut(), 0, &mut n), crate::runtime::BUFFER_TOO_SMALL);
            assert_eq!(n, 2);
            assert_eq!(affinity::<Model<0>>(list.as_mut_ptr(), 3, &mut n), OK);
        }
        assert_eq!(n, 2); assert_eq!(list, [1, 7, 0]);
    }
    fn rejected<const MODE:u8>() {
        let mut list = [99; 3]; let mut n = 99;
        assert_eq!(unsafe { affinity::<Model<MODE>>(list.as_mut_ptr(), 3, &mut n) }, OS_ERROR);
        assert_eq!(list, [0; 3]); assert_eq!(n, 0);
    }
    #[test] fn invalid_host_counts_and_duplicates_are_rejected() {
        rejected::<3>(); rejected::<4>(); rejected::<5>(); rejected::<6>(); rejected::<7>();
    }
    #[test] fn output_overlap_and_bad_geometry_are_rejected_without_access() {
        let mut words = [55usize; 4]; let p = words.as_mut_ptr();
        assert_eq!(unsafe { affinity::<Model<0>>(p.cast(), 2, p) }, INVALID_ARGUMENT);
        assert_eq!(words, [55; 4]);
        assert_eq!(unsafe { affinity::<Model<0>>(ptr::null_mut(), MAX_CPUS+1, p) }, INVALID_ARGUMENT);
        assert_eq!(words, [55; 4]);
    }
    #[test] fn current_cpu_and_binding_bounds() {
        let mut cpu=10;
        assert_eq!(unsafe { current::<Model<8>>(&mut cpu) }, OS_ERROR);
        assert_eq!(cpu,u32::MAX);
        assert_eq!(unsafe { current::<Model<0>>(&mut cpu) }, OK);
        assert_eq!(cpu,7);
        assert_eq!(unsafe { bind::<Model<0>>(MAX_CPUS as u32) },INVALID_ARGUMENT);
    }
}
