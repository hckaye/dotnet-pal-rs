#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(target_os = "linux")]
//! Linux-only providers, reusable without exporting a C entry point.
mod linux;
mod linux_accounts;
mod linux_files;
mod linux_local_sockets;
mod linux_mappings;
mod linux_network;
mod linux_notifications;
mod linux_packets;
mod linux_platform;
mod linux_priority;
mod linux_processes;
mod linux_sockets;
mod linux_system;
mod linux_terminal;
mod linux_volumes;
mod linux_watches;
pub use linux::Linux;
dotnet_pal_rs::declare_port! { pub struct LinuxPort;
    VirtualMemory = Linux,
    Clock = Linux,
    Scheduler = Linux,
    Events = Linux,
    Mutexes = Linux,
    Threads = Linux,
    ThreadLocal = Linux,
    StackBounds = Linux,
    ProcessBarrier = Linux,
    Environment = Linux,
    Identity = Linux,
    Realtime = Linux,
    Entropy = Linux,
    NativeMapping = Linux,
    Modules = Linux,
    NativeHeap = Linux,
    RwLocks = Linux,
    ThreadName = Linux,
    Diagnostics = Linux,
    Context = Linux,
    Topology = Linux,
    Process = Linux,
    Image = Linux,
    Streams = Linux,
    Files = Linux,
    Sockets = Linux,
    SystemInfo = Linux,
    Notifications = Linux,
    Processes = Linux,
    Terminal = Linux,
    Watches = Linux,
    Mappings = Linux,
    Volumes = Linux,
    Network = Linux,
    LocalSockets = Linux,
    Accounts = Linux,
    Priority = Linux,
    Packets = Linux,
    SpawnAs = Linux,
    Abort = Linux
}
pub fn api() -> *const dotnet_pal_rs::Api {
    static TABLE: dotnet_pal_rs::Slot = dotnet_pal_rs::Slot::new();
    dotnet_pal_rs::negotiate::<LinuxPort>(&TABLE, dotnet_pal_rs::ABI_VERSION)
}
#[cfg(feature = "entry")]
#[no_mangle]
pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const dotnet_pal_rs::Api {
    static TABLE: dotnet_pal_rs::Slot = dotnet_pal_rs::Slot::new();
    dotnet_pal_rs::negotiate::<LinuxPort>(&TABLE, version)
}
#[cfg(all(feature = "entry", not(test)))]
dotnet_pal_rs::define_panic_handler!(Linux);
