#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
//! A versioned, allocation-free C boundary. Only the GC VM slice is implemented.
//! Pointer ownership, readable output storage and synchronization are caller contracts.

#[cfg(any(all(feature = "linux", feature = "host"), all(feature = "linux", feature = "arena"), all(feature = "host", feature = "arena")))]
compile_error!("select exactly one backend: linux OR host OR arena");
#[cfg(not(any(feature = "linux", feature = "host", feature = "arena")))]
compile_error!("select a backend: linux OR host OR arena");
#[cfg(all(feature = "linux", not(target_os = "linux")))]
compile_error!("the linux backend supports Linux only; use host for SDK targets");
#[cfg(all(feature = "arena", not(target_arch = "wasm32")))]
compile_error!("the reference arena backend is restricted to single-threaded wasm32");
#[cfg(all(feature = "arena", target_feature = "atomics"))]
compile_error!("the arena backend must not be built for shared-memory/atomic WASM");

use core::{ffi::c_void, mem, ptr, sync::atomic::Ordering};
mod counters;
use counters::Counter as AtomicU64;
#[cfg(feature = "linux")]
#[path = "linux.rs"]
mod backend;
#[cfg(all(feature = "host", not(feature = "linux")))]
#[path = "host.rs"]
mod backend;
#[cfg(feature = "arena")]
#[path = "arena.rs"]
mod backend;

pub const ABI_VERSION: u32 = 2;
pub const OK: u32 = 0;
pub const UNSUPPORTED: u32 = 1;
pub const INVALID_ARGUMENT: u32 = 2;
pub const OS_ERROR: u32 = 3;
// CAP_VM retains its original native, inaccessible-reservation contract.
pub const CAP_VM: u64 = 1;
pub const CAP_VM_LINEAR: u64 = 1 << 1;
pub const CAP_ZERO_RECOMMIT: u64 = 1 << 2;
pub const CAP_STATS: u64 = 1 << 3;

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
pub struct HostApi {
    pub header: Header,
    pub vm: VmOps,
}

#[repr(C)]
#[derive(Default)]
pub struct Stats {
    pub reserve_ok: u64,
    pub commit_ok: u64,
    pub decommit_ok: u64,
    pub release_ok: u64,
    pub reset_ok: u64,
    pub rejected_or_failed: u64,
}

#[repr(C)]
pub struct Api {
    pub header: Header,
    pub vm: VmOps,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}

static RESERVE: AtomicU64 = AtomicU64::new(0);
static COMMIT: AtomicU64 = AtomicU64::new(0);
static DECOMMIT: AtomicU64 = AtomicU64::new(0);
static RELEASE: AtomicU64 = AtomicU64::new(0);
static RESET: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);

fn record(status: u32, counter: &AtomicU64) -> u32 {
    let selected = if status == OK { counter } else { &FAILED };
    selected.fetch_add(1, Ordering::Relaxed);
    status
}

fn round_size(size: usize, page: usize) -> Option<usize> {
    if size == 0 || !page.is_power_of_two() { return None; }
    size.checked_add(page - 1).map(|n| n & !(page - 1))
}

fn valid_range(address: *mut c_void, size: usize) -> Option<usize> {
    let page = backend::page_size();
    let size = round_size(size, page)?;
    if address.is_null() || (address as usize) & (page - 1) != 0 { return None; }
    (address as usize).checked_add(size)?;
    Some(size)
}

unsafe extern "C" fn page_size() -> usize { backend::page_size() }

unsafe extern "C" fn reserve(size: usize, alignment: usize, flags: u32, out: *mut *mut c_void) -> u32 {
    if out.is_null() || (out as usize) % mem::align_of::<*mut c_void>() != 0 {
        return record(INVALID_ARGUMENT, &RESERVE);
    }
    // SAFETY: a non-null, aligned, writable output slot is a caller precondition.
    unsafe { out.write(ptr::null_mut()) };
    if flags != 0 { return record(UNSUPPORTED, &RESERVE); }
    let page = backend::page_size();
    let Some(size) = round_size(size, page) else { return record(INVALID_ARGUMENT, &RESERVE); };
    if alignment != 0 && !alignment.is_power_of_two() { return record(INVALID_ARGUMENT, &RESERVE); }
    let alignment = alignment.max(page);
    if size.checked_add(alignment - page).is_none() { return record(INVALID_ARGUMENT, &RESERVE); }
    // SAFETY: geometry and output have been validated; backend owns OS semantics.
    record(unsafe { backend::reserve(size, alignment, out) }, &RESERVE)
}

macro_rules! range_op {
    ($name:ident, $counter:ident) => {
        unsafe extern "C" fn $name(address: *mut c_void, size: usize) -> u32 {
            let Some(size) = valid_range(address, size) else {
                return record(INVALID_ARGUMENT, &$counter);
            };
            // SAFETY: caller owns this entire reservation/subrange and serializes changes.
            record(unsafe { backend::$name(address, size) }, &$counter)
        }
    };
}
range_op!(commit, COMMIT);
range_op!(decommit, DECOMMIT);
range_op!(release, RELEASE);
range_op!(reset, RESET);

unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if out.is_null() || size < mem::size_of::<Stats>() || (out as usize) % mem::align_of::<Stats>() != 0 {
        return INVALID_ARGUMENT;
    }
    // Diagnostics must not be a portability prerequisite or fake zero activity.
    if !cfg!(target_has_atomic = "64") { return UNSUPPORTED; }
    // Diagnostic snapshots are per-counter atomic, not a transactional snapshot.
    unsafe { out.write(Stats {
        reserve_ok: RESERVE.load(Ordering::Relaxed),
        commit_ok: COMMIT.load(Ordering::Relaxed),
        decommit_ok: DECOMMIT.load(Ordering::Relaxed),
        release_ok: RELEASE.load(Ordering::Relaxed),
        reset_ok: RESET.load(Ordering::Relaxed),
        rejected_or_failed: FAILED.load(Ordering::Relaxed),
    }) };
    OK
}

static API: Api = Api {
    header: Header {
        abi_version: ABI_VERSION,
        struct_size: mem::size_of::<Api>() as u32,
        capabilities: (if cfg!(feature = "arena") { CAP_VM_LINEAR } else { CAP_VM })
            | CAP_ZERO_RECOMMIT
            | (if cfg!(target_has_atomic = "64") { CAP_STATS } else { 0 }),
    },
    vm: VmOps {
        page_size: Some(page_size), reserve: Some(reserve), commit: Some(commit),
        decommit: Some(decommit), release: Some(release), reset: Some(reset),
    },
    read_stats: Some(read_stats),
};

/// The single runtime-facing entry point. Check capabilities, not just ABI version.
/// The arena profile requires host initialization before this returns a table.
#[no_mangle]
pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const Api {
    if version != ABI_VERSION || !backend::page_size().is_power_of_two() { return ptr::null(); }
    &API
}

/// Initialize the reference linear arena before any managed runtime starts.
///
/// # Safety
/// The memory is exclusive, writable, nonmoving and lives for the WASM instance.
/// It must not overlap code/data/stack/allocator storage or PAL metadata. Calls
/// must be serialized and non-reentrant; the instance memory must NOT be shared.
#[cfg(feature = "arena")]
#[no_mangle]
pub unsafe extern "C" fn dotnet_pal_arena_init(base: *mut c_void, size: usize, page: usize) -> u32 {
    unsafe { backend::initialize(base, size, page) }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // No unwinding through runtime / vendor ABI frames. Abort must never return.
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
        assert_eq!(round_size(0, 4096), None);
        assert_eq!(round_size(1, 0), None);
        assert_eq!(round_size(1, 3000), None);
    }
    #[test]
    fn negotiation_rejects_unknown_version() { assert!(dotnet_pal_get_api(99).is_null()); }
    #[test]
    fn native_and_linear_capabilities_do_not_alias() {
        assert_eq!(CAP_VM & CAP_VM_LINEAR, 0);
        assert_eq!(API.header.capabilities & CAP_ZERO_RECOMMIT, CAP_ZERO_RECOMMIT);
        assert_eq!(API.header.capabilities & CAP_VM, CAP_VM);
    }
}
