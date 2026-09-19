#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
//! A bare-metal AArch64 port of the dotnet-pal-rs boundary.
//!
//! The target is `qemu-system-aarch64 -M virt -m 1024` with no operating system,
//! no libc and no firmware beyond QEMU's own `-kernel` loader. This crate is the
//! whole platform: it boots the core, maps memory, drives the serial port, runs
//! threads and then answers the boundary's C table on top of that.
//!
//! What the port provides:
//!
//! | Capability | Provider |
//! | --- | --- |
//! | `VirtualMemory`, `NativeMapping` | ranges carved from the RAM behind the image |
//! | `NativeHeap` | a first-fit heap in a 64 MiB slice of that RAM |
//! | `Clock`, `Realtime`, `Scheduler` | the architected generic timer, plus a fixed epoch |
//! | `Events`, `Mutexes`, `Threads`, `ThreadLocal`, `RwLocks` | the cooperative scheduler in [`thread`] and [`objects`] |
//! | `StackBounds`, `Identity`, `ThreadName` | the thread table |
//! | `ProcessBarrier` | `dsb ish`, which is a full barrier on this one core |
//! | `Diagnostics`, `Streams` | the PL011 UART at 0x0900_0000 (no input: reads report end of input) |
//! | `Topology`, `Process`, `Image`, `Modules` | one CPU, the RAM figures, exit through semihosting, the image's own unwind tables ([`platform`]) |
//! | `Faults` | the synchronous exception vector: the interrupted registers go to the installed handler, which may resume them ([`fault`]) |
//! | `Files`, `Volumes`, `Watches`, `Mappings` | an in-memory file system (`dotnet-pal-memfs`) over the port's heap |
//! | `SystemInfo` | the machine's name and uptime, the image's path; no environment, no user, no process accounting |
//!
//! What it does not provide, and says so through absent capabilities: an
//! environment, entropy, sockets (the machine has no network device), child
//! processes, notifications (nothing reaches the machine from outside), terminal
//! control, a signal/context substrate and the WASI transport.
//!
//! Boot order is `_start` (src/boot.S) to [`pal_rust_start`]: the region of free
//! RAM is published, the boot thread gets its ELF TLS block, `.init_array` runs
//! the C++ static constructors, and `main` is called with `argv = {"app"}`. When
//! `main` returns, its status leaves the machine through semihosting.
use core::ptr;
use dotnet_pal_rs::port::{self, Error};

core::arch::global_asm!(include_str!("boot.S"));
core::arch::global_asm!(include_str!("switch.S"));

pub mod clock;
pub mod exit;
pub mod fault;
pub mod memory;
pub mod objects;
pub mod platform;
mod sync;
pub mod thread;
pub mod tls;
pub mod uart;

/// The port. Every provider trait is implemented on this type.
pub struct Baremetal;

impl port::Diagnostics for Baremetal {
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
        // SAFETY: the front end has checked the pointer, the size and the sum.
        uart::write(unsafe { core::slice::from_raw_parts(data, size) });
        Ok(())
    }
}
impl port::ProcessBarrier for Baremetal {
    /// One core with no other observers: a data barrier orders every access this
    /// machine can make, so this really is the process-wide barrier and not a
    /// local fence passed off as one.
    fn barrier() -> port::Result<()> {
        // SAFETY: a barrier instruction has no operands and no side effects.
        unsafe { core::arch::asm!("dsb ish", options(nostack)) };
        Ok(())
    }
}

/// Serial-port tracing of the boundary's own activity, only with the `trace` feature.
pub fn trace(label: &[u8], value: u64) {
    #[cfg(feature = "trace")]
    {
        uart::write(b"[pal] ");
        uart::write(label);
        uart::write(b" ");
        uart::write_hex(value);
        uart::write(b"\n");
    }
    #[cfg(not(feature = "trace"))]
    { let _ = (label, value); }
}
dotnet_pal_rs::declare_port! {
    pub struct DotnetPalPort;
    VirtualMemory = Baremetal,
    NativeMapping = Baremetal,
    NativeHeap = Baremetal,
    Clock = Baremetal,
    Realtime = Baremetal,
    Scheduler = Baremetal,
    Events = Baremetal,
    Mutexes = Baremetal,
    Threads = Baremetal,
    ThreadLocal = Baremetal,
    StackBounds = Baremetal,
    Identity = Baremetal,
    ThreadName = Baremetal,
    RwLocks = Baremetal,
    ProcessBarrier = Baremetal,
    Diagnostics = Baremetal,
    Topology = Baremetal,
    Process = Baremetal,
    Image = Baremetal,
    Streams = Baremetal,
    Modules = Baremetal,
    Faults = Baremetal,
    Files = dotnet_pal_memfs::MemFs, Volumes = dotnet_pal_memfs::MemFs, Watches = dotnet_pal_memfs::MemFs, Mappings = dotnet_pal_memfs::MemFs,
    SystemInfo = Baremetal,
    Abort = exit::Abort,
}
static DOTNET_PAL_TABLE: dotnet_pal_rs::Slot = dotnet_pal_rs::Slot::new();
/// The only runtime-facing PAL entry point. Valid before managed runtime startup.
#[no_mangle]
pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const dotnet_pal_rs::Api {
    let api = dotnet_pal_rs::negotiate::<DotnetPalPort>(&DOTNET_PAL_TABLE, version);
    #[cfg(feature = "trace")]
    {
        use core::sync::atomic::{AtomicBool, Ordering};
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) {
            // SAFETY: a non-null table is the immutable negotiated table.
            trace(b"negotiated capabilities", if api.is_null() { 0 } else { unsafe { (*api).header.capabilities } });
        }
    }
    api
}
dotnet_pal_rs::define_panic_handler!(exit::Abort);

/// Writes to the machine's serial port.
pub fn console_write(bytes: &[u8]) {
    uart::write(bytes);
}
/// Always `None`: nothing is wired to the serial input.
pub fn console_read() -> Option<u8> {
    uart::read()
}
/// Ends the run with `code` through semihosting.
pub fn exit(code: i32) -> ! {
    exit::exit(code)
}

/// The console and exit services, for C code linked against this archive.
#[no_mangle]
pub extern "C" fn pal_console_write(data: *const u8, size: usize) {
    if data.is_null() || size == 0 {
        return;
    }
    // SAFETY: a C caller passing a readable range is the contract here, the same
    // one every other pointer argument at this boundary has.
    uart::write(unsafe { core::slice::from_raw_parts(data, size) });
}
#[no_mangle]
pub extern "C" fn pal_console_read() -> i32 {
    match uart::read() {
        Some(byte) => byte as i32,
        None => -1,
    }
}
#[no_mangle]
pub extern "C" fn pal_exit(code: i32) -> ! {
    exit::exit(code)
}
/// The static RAM region, for the report the test prints: its first and last
/// address and the bytes in it that are still free.
#[no_mangle]
pub extern "C" fn pal_region_start() -> usize {
    memory::bounds().0
}
#[no_mangle]
pub extern "C" fn pal_region_end() -> usize {
    memory::bounds().1
}
#[no_mangle]
pub extern "C" fn pal_region_free() -> usize {
    memory::available()
}

extern "C" {
    static __init_array_start: u8;
    static __init_array_end: u8;
    fn main(argc: i32, argv: *const *const u8) -> i32;
}
type Constructor = unsafe extern "C" fn();

/// Entered from the boot code with the MMU, caches, FP/SIMD and the exception
/// vectors already set up, on the boot stack from the linker script.
#[no_mangle]
pub extern "C" fn pal_rust_start() -> ! {
    memory::init();
    thread::init_boot_thread();
    // File times come from the same clock as `Realtime`.
    dotnet_pal_memfs::set_clock(|| <Baremetal as port::Realtime>::realtime_ns().unwrap_or(0));
    // A reader that waits for a change, or a lock that waits for its turn, lets the other threads run meanwhile.
    dotnet_pal_memfs::set_wait(|nanoseconds| { let _ = <Baremetal as port::Scheduler>::sleep_ns(nanoseconds); });
    dotnet_pal_memfs::set_yield(|| { let _ = <Baremetal as port::Scheduler>::yield_now(); });
    // SAFETY: the linker script brackets .init_array with these symbols and every
    // entry in it is a function the C++ compiler put there.
    unsafe {
        let mut entry = ptr::addr_of!(__init_array_start) as usize;
        let end = ptr::addr_of!(__init_array_end) as usize;
        while entry < end {
            (entry as *const Constructor).read()();
            entry += core::mem::size_of::<Constructor>();
        }
    }
    let argv: [*const u8; 2] = [b"app\0".as_ptr(), ptr::null()];
    trace(b"calling main", 0);
    // SAFETY: `main` is the C entry point linked into this image.
    let status = unsafe { main(1, argv.as_ptr()) };
    exit::exit(status)
}
