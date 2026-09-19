//! Machine topology and memory accounting (`CAP_TOPOLOGY`).
//!
//! Counts are logical CPUs. Affinity masks are little-endian bitmaps (bit n is
//! CPU n). Memory figures are bytes within the limit in force for the process.
//! NUMA placement is deliberately not part of the boundary: every consumer sees
//! one node, so no port can pretend to place heaps on nodes it cannot see.
use crate::port::{self, Port, Topology};
use crate::runtime::BUFFER_TOO_SMALL;
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 8388608;
/// Upper bound on a reported CPU count: the GC stores processor numbers in 16 bits.
pub const MAX_CPUS: u32 = 65535;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub cpu_max: Option<unsafe extern "C" fn(*mut u32) -> u32>,
    pub cpu_count: Option<unsafe extern "C" fn(*mut u32) -> u32>,
    pub current_cpu: Option<unsafe extern "C" fn(*mut u32) -> u32>,
    pub process_affinity: Option<unsafe extern "C" fn(*mut u8, usize, *mut usize) -> u32>,
    pub set_thread_affinity: Option<unsafe extern "C" fn(u32) -> u32>,
    pub physical_memory: Option<unsafe extern "C" fn(*mut u64, *mut u64) -> u32>,
    pub memory_limit: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub virtual_limit: Option<unsafe extern "C" fn(*mut u64) -> u32>,
    pub cache_size: Option<unsafe extern "C" fn(*mut usize) -> u32>,
    pub cpu_features: Option<unsafe extern "C" fn(*mut u64, *mut u64) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub cpu_ok: u64, pub affinity_ok: u64, pub memory_ok: u64, pub cache_ok: u64, pub features_ok: u64, pub rejected_or_failed: u64 }
static COUNTERS: [Counter; 6] = [const { Counter::new() }; 6];

fn record(status: u32, index: usize) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR => status,
        BUFFER_TOO_SMALL if index == 1 => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { index } else { 5 }].increment();
    status
}
unsafe fn count(out: *mut u32, value: port::Result<u32>, nonzero: bool) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 0); }
    unsafe { out.write(0) };
    match value {
        Ok(v) if (nonzero && v == 0) || v > MAX_CPUS => record(OS_ERROR, 0),
        Ok(v) => { unsafe { out.write(v) }; record(OK, 0) }
        Err(e) => record(e.status(), 0),
    }
}
unsafe extern "C" fn cpu_max<T: Topology>(out: *mut u32) -> u32 { unsafe { count(out, T::cpu_max(), true) } }
unsafe extern "C" fn cpu_count<T: Topology>(out: *mut u32) -> u32 { unsafe { count(out, T::cpu_count(), true) } }
unsafe extern "C" fn current_cpu<T: Topology>(out: *mut u32) -> u32 { unsafe { count(out, T::current_cpu(), false) } }
unsafe extern "C" fn process_affinity<T: Topology>(mask: *mut u8, capacity: usize, needed: *mut usize) -> u32 {
    if !aligned_output(needed) { return record(INVALID_ARGUMENT, 1); }
    unsafe { needed.write(0) };
    if capacity > isize::MAX as usize || (capacity != 0 && (mask.is_null() || (mask as usize).checked_add(capacity).is_none())) {
        return record(INVALID_ARGUMENT, 1);
    }
    if capacity != 0 { unsafe { core::ptr::write_bytes(mask, 0, capacity) }; }
    let status = match unsafe { T::process_affinity(mask, capacity) } {
        Ok(0) => OS_ERROR, // an empty affinity set can run nowhere
        Ok(bytes) if bytes > (MAX_CPUS as usize).div_ceil(8) => OS_ERROR,
        Ok(bytes) => { unsafe { needed.write(bytes) }; if bytes > capacity { BUFFER_TOO_SMALL } else { OK } }
        Err(e) => e.status(),
    };
    if status != OK && capacity != 0 { unsafe { core::ptr::write_bytes(mask, 0, capacity) }; }
    record(status, 1)
}
unsafe extern "C" fn set_thread_affinity<T: Topology>(cpu: u32) -> u32 {
    if cpu > MAX_CPUS { return record(INVALID_ARGUMENT, 1); }
    record(port::status(T::set_thread_affinity(cpu)), 1)
}
unsafe extern "C" fn physical_memory<T: Topology>(total: *mut u64, available: *mut u64) -> u32 {
    if !aligned_output(total) || !aligned_output(available) { return record(INVALID_ARGUMENT, 2); }
    unsafe { total.write(0); available.write(0) };
    match T::physical_memory() {
        Ok((0, _)) => record(OS_ERROR, 2),
        Ok((t, a)) if a > t => record(OS_ERROR, 2),
        Ok((t, a)) => { unsafe { total.write(t); available.write(a) }; record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe fn limit(out: *mut u64, value: port::Result<u64>) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 2); }
    unsafe { out.write(0) };
    match value {
        Ok(v) => { unsafe { out.write(v) }; record(OK, 2) }
        Err(e) => record(e.status(), 2),
    }
}
unsafe extern "C" fn memory_limit<T: Topology>(out: *mut u64) -> u32 { unsafe { limit(out, T::memory_limit()) } }
unsafe extern "C" fn virtual_limit<T: Topology>(out: *mut u64) -> u32 { unsafe { limit(out, T::virtual_limit()) } }
unsafe extern "C" fn cache_size<T: Topology>(out: *mut usize) -> u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT, 3); }
    unsafe { out.write(0) };
    match T::cache_size() {
        Ok(v) => { unsafe { out.write(v) }; record(OK, 3) }
        Err(e) => record(e.status(), 3),
    }
}
unsafe extern "C" fn cpu_features<T: Topology>(first: *mut u64, second: *mut u64) -> u32 {
    if !aligned_output(first) || !aligned_output(second) { return record(INVALID_ARGUMENT, 4); }
    unsafe { first.write(0); second.write(0) };
    match T::cpu_features() {
        Ok((a, b)) => { unsafe { first.write(a); second.write(b) }; record(OK, 4) }
        Err(e) => record(e.status(), 4),
    }
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { cpu_ok: COUNTERS[0].load(), affinity_ok: COUNTERS[1].load(), memory_ok: COUNTERS[2].load(),
        cache_ok: COUNTERS[3].load(), features_ok: COUNTERS[4].load(), rejected_or_failed: COUNTERS[5].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { cpu_max: None, cpu_count: None, current_cpu: None, process_affinity: None, set_thread_affinity: None,
    physical_memory: None, memory_limit: None, virtual_limit: None, cache_size: None, cpu_features: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's topology provider. Every callback
/// is present when the capability is; a provider reports what it cannot know
/// with `Unsupported` per call rather than a missing pointer.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::Topology;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { cpu_max: Some(cpu_max::<T<P>>), cpu_count: Some(cpu_count::<T<P>>), current_cpu: Some(current_cpu::<T<P>>),
        process_affinity: Some(process_affinity::<T<P>>), set_thread_affinity: Some(set_thread_affinity::<T<P>>),
        physical_memory: Some(physical_memory::<T<P>>), memory_limit: Some(memory_limit::<T<P>>), virtual_limit: Some(virtual_limit::<T<P>>),
        cache_size: Some(cache_size::<T<P>>), cpu_features: Some(cpu_features::<T<P>>), read_stats: Some(read_stats) })
}
