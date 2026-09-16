#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
//! Versioned, allocation-free OS-service boundary; VM and linear memory are distinct.
//! Raw-pointer validity, ownership and synchronization remain unsafe caller contracts.

#[cfg(any(all(feature = "linux", feature = "host"), all(feature = "linux", feature = "linear"), all(feature = "host", feature = "linear")))]
compile_error!("select exactly one backend: linux, host or linear");
#[cfg(not(any(feature = "linux", feature = "host", feature = "linear")))]
compile_error!("select a backend: linux, host or linear");
#[cfg(all(feature = "linux", not(target_os = "linux")))]
compile_error!("the linux backend supports Linux only; use host or linear");
#[cfg(not(target_has_atomic = "ptr"))]
compile_error!("this implementation requires pointer-width atomics (not 64-bit atomics)");
#[cfg(all(feature = "wasi-clock", not(all(feature = "linear", target_arch = "wasm32", target_os = "wasi", target_env = "p1"))))]
compile_error!("wasi-clock requires the linear backend on wasm32-wasip1");

use core::{ffi::c_void, mem, ptr};
mod counter;
use counter::Counter;
pub mod services;
pub mod kernel;
pub mod runtime;
pub mod wasi;
pub mod context;
pub mod elf;
#[cfg(feature = "linux")]
#[path = "linux.rs"]
mod backend;
#[cfg(all(feature = "host", not(feature = "linux")))]
#[path = "host.rs"]
mod backend;
#[cfg(all(feature = "linear", not(feature = "linear-heap")))]
mod linear;
#[cfg(feature = "linear-heap")]
#[path = "linear_heap.rs"]
mod linear;

pub const ABI_VERSION: u32 = 2;
pub const OK: u32 = 0;
pub const UNSUPPORTED: u32 = 1;
pub const INVALID_ARGUMENT: u32 = 2;
pub const OS_ERROR: u32 = 3;
pub const OUT_OF_MEMORY: u32 = 4;
pub const CAP_VM: u64 = 1;
pub const CAP_LINEAR: u64 = 2;
pub const CAP_DYNAMIC_LINEAR: u64 = 262144;

// Integers, not Rust enums, cross the ABI. Unknown values can be rejected safely.
pub type Reserve = unsafe extern "C" fn(usize, usize, u32, *mut *mut c_void) -> u32;
pub type Range = unsafe extern "C" fn(*mut c_void, usize) -> u32;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmOps {
    pub page_size: Option<unsafe extern "C" fn() -> usize>,
    pub reserve: Option<Reserve>,
    pub commit: Option<Range>,
    pub decommit: Option<Range>,
    pub release: Option<Range>,
    pub reset: Option<Range>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Header {
    pub abi_version: u32,
    pub struct_size: u32,
    pub capabilities: u64,
}
#[repr(C)]
pub struct HostApi { pub header: Header, pub vm: VmOps }
#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub reserve_ok: u64, pub commit_ok: u64, pub decommit_ok: u64,
    pub release_ok: u64, pub reset_ok: u64, pub rejected_or_failed: u64,
}
#[repr(C)]
#[derive(Default)]
pub struct LinearStats {
    pub allocate_ok: u64, pub zero_ok: u64, pub release_ok: u64, pub rejected_or_failed: u64,
}
#[repr(C)]
pub struct LinearOps {
    pub granularity: Option<unsafe extern "C" fn() -> usize>,
    pub capacity: Option<unsafe extern "C" fn() -> usize>,
    pub allocate: Option<Reserve>,
    pub zero: Option<Range>,
    pub release: Option<Range>,
    pub read_stats: Option<unsafe extern "C" fn(*mut LinearStats, usize) -> u32>,
}
#[repr(C)]
pub struct Api {
    pub header: Header,
    pub vm: VmOps,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
    // Additive ABI 2 extensions. Existing prefix and HostApi layout stay unchanged.
    pub linear: LinearOps,
    pub services: services::Ops,
    pub kernel: kernel::Ops,
    pub runtime: runtime::Ops,
    pub wasi: wasi::Ops,
    pub context: context::Ops,
    pub elf: elf::Ops,
}
static RESERVE: Counter = Counter::new();
static COMMIT: Counter = Counter::new();
static DECOMMIT: Counter = Counter::new();
static RELEASE: Counter = Counter::new();
static RESET: Counter = Counter::new();
static FAILED: Counter = Counter::new();

fn record(status: u32, counter: &Counter, failed: &Counter) -> u32 {
    if status == OK { counter.increment(); } else { failed.increment(); }
    status
}
fn round_size(size: usize, page: usize) -> Option<usize> {
    if size == 0 || !page.is_power_of_two() { return None; }
    let rounded = size.checked_add(page - 1)? & !(page - 1);
    (rounded <= isize::MAX as usize).then_some(rounded)
}
fn aligned_output<T>(out: *mut T) -> bool {
    !out.is_null() && (out as usize) % mem::align_of::<T>() == 0
}
fn geometry(size: usize, alignment: usize, page: usize) -> Option<(usize, usize)> {
    let size = round_size(size, page)?;
    if alignment != 0 && !alignment.is_power_of_two() { return None; }
    let alignment = alignment.max(page);
    if size.checked_add(alignment - page)? > isize::MAX as usize { return None; }
    Some((size, alignment))
}

#[cfg(any(feature = "linux", feature = "host"))]
mod vm {
    use super::*;
    unsafe extern "C" fn page_size() -> usize { backend::page_size() }
    unsafe extern "C" fn reserve(size: usize, alignment: usize, flags: u32, out: *mut *mut c_void) -> u32 {
        if !aligned_output(out) { return record(INVALID_ARGUMENT, &RESERVE, &FAILED); }
        // SAFETY: valid writable output storage is a caller precondition.
        unsafe { out.write(ptr::null_mut()) };
        if flags != 0 { return record(UNSUPPORTED, &RESERVE, &FAILED); }
        let Some((size, alignment)) = geometry(size, alignment, backend::page_size()) else {
            return record(INVALID_ARGUMENT, &RESERVE, &FAILED);
        };
        // Do not expose an output accidentally written by a failing foreign callback.
        let mut result: *mut c_void = ptr::null_mut();
        let mut status = unsafe { backend::reserve(size, alignment, &mut result) };
        if status == OK {
            if result.is_null() || (result as usize) % alignment != 0 || (result as usize).checked_add(size).is_none() {
                status = OS_ERROR; // broken host contract; never dereference such a result
            } else { unsafe { out.write(result) }; }
        }
        record(status, &RESERVE, &FAILED)
    }
    fn valid_range(address: *mut c_void, size: usize) -> Option<usize> {
        let page = backend::page_size();
        let size = round_size(size, page)?;
        if address.is_null() || (address as usize) & (page - 1) != 0 { return None; }
        (address as usize).checked_add(size)?;
        Some(size)
    }
    macro_rules! range_op {
        ($name:ident, $counter:ident) => {
            unsafe extern "C" fn $name(address: *mut c_void, size: usize) -> u32 {
                let Some(size) = valid_range(address, size) else {
                    return record(INVALID_ARGUMENT, &$counter, &FAILED);
                };
                // SAFETY: caller owns the range and serializes conflicting operations.
                record(unsafe { backend::$name(address, size) }, &$counter, &FAILED)
            }
        };
    }
    range_op!(commit, COMMIT);
    range_op!(decommit, DECOMMIT);
    range_op!(release, RELEASE);
    range_op!(reset, RESET);
    pub const OPS: VmOps = VmOps {
        page_size: Some(page_size), reserve: Some(reserve), commit: Some(commit),
        decommit: Some(decommit), release: Some(release), reset: Some(reset),
    };
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    // SAFETY: output must be writable. Individual counters, not a transactional snapshot.
    unsafe { out.write(Stats {
        reserve_ok: RESERVE.load(), commit_ok: COMMIT.load(), decommit_ok: DECOMMIT.load(),
        release_ok: RELEASE.load(), reset_ok: RESET.load(), rejected_or_failed: FAILED.load(),
    }) };
    OK
}
#[cfg(feature = "linear")]
mod linear_api {
    use super::*;
    static ALLOCATE: Counter = Counter::new();
    static ZERO: Counter = Counter::new();
    static FREE: Counter = Counter::new();
    static ERRORS: Counter = Counter::new();
    unsafe extern "C" fn granularity() -> usize { linear::GRANULARITY }
    unsafe extern "C" fn capacity() -> usize { linear::CAPACITY }
    unsafe extern "C" fn allocate(size: usize, alignment: usize, flags: u32, out: *mut *mut c_void) -> u32 {
        if !aligned_output(out) { return record(INVALID_ARGUMENT, &ALLOCATE, &ERRORS); }
        unsafe { out.write(ptr::null_mut()) };
        if flags != 0 { return record(UNSUPPORTED, &ALLOCATE, &ERRORS); }
        let Some((size, alignment)) = geometry(size, alignment, linear::GRANULARITY) else {
            return record(INVALID_ARGUMENT, &ALLOCATE, &ERRORS);
        };
        record(unsafe { linear::allocate(size, alignment, out) }, &ALLOCATE, &ERRORS)
    }
    unsafe extern "C" fn zero(address: *mut c_void, size: usize) -> u32 {
        if address.is_null() || size == 0 || (address as usize).checked_add(size).is_none() {
            return record(INVALID_ARGUMENT, &ZERO, &ERRORS);
        }
        record(unsafe { linear::zero(address, size) }, &ZERO, &ERRORS)
    }
    unsafe extern "C" fn release(address: *mut c_void, size: usize) -> u32 {
        let Some(size) = round_size(size, linear::GRANULARITY) else {
            return record(INVALID_ARGUMENT, &FREE, &ERRORS);
        };
        record(unsafe { linear::release(address, size) }, &FREE, &ERRORS)
    }
    unsafe extern "C" fn stats(out: *mut LinearStats, size: usize) -> u32 {
        if !aligned_output(out) || size < mem::size_of::<LinearStats>() { return INVALID_ARGUMENT; }
        unsafe { out.write(LinearStats {
            allocate_ok: ALLOCATE.load(), zero_ok: ZERO.load(),
            release_ok: FREE.load(), rejected_or_failed: ERRORS.load(),
        }) };
        OK
    }
    pub const OPS: LinearOps = LinearOps {
        granularity: Some(granularity), capacity: Some(capacity), allocate: Some(allocate),
        zero: Some(zero), release: Some(release), read_stats: Some(stats),
    };
}
const API_BASE: Api = Api {
    header: Header {
        abi_version: ABI_VERSION, struct_size: mem::size_of::<Api>() as u32,
        capabilities: (if cfg!(feature = "linear") { CAP_LINEAR } else { CAP_VM }) | services::CAPABILITIES | kernel::CAPABILITIES | runtime::CAPABILITIES | wasi::CAPABILITIES | context::CAPABILITIES | elf::CAPABILITIES | if cfg!(feature="linear-heap") { CAP_DYNAMIC_LINEAR } else { 0 },
    },
    #[cfg(not(feature = "linear"))]
    vm: vm::OPS,
    #[cfg(feature = "linear")]
    vm: VmOps { page_size: None, reserve: None, commit: None, decommit: None, release: None, reset: None },
    read_stats: Some(read_stats),
    #[cfg(feature = "linear")]
    linear: linear_api::OPS,
    #[cfg(not(feature = "linear"))]
    linear: LinearOps { granularity: None, capacity: None, allocate: None, zero: None, release: None, read_stats: None },
    services: services::OPS,
    kernel: kernel::OPS,
    runtime: runtime::OPS,
    wasi: wasi::OPS,
    context: context::OPS,
    elf: elf::OPS,
};
static API: Api = API_BASE;
#[cfg(feature = "linux")]
static API_NO_BARRIER: Api = Api {
    header: Header {
        abi_version: ABI_VERSION, struct_size: mem::size_of::<Api>() as u32,
        capabilities: API_BASE.header.capabilities & !kernel::CAP_BARRIER,
    },
    kernel: kernel::OPS_NO_BARRIER,
    ..API_BASE
};
/// The only runtime-facing PAL entry point. Valid before managed runtime startup.
#[no_mangle]
pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const Api {
    if version != ABI_VERSION || !services::available() || !kernel::available() || !runtime::available() || !context::available() || !elf::available() { return ptr::null(); }
    #[cfg(not(feature = "linear"))]
    if !backend::page_size().is_power_of_two() { return ptr::null(); }
    #[cfg(feature = "linux")]
    if !kernel::has_barrier() { return &API_NO_BARRIER; }
    &API
}
#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(feature = "linear")]
    unsafe { linear::abort() }
    #[cfg(not(feature = "linear"))]
    unsafe { backend::abort() }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rounding_is_checked() {
        assert_eq!(round_size(1, 4096), Some(4096));
        assert_eq!(round_size(4097, 4096), Some(8192));
        assert_eq!(round_size(usize::MAX, 4096), None);
        assert_eq!(round_size(isize::MAX as usize, 4096), None);
        assert_eq!(round_size(0, 4096), None);
        assert_eq!(round_size(1, 0), None);
        assert_eq!(round_size(1, 3000), None);
    }
    #[test]
    fn geometry_rejects_overflow_and_bad_alignment() {
        assert_eq!(geometry(1, 3, 4096), None);
        assert_eq!(geometry(1, 1, 4096), Some((4096, 4096)));
        assert_eq!(geometry(1, 1usize << (usize::BITS - 1), 4096), None);
    }
    #[test]
    fn negotiation_rejects_unknown_version() { assert!(dotnet_pal_get_api(99).is_null()); }
}
