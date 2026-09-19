#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
//! Platform-independent PAL contracts, C ABI, validation and negotiation.
//!
//! This crate has no OS dependencies, provider implementations or backend
//! features. Select a separate platform crate or implement `port` traits and
//! export an entry point with `define_pal!`. Allocation, panic policy and OS
//! ownership remain the final application's responsibilities.
#[cfg(not(target_has_atomic = "ptr"))]
compile_error!("the PAL requires pointer-width atomics");

/// C contract headers shipped with this package, for native bridge build tools.
pub const C_INCLUDE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/include");

use core::{cell::UnsafeCell, ffi::c_void, mem, ptr, sync::atomic::{AtomicU8, Ordering}};
mod counter;
use counter::Counter;
// Safety contracts of the provider methods are stated once per trait, not per method.
#[allow(clippy::missing_safety_doc)]
pub mod port;
pub mod services;
pub mod kernel;
pub mod runtime;
pub mod wasi;
pub mod context;
pub mod support;
pub mod topology;
pub mod process;
pub mod image;
pub mod streams;
pub mod io;
pub mod files;
pub mod sockets;
pub mod faults;
pub mod system;
pub mod notifications;
pub mod processes;
pub mod terminal;
pub mod watches;
pub mod mappings;
pub mod volumes;
pub mod network;
pub mod local_sockets;
pub mod accounts;
pub mod priority;
pub mod packets;
pub mod spawn_as;
use port::{LinearStorage, Port, VirtualMemory};

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
#[derive(Clone, Copy)]
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
    pub support: support::Ops,
    // Appended after the support group: earlier prefix offsets are unchanged.
    pub topology: topology::Ops,
    pub process: process::Ops,
    pub image: image::Ops,
    pub streams: streams::Ops,
    // Appended after the streams group.
    pub files: files::Ops,
    pub sockets: sockets::Ops,
    pub faults: faults::Ops,
    pub system: system::Ops,
    pub notifications: notifications::Ops,
    pub processes: processes::Ops,
    pub terminal: terminal::Ops,
    pub watches: watches::Ops,
    pub mappings: mappings::Ops,
    pub volumes: volumes::Ops,
    pub network: network::Ops,
    pub local_sockets: local_sockets::Ops,
    pub accounts: accounts::Ops,
    pub priority: priority::Ops,
    pub packets: packets::Ops,
    pub spawn_as: spawn_as::Ops,
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

mod vm {
    use super::*;
    pub unsafe extern "C" fn page_size<V: VirtualMemory>() -> usize { V::page_size() }
    pub unsafe extern "C" fn reserve<V: VirtualMemory>(size: usize, alignment: usize, flags: u32, out: *mut *mut c_void) -> u32 {
        if !aligned_output(out) { return record(INVALID_ARGUMENT, &RESERVE, &FAILED); }
        // SAFETY: valid writable output storage is a caller precondition.
        unsafe { out.write(ptr::null_mut()) };
        if flags != 0 { return record(UNSUPPORTED, &RESERVE, &FAILED); }
        let Some((size, alignment)) = geometry(size, alignment, V::page_size()) else {
            return record(INVALID_ARGUMENT, &RESERVE, &FAILED);
        };
        let status = match unsafe { V::reserve(size, alignment) } {
            Ok(result) => {
                if result.is_null() || (result as usize) % alignment != 0 || (result as usize).checked_add(size).is_none() {
                    OS_ERROR // broken provider contract; never dereference such a result
                } else { unsafe { out.write(result) }; OK }
            }
            Err(e) => e.status(),
        };
        record(status, &RESERVE, &FAILED)
    }
    fn valid_range<V: VirtualMemory>(address: *mut c_void, size: usize) -> Option<usize> {
        let page = V::page_size();
        let size = round_size(size, page)?;
        if address.is_null() || (address as usize) & (page - 1) != 0 { return None; }
        (address as usize).checked_add(size)?;
        Some(size)
    }
    macro_rules! range_op {
        ($name:ident, $counter:ident) => {
            pub unsafe extern "C" fn $name<V: VirtualMemory>(address: *mut c_void, size: usize) -> u32 {
                let Some(size) = valid_range::<V>(address, size) else {
                    return record(INVALID_ARGUMENT, &$counter, &FAILED);
                };
                // SAFETY: caller owns the range and serializes conflicting operations.
                record(port::status(unsafe { V::$name(address, size) }), &$counter, &FAILED)
            }
        };
    }
    range_op!(commit, COMMIT);
    range_op!(decommit, DECOMMIT);
    range_op!(release, RELEASE);
    range_op!(reset, RESET);
    pub const EMPTY: VmOps = VmOps { page_size: None, reserve: None, commit: None, decommit: None, release: None, reset: None };
    pub fn ops<V: VirtualMemory>() -> VmOps {
        if !V::PROVIDED { return EMPTY; }
        VmOps {
            page_size: Some(page_size::<V>), reserve: Some(reserve::<V>), commit: Some(commit::<V>),
            decommit: Some(decommit::<V>), release: Some(release::<V>), reset: Some(reset::<V>),
        }
    }
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
mod linear_api {
    use super::*;
    static ALLOCATE: Counter = Counter::new();
    static ZERO: Counter = Counter::new();
    static FREE: Counter = Counter::new();
    static ERRORS: Counter = Counter::new();
    unsafe extern "C" fn granularity<L: LinearStorage>() -> usize { L::GRANULARITY }
    unsafe extern "C" fn capacity<L: LinearStorage>() -> usize { L::CAPACITY }
    unsafe extern "C" fn allocate<L: LinearStorage>(size: usize, alignment: usize, flags: u32, out: *mut *mut c_void) -> u32 {
        if !aligned_output(out) { return record(INVALID_ARGUMENT, &ALLOCATE, &ERRORS); }
        unsafe { out.write(ptr::null_mut()) };
        if flags != 0 { return record(UNSUPPORTED, &ALLOCATE, &ERRORS); }
        let Some((size, alignment)) = geometry(size, alignment, L::GRANULARITY) else {
            return record(INVALID_ARGUMENT, &ALLOCATE, &ERRORS);
        };
        let status = match unsafe { L::allocate(size, alignment) } {
            Ok(address) => {
                if address.is_null() || (address as usize) % alignment != 0 || (address as usize).checked_add(size).is_none() { OS_ERROR }
                else { unsafe { out.write(address) }; OK }
            }
            Err(e) => e.status(),
        };
        record(status, &ALLOCATE, &ERRORS)
    }
    unsafe extern "C" fn zero<L: LinearStorage>(address: *mut c_void, size: usize) -> u32 {
        if address.is_null() || size == 0 || (address as usize).checked_add(size).is_none() {
            return record(INVALID_ARGUMENT, &ZERO, &ERRORS);
        }
        record(port::status(unsafe { L::zero(address, size) }), &ZERO, &ERRORS)
    }
    unsafe extern "C" fn release<L: LinearStorage>(address: *mut c_void, size: usize) -> u32 {
        let Some(size) = round_size(size, L::GRANULARITY) else {
            return record(INVALID_ARGUMENT, &FREE, &ERRORS);
        };
        if address.is_null() { return record(INVALID_ARGUMENT, &FREE, &ERRORS); }
        record(port::status(unsafe { L::release(address, size) }), &FREE, &ERRORS)
    }
    unsafe extern "C" fn stats(out: *mut LinearStats, size: usize) -> u32 {
        if !aligned_output(out) || size < mem::size_of::<LinearStats>() { return INVALID_ARGUMENT; }
        unsafe { out.write(LinearStats {
            allocate_ok: ALLOCATE.load(), zero_ok: ZERO.load(),
            release_ok: FREE.load(), rejected_or_failed: ERRORS.load(),
        }) };
        OK
    }
    pub const EMPTY: LinearOps = LinearOps { granularity: None, capacity: None, allocate: None, zero: None, release: None, read_stats: None };
    pub fn ops<L: LinearStorage>() -> LinearOps {
        if !L::PROVIDED { return EMPTY; }
        LinearOps {
            granularity: Some(granularity::<L>), capacity: Some(capacity::<L>), allocate: Some(allocate::<L>),
            zero: Some(zero::<L>), release: Some(release::<L>), read_stats: Some(stats),
        }
    }
}

/// Builds the immutable table for a port, or `None` when the port rejects negotiation.
pub fn build<P: Port>() -> Option<Api> {
    // VM and linear storage are different contracts; a port advertises one of them.
    if P::VirtualMemory::PROVIDED && P::Linear::PROVIDED { return None; }
    if !P::validate() { return None; }
    if P::VirtualMemory::PROVIDED && !P::VirtualMemory::page_size().is_power_of_two() { return None; }
    if P::Linear::PROVIDED && (!P::Linear::GRANULARITY.is_power_of_two() || P::Linear::CAPACITY == 0) { return None; }
    let mut capabilities = 0;
    if P::VirtualMemory::PROVIDED { capabilities |= CAP_VM; }
    if P::Linear::PROVIDED { capabilities |= CAP_LINEAR; }
    if P::Linear::PROVIDED && P::Linear::DYNAMIC { capabilities |= CAP_DYNAMIC_LINEAR; }
    let (services_caps, services_ops) = services::negotiate::<P>();
    let (kernel_caps, kernel_ops) = kernel::negotiate::<P>();
    let (runtime_caps, runtime_ops) = runtime::negotiate::<P>();
    let (wasi_caps, wasi_ops) = wasi::negotiate::<P>();
    let (context_caps, context_ops) = context::negotiate::<P>()?;
    let (support_caps, support_ops) = support::negotiate::<P>();
    let (topology_caps, topology_ops) = topology::negotiate::<P>();
    let (process_caps, process_ops) = process::negotiate::<P>();
    let (image_caps, image_ops) = image::negotiate::<P>();
    let (streams_caps, streams_ops) = streams::negotiate::<P>();
    let (files_caps, files_ops) = files::negotiate::<P>();
    let (sockets_caps, sockets_ops) = sockets::negotiate::<P>();
    let (faults_caps, faults_ops) = faults::negotiate::<P>()?;
    let (system_caps, system_ops) = system::negotiate::<P>();
    let (notifications_caps, notifications_ops) = notifications::negotiate::<P>();
    let (processes_caps, processes_ops) = processes::negotiate::<P>();
    let (terminal_caps, terminal_ops) = terminal::negotiate::<P>();
    let (watches_caps, watches_ops) = watches::negotiate::<P>();
    let (mappings_caps, mappings_ops) = mappings::negotiate::<P>();
    let (volumes_caps, volumes_ops) = volumes::negotiate::<P>();
    let (network_caps, network_ops) = network::negotiate::<P>();
    let (local_sockets_caps, local_sockets_ops) = local_sockets::negotiate::<P>();
    let (accounts_caps, accounts_ops) = accounts::negotiate::<P>();
    let (priority_caps, priority_ops) = priority::negotiate::<P>();
    let (packets_caps, packets_ops) = packets::negotiate::<P>();
    let (spawn_as_caps, spawn_as_ops) = spawn_as::negotiate::<P>();
    capabilities |= services_caps | kernel_caps | runtime_caps | wasi_caps | context_caps | support_caps
        | topology_caps | process_caps | image_caps | streams_caps | files_caps | sockets_caps | faults_caps
        | system_caps | notifications_caps | processes_caps | terminal_caps
        | watches_caps | mappings_caps | volumes_caps | network_caps | local_sockets_caps | accounts_caps | priority_caps | packets_caps | spawn_as_caps;
    Some(Api {
        header: Header { abi_version: ABI_VERSION, struct_size: mem::size_of::<Api>() as u32, capabilities },
        vm: vm::ops::<P::VirtualMemory>(),
        read_stats: Some(read_stats),
        linear: linear_api::ops::<P::Linear>(),
        services: services_ops,
        kernel: kernel_ops,
        runtime: runtime_ops,
        wasi: wasi_ops,
        context: context_ops,
        support: support_ops,
        topology: topology_ops,
        process: process_ops,
        image: image_ops,
        streams: streams_ops,
        files: files_ops,
        sockets: sockets_ops,
        faults: faults_ops,
        system: system_ops,
        notifications: notifications_ops,
        processes: processes_ops,
        terminal: terminal_ops,
        watches: watches_ops,
        mappings: mappings_ops,
        volumes: volumes_ops,
        network: network_ops,
        local_sockets: local_sockets_ops,
        accounts: accounts_ops,
        priority: priority_ops,
        packets: packets_ops,
        spawn_as: spawn_as_ops,
    })
}

/// Storage for one negotiated table. Written once, then immutable for the
/// process or instance lifetime, so callers may keep the returned pointer.
pub struct Slot { api: UnsafeCell<mem::MaybeUninit<Api>>, state: AtomicU8 }
// SAFETY: the table is written only by the thread that wins the 0 -> 1 transition
// and read only after the state reaches 2; a rejected negotiation never publishes.
unsafe impl Sync for Slot {}
const UNINITIALIZED: u8 = 0;
const NEGOTIATING: u8 = 1;
const READY: u8 = 2;
const REJECTED: u8 = 3;
impl Slot {
    pub const fn new() -> Self { Self { api: UnsafeCell::new(mem::MaybeUninit::uninit()), state: AtomicU8::new(UNINITIALIZED) } }
}
impl Default for Slot { fn default() -> Self { Self::new() } }
/// Negotiates (once) and returns the table for `P`, or NULL.
///
/// A rejected negotiation is final: the runtime must not retry with different
/// host configuration after startup. Concurrent first callers wait for the winner.
pub fn negotiate<P: Port>(slot: &'static Slot, version: u32) -> *const Api {
    if version != ABI_VERSION { return ptr::null(); }
    loop {
        match slot.state.load(Ordering::Acquire) {
            READY => return slot.api.get().cast::<Api>(),
            REJECTED => return ptr::null(),
            NEGOTIATING => core::hint::spin_loop(),
            _ => {
                if slot.state.compare_exchange(UNINITIALIZED, NEGOTIATING, Ordering::AcqRel, Ordering::Acquire).is_err() { continue; }
                let outcome = match build::<P>() {
                    Some(api) => { unsafe { (*slot.api.get()).write(api) }; READY }
                    None => REJECTED,
                };
                slot.state.store(outcome, Ordering::Release);
            }
        }
    }
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
    fn capability_bits_never_alias() {
        let bits = [CAP_VM, CAP_LINEAR, CAP_DYNAMIC_LINEAR, services::CAP_CLOCK, services::CAP_SCHEDULER,
            kernel::ALL, runtime::ALL, wasi::CAP, context::CAP, support::ALL, topology::CAP, process::CAP, image::CAP, streams::CAP,
            files::CAP, sockets::CAP, faults::CAP, system::CAP, notifications::CAP, processes::CAP, terminal::CAP,
            watches::CAP, mappings::CAP, volumes::CAP, network::CAP, local_sockets::CAP, accounts::CAP, priority::CAP, packets::CAP, spawn_as::CAP];
        for (i, a) in bits.iter().enumerate() {
            assert_ne!(*a, 0);
            for b in &bits[i + 1..] { assert_eq!(a & b, 0, "capability groups overlap"); }
        }
    }
    declare_port! { struct Empty; }
    static EMPTY_SLOT: Slot = Slot::new();
    #[test]
    fn an_empty_port_negotiates_with_no_capabilities() {
        assert!(negotiate::<Empty>(&EMPTY_SLOT, 99).is_null());
        let api = negotiate::<Empty>(&EMPTY_SLOT, ABI_VERSION);
        assert!(!api.is_null());
        let api = unsafe { &*api };
        assert_eq!(api.header.capabilities, 0);
        assert!(api.vm.reserve.is_none() && api.linear.allocate.is_none() && api.services.monotonic_ns.is_none());
        assert!(api.kernel.event_create.is_none() && api.runtime.random_bytes.is_none() && api.support.write_stderr.is_none());
        assert!(api.topology.cpu_count.is_none() && api.process.exit.is_none() && api.image.unwind_info.is_none() && api.streams.write.is_none());
        assert!(api.files.open.is_none() && api.sockets.create.is_none() && api.faults.install.is_none() && api.faults.read_stats.is_some());
        assert!(api.system.text.is_none() && api.notifications.install.is_none() && api.processes.spawn.is_none() && api.terminal.window_size.is_none());
        assert!(api.watches.open.is_none() && api.mappings.map.is_none() && api.volumes.entry.is_none() && api.network.interface_entry.is_none());
        assert!(api.local_sockets.bind.is_none() && api.accounts.user_by_id.is_none() && api.priority.get.is_none());
        assert!(api.packets.receive.is_none() && api.spawn_as.spawn_as.is_none());
        assert!(core::ptr::eq(api, negotiate::<Empty>(&EMPTY_SLOT, ABI_VERSION)));
    }
    struct Rejecting;
    declare_port! { struct RejectingPort; Clock = Rejecting }
    impl port::Clock for Rejecting { fn monotonic_ns() -> port::Result<u64> { Ok(1) } }
    impl port::Port for Rejecting {
        type VirtualMemory = port::Absent; type Linear = port::Absent; type Clock = Rejecting; type Scheduler = port::Absent;
        type Events = port::Absent; type Mutexes = port::Absent; type Threads = port::Absent; type ThreadLocal = port::Absent;
        type StackBounds = port::Absent; type ProcessBarrier = port::Absent; type Environment = port::Absent; type Identity = port::Absent;
        type Realtime = port::Absent; type Entropy = port::Absent; type NativeMapping = port::Absent; type Modules = port::Absent;
        type NativeHeap = port::Absent; type RwLocks = port::Absent; type ThreadName = port::Absent; type Diagnostics = port::Absent;
        type Context = port::Absent; type Wasi = port::Absent; type Topology = port::Absent; type Process = port::Absent;
        type Image = port::Absent; type Streams = port::Absent; type Files = port::Absent; type Sockets = port::Absent;
        type Faults = port::Absent; type SystemInfo = port::Absent; type Notifications = port::Absent; type Processes = port::Absent;
        type Terminal = port::Absent; type Watches = port::Absent; type Mappings = port::Absent; type Volumes = port::Absent;
        type Network = port::Absent; type LocalSockets = port::Absent; type Accounts = port::Absent; type Priority = port::Absent;
        type Packets = port::Absent; type SpawnAs = port::Absent; type Abort = port::Trap;
        fn validate() -> bool { false }
    }
    static REJECT_SLOT: Slot = Slot::new();
    static CLOCK_SLOT: Slot = Slot::new();
    #[test]
    fn validation_failure_is_final_and_clock_only_ports_work() {
        assert!(negotiate::<Rejecting>(&REJECT_SLOT, ABI_VERSION).is_null());
        assert!(negotiate::<Rejecting>(&REJECT_SLOT, ABI_VERSION).is_null());
        let api = unsafe { &*negotiate::<RejectingPort>(&CLOCK_SLOT, ABI_VERSION) };
        assert_eq!(api.header.capabilities, services::CAP_CLOCK);
        let mut value = 0;
        assert_eq!(unsafe { api.services.monotonic_ns.unwrap()(&mut value) }, OK);
        assert_eq!(value, 1);
        assert!(api.services.sleep_ns.is_none());
    }
}
