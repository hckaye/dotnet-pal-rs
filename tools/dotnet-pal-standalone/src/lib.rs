#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
//! Standalone static-library configurations assembled from built-in providers.
//! Selected by the `linux`, `host*`, `linear*` and `wasi*` features; a library
//! consumer enables none of these and declares its own port instead.
#[cfg(all(feature = "linux", any(feature = "host", feature = "linear")))]
compile_error!("select exactly one qualification memory backend");
#[cfg(all(feature = "linux", not(target_os = "linux")))]
compile_error!("linux requires a Linux target");
#[cfg(all(feature = "storage-grow", not(target_arch = "wasm32")))]
compile_error!("storage-grow requires wasm32");
#[cfg(all(feature = "wasi-clock", not(all(target_arch = "wasm32", target_os = "wasi", target_env = "p1"))))]
compile_error!("wasi-clock requires wasm32-wasip1");
pub use dotnet_pal_rs::*;
use crate::port::{Absent, Port};

#[cfg(feature = "linux")]
type Linux = dotnet_pal_linux::Linux;
#[cfg(feature = "host")]
type Host = dotnet_pal_host::Host;

pub struct Standalone;
impl Port for Standalone {
    #[cfg(feature = "linux")] type VirtualMemory = Linux;
    #[cfg(all(feature = "host", not(feature = "linear")))] type VirtualMemory = Host;
    #[cfg(not(any(feature = "linux", all(feature = "host", not(feature = "linear")))))] type VirtualMemory = Absent;

    #[cfg(all(feature = "linear", not(feature = "linear-heap"), not(feature = "linear-grow")))] type Linear = dotnet_pal_storage::Arena;
    #[cfg(feature = "linear-heap")] type Linear = dotnet_pal_storage::Ledger<dotnet_pal_storage::Hooks>;
    #[cfg(feature = "linear-grow")] type Linear = dotnet_pal_storage::Ledger<dotnet_pal_wasm::Grow>;
    #[cfg(not(feature = "linear"))] type Linear = Absent;

    #[cfg(feature = "linux")] type Clock = Linux;
    #[cfg(feature = "host-services")] type Clock = Host;
    #[cfg(feature = "wasi-clock")] type Clock = dotnet_pal_wasip1::Wasi;
    #[cfg(not(any(feature = "linux", feature = "host-services", feature = "wasi-clock")))] type Clock = Absent;

    #[cfg(feature = "linux")] type Scheduler = Linux;
    #[cfg(feature = "host-services")] type Scheduler = Host;
    #[cfg(not(any(feature = "linux", feature = "host-services")))] type Scheduler = Absent;

    #[cfg(feature = "linux")] type Events = Linux;
    #[cfg(feature = "host-kernel")] type Events = Host;
    #[cfg(not(any(feature = "linux", feature = "host-kernel")))] type Events = Absent;
    #[cfg(feature = "linux")] type Mutexes = Linux;
    #[cfg(feature = "host-kernel")] type Mutexes = Host;
    #[cfg(not(any(feature = "linux", feature = "host-kernel")))] type Mutexes = Absent;
    #[cfg(feature = "linux")] type Threads = Linux;
    #[cfg(feature = "host-kernel")] type Threads = Host;
    #[cfg(not(any(feature = "linux", feature = "host-kernel")))] type Threads = Absent;
    #[cfg(feature = "linux")] type ThreadLocal = Linux;
    #[cfg(feature = "host-kernel")] type ThreadLocal = Host;
    #[cfg(not(any(feature = "linux", feature = "host-kernel")))] type ThreadLocal = Absent;
    #[cfg(feature = "linux")] type StackBounds = Linux;
    #[cfg(feature = "host-kernel")] type StackBounds = Host;
    #[cfg(not(any(feature = "linux", feature = "host-kernel")))] type StackBounds = Absent;
    #[cfg(feature = "linux")] type ProcessBarrier = Linux;
    #[cfg(feature = "host-kernel")] type ProcessBarrier = Host;
    #[cfg(not(any(feature = "linux", feature = "host-kernel")))] type ProcessBarrier = Absent;

    #[cfg(feature = "linux")] type Environment = Linux;
    #[cfg(feature = "host-runtime")] type Environment = Host;
    #[cfg(feature = "wasi-runtime")] type Environment = dotnet_pal_wasip1::Wasi;
    #[cfg(not(any(feature = "linux", feature = "host-runtime", feature = "wasi-runtime")))] type Environment = Absent;
    #[cfg(feature = "linux")] type Identity = Linux;
    #[cfg(feature = "host-runtime")] type Identity = Host;
    #[cfg(not(any(feature = "linux", feature = "host-runtime")))] type Identity = Absent;
    #[cfg(feature = "linux")] type Realtime = Linux;
    #[cfg(feature = "host-runtime")] type Realtime = Host;
    #[cfg(feature = "wasi-runtime")] type Realtime = dotnet_pal_wasip1::Wasi;
    #[cfg(not(any(feature = "linux", feature = "host-runtime", feature = "wasi-runtime")))] type Realtime = Absent;
    #[cfg(feature = "linux")] type Entropy = Linux;
    #[cfg(feature = "host-runtime")] type Entropy = Host;
    #[cfg(feature = "wasi-runtime")] type Entropy = dotnet_pal_wasip1::Wasi;
    #[cfg(not(any(feature = "linux", feature = "host-runtime", feature = "wasi-runtime")))] type Entropy = Absent;
    #[cfg(feature = "linux")] type NativeMapping = Linux;
    #[cfg(feature = "host-runtime")] type NativeMapping = Host;
    #[cfg(not(any(feature = "linux", feature = "host-runtime")))] type NativeMapping = Absent;
    #[cfg(feature = "linux")] type Modules = Linux;
    #[cfg(feature = "host-runtime")] type Modules = Host;
    #[cfg(not(any(feature = "linux", feature = "host-runtime")))] type Modules = Absent;

    #[cfg(feature = "linux")] type NativeHeap = Linux;
    #[cfg(feature = "host-support")] type NativeHeap = Host;
    #[cfg(not(any(feature = "linux", feature = "host-support")))] type NativeHeap = Absent;
    #[cfg(feature = "linux")] type RwLocks = Linux;
    #[cfg(feature = "host-support")] type RwLocks = Host;
    #[cfg(not(any(feature = "linux", feature = "host-support")))] type RwLocks = Absent;
    #[cfg(feature = "linux")] type ThreadName = Linux;
    #[cfg(feature = "host-support")] type ThreadName = Host;
    #[cfg(not(any(feature = "linux", feature = "host-support")))] type ThreadName = Absent;
    #[cfg(feature = "linux")] type Diagnostics = Linux;
    #[cfg(feature = "host-support")] type Diagnostics = Host;
    #[cfg(not(any(feature = "linux", feature = "host-support")))] type Diagnostics = Absent;

    #[cfg(feature = "linux")] type Context = Linux;
    #[cfg(feature = "host-context")] type Context = Host;
    #[cfg(not(any(feature = "linux", feature = "host-context")))] type Context = Absent;


    #[cfg(feature = "wasi-dispatch")] type Wasi = dotnet_pal_wasip1::Dispatch;
    #[cfg(not(feature = "wasi-dispatch"))] type Wasi = Absent;

    #[cfg(feature = "linux")] type Topology = Linux;
    #[cfg(feature = "host-topology")] type Topology = Host;
    #[cfg(not(any(feature = "linux", feature = "host-topology")))] type Topology = Absent;
    #[cfg(feature = "linux")] type Process = Linux;
    #[cfg(feature = "host-process")] type Process = Host;
    #[cfg(not(any(feature = "linux", feature = "host-process")))] type Process = Absent;
    #[cfg(feature = "linux")] type Image = Linux;
    #[cfg(feature = "host-image")] type Image = Host;
    #[cfg(not(any(feature = "linux", feature = "host-image")))] type Image = Absent;
    #[cfg(feature = "linux")] type Streams = Linux;
    #[cfg(feature = "host-streams")] type Streams = Host;
    #[cfg(not(any(feature = "linux", feature = "host-streams")))] type Streams = Absent;

    #[cfg(feature = "linux")] type Files = Linux;
    #[cfg(feature = "host-files")] type Files = Host;
    #[cfg(not(any(feature = "linux", feature = "host-files")))] type Files = Absent;
    #[cfg(feature = "linux")] type Sockets = Linux;
    #[cfg(feature = "host-sockets")] type Sockets = Host;
    #[cfg(not(any(feature = "linux", feature = "host-sockets")))] type Sockets = Absent;
    // Linux reports faults as signals through the context group; only a host table reports them here.
    #[cfg(feature = "host-faults")] type Faults = Host;
    #[cfg(not(feature = "host-faults"))] type Faults = Absent;

    #[cfg(feature = "linux")] type SystemInfo = Linux;
    #[cfg(feature = "host-system")] type SystemInfo = Host;
    #[cfg(not(any(feature = "linux", feature = "host-system")))] type SystemInfo = Absent;
    #[cfg(feature = "linux")] type Notifications = Linux;
    #[cfg(feature = "host-notifications")] type Notifications = Host;
    #[cfg(not(any(feature = "linux", feature = "host-notifications")))] type Notifications = Absent;
    #[cfg(feature = "linux")] type Processes = Linux;
    #[cfg(feature = "host-processes")] type Processes = Host;
    #[cfg(not(any(feature = "linux", feature = "host-processes")))] type Processes = Absent;
    #[cfg(feature = "linux")] type Terminal = Linux;
    #[cfg(feature = "host-terminal")] type Terminal = Host;
    #[cfg(not(any(feature = "linux", feature = "host-terminal")))] type Terminal = Absent;

    #[cfg(feature = "linux")] type Watches = Linux;
    #[cfg(feature = "host-watches")] type Watches = Host;
    #[cfg(not(any(feature = "linux", feature = "host-watches")))] type Watches = Absent;
    #[cfg(feature = "linux")] type Mappings = Linux;
    #[cfg(feature = "host-mappings")] type Mappings = Host;
    #[cfg(not(any(feature = "linux", feature = "host-mappings")))] type Mappings = Absent;
    #[cfg(feature = "linux")] type Volumes = Linux;
    #[cfg(feature = "host-volumes")] type Volumes = Host;
    #[cfg(not(any(feature = "linux", feature = "host-volumes")))] type Volumes = Absent;
    #[cfg(feature = "linux")] type Network = Linux;
    #[cfg(feature = "host-network")] type Network = Host;
    #[cfg(not(any(feature = "linux", feature = "host-network")))] type Network = Absent;
    #[cfg(feature = "linux")] type LocalSockets = Linux;
    #[cfg(feature = "host-local-sockets")] type LocalSockets = Host;
    #[cfg(not(any(feature = "linux", feature = "host-local-sockets")))] type LocalSockets = Absent;
    #[cfg(feature = "linux")] type Accounts = Linux;
    #[cfg(feature = "host-accounts")] type Accounts = Host;
    #[cfg(not(any(feature = "linux", feature = "host-accounts")))] type Accounts = Absent;
    #[cfg(feature = "linux")] type Priority = Linux;
    #[cfg(feature = "host-priority")] type Priority = Host;
    #[cfg(not(any(feature = "linux", feature = "host-priority")))] type Priority = Absent;
    #[cfg(feature = "linux")] type Packets = Linux;
    #[cfg(feature = "host-packets")] type Packets = Host;
    #[cfg(not(any(feature = "linux", feature = "host-packets")))] type Packets = Absent;
    #[cfg(feature = "linux")] type SpawnAs = Linux;
    #[cfg(feature = "host-spawn-as")] type SpawnAs = Host;
    #[cfg(not(any(feature = "linux", feature = "host-spawn-as")))] type SpawnAs = Absent;

    #[cfg(feature = "linux")] type Abort = Linux;
    #[cfg(all(feature = "host", not(feature = "linux")))] type Abort = Host;
    #[cfg(not(any(feature = "linux", feature = "host")))] type Abort = crate::port::Trap;

    fn validate() -> bool {
        #[cfg(feature = "host")]
        return dotnet_pal_host::validate::<{ !cfg!(feature = "linear") }>();
        #[cfg(not(feature = "host"))]
        true
    }
}
static TABLE: crate::Slot = crate::Slot::new();
/// The only runtime-facing PAL entry point. Valid before managed runtime startup.
#[no_mangle]
pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const crate::Api {
    crate::negotiate::<Standalone>(&TABLE, version)
}
/// The negotiated table for built-in providers that route through it.
#[allow(dead_code)]
pub(crate) fn api() -> *const crate::Api { dotnet_pal_get_api(crate::ABI_VERSION) }
#[cfg(not(test))]
crate::define_panic_handler!(<Standalone as Port>::Abort);
