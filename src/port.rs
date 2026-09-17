//! Port surface: the traits a platform implements to supply OS services.
//!
//! A port is a type implementing [`Port`]. Every associated type names the
//! provider of one capability, or [`Absent`] when the platform cannot honestly
//! provide it. Absent capabilities are reported to the runtime as clear
//! capability bits and NULL callbacks, never as stubs that return success.
//!
//! The front ends in this crate validate arguments, sanitize outputs and count
//! calls before and after invoking a provider, so a provider only implements the
//! platform operation itself. Provider functions receive raw pointers exactly as
//! the C ABI passes them: they are trusted FFI borrows, not sandboxed addresses.
//!
//! Nothing here allocates, unwinds or requires `std`. A port that needs the Rust
//! standard library lives in its own crate (see `dotnet-pal-std`), which keeps
//! `no_std` consumers of this crate free of that dependency.
use core::ffi::c_void;

/// Failure statuses of the C ABI, without `OK`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Error {
    Unsupported = crate::UNSUPPORTED,
    InvalidArgument = crate::INVALID_ARGUMENT,
    Os = crate::OS_ERROR,
    OutOfMemory = crate::OUT_OF_MEMORY,
    Timeout = crate::kernel::TIMEOUT,
    Busy = crate::kernel::BUSY,
    BufferTooSmall = crate::runtime::BUFFER_TOO_SMALL,
    NotFound = crate::runtime::NOT_FOUND,
}
impl Error {
    pub const fn status(self) -> u32 { self as u32 }
    /// Maps a foreign status code; values outside the published set become `Os`.
    pub const fn from_status(status: u32) -> Option<Self> {
        Some(match status {
            crate::OK => return None,
            crate::UNSUPPORTED => Self::Unsupported,
            crate::INVALID_ARGUMENT => Self::InvalidArgument,
            crate::OUT_OF_MEMORY => Self::OutOfMemory,
            crate::kernel::TIMEOUT => Self::Timeout,
            crate::kernel::BUSY => Self::Busy,
            crate::runtime::BUFFER_TOO_SMALL => Self::BufferTooSmall,
            crate::runtime::NOT_FOUND => Self::NotFound,
            _ => Self::Os,
        })
    }
}
pub type Result<T> = core::result::Result<T, Error>;
/// Converts a provider result into an ABI status.
pub fn status<T>(result: Result<T>) -> u32 {
    match result { Ok(_) => crate::OK, Err(e) => e.status() }
}
/// Converts a foreign ABI status into a provider result.
pub fn from_status(code: u32) -> Result<()> {
    match Error::from_status(code) { None => Ok(()), Some(e) => Err(e) }
}

/// Marker for a capability the port does not provide.
pub struct Absent;

/// Sparse virtual memory with inaccessible reservations (`CAP_VM`).
pub trait VirtualMemory {
    const PROVIDED: bool = true;
    fn page_size() -> usize;
    /// `size` is page-rounded and `alignment` is a power of two at least one page.
    unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void>;
    unsafe fn commit(address: *mut c_void, size: usize) -> Result<()>;
    unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()>;
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()>;
    unsafe fn reset(address: *mut c_void, size: usize) -> Result<()>;
}
/// Eager, always-accessible storage in fixed units (`CAP_LINEAR`).
pub trait LinearStorage {
    const PROVIDED: bool = true;
    /// Advertise `CAP_DYNAMIC_LINEAR`: storage is obtained on demand.
    const DYNAMIC: bool = false;
    const GRANULARITY: usize;
    const CAPACITY: usize;
    /// `size` is a multiple of the granularity; `alignment` a power of two at least that large.
    unsafe fn allocate(size: usize, alignment: usize) -> Result<*mut c_void>;
    unsafe fn zero(address: *mut c_void, size: usize) -> Result<()>;
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()>;
}
/// Monotonic clock (`CAP_CLOCK`). Epoch unspecified; readings never decrease.
pub trait Clock {
    const PROVIDED: bool = true;
    fn monotonic_ns() -> Result<u64>;
}
/// Blocking relative sleep and a scheduling hint (`CAP_SCHEDULER`).
pub trait Scheduler {
    const PROVIDED: bool = true;
    fn sleep_ns(nanoseconds: u64) -> Result<()>;
    fn yield_now() -> Result<()>;
}
/// Manual/auto-reset events with monotonic timed waits (`CAP_EVENTS`).
pub trait Events {
    const PROVIDED: bool = true;
    unsafe fn create(manual_reset: bool, initially_set: bool) -> Result<*mut c_void>;
    unsafe fn destroy(handle: *mut c_void) -> Result<()>;
    unsafe fn set(handle: *mut c_void) -> Result<()>;
    unsafe fn reset(handle: *mut c_void) -> Result<()>;
    /// `timeout_ns` of `INFINITE` waits forever; zero polls. `Err(Timeout)` on expiry.
    unsafe fn wait(handle: *mut c_void, timeout_ns: u64) -> Result<()>;
}
/// Error-checking or recursive mutexes (`CAP_MUTEX`).
pub trait Mutexes {
    const PROVIDED: bool = true;
    unsafe fn create(recursive: bool) -> Result<*mut c_void>;
    unsafe fn destroy(handle: *mut c_void) -> Result<()>;
    unsafe fn lock(handle: *mut c_void) -> Result<()>;
    unsafe fn unlock(handle: *mut c_void) -> Result<()>;
}
/// Native threads (`CAP_THREADS`). Handles are joined or detached exactly once.
pub trait Threads {
    const PROVIDED: bool = true;
    unsafe fn create(entry: crate::kernel::Entry, argument: *mut c_void, stack_size: usize) -> Result<*mut c_void>;
    unsafe fn join(handle: *mut c_void) -> Result<()>;
    unsafe fn detach(handle: *mut c_void) -> Result<()>;
}
/// Dynamic thread-local slots with optional destructors (`CAP_TLS`).
pub trait ThreadLocal {
    const PROVIDED: bool = true;
    unsafe fn create(destructor: Option<crate::kernel::Destructor>) -> Result<*mut c_void>;
    unsafe fn destroy(handle: *mut c_void) -> Result<()>;
    unsafe fn get(handle: *mut c_void) -> Result<*mut c_void>;
    unsafe fn set(handle: *mut c_void, value: *mut c_void) -> Result<()>;
}
/// Bounds of the current native thread's stack (`CAP_STACK`): `(low, high)`.
pub trait StackBounds {
    const PROVIDED: bool = true;
    fn current() -> Result<(*mut c_void, *mut c_void)>;
}
/// Process-wide memory barrier (`CAP_PROCESS_BARRIER`), not a local fence.
pub trait ProcessBarrier {
    const PROVIDED: bool = true;
    /// Probed once at negotiation; `false` clears the capability bit.
    fn available() -> bool { true }
    fn barrier() -> Result<()>;
}
/// Result of an environment lookup that found the variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The value plus its terminating NUL was copied; `usize` is that length.
    Copied(usize),
    /// The buffer was too small; `usize` is the length needed including NUL.
    TooSmall(usize),
}
/// Environment snapshot (`CAP_ENVIRONMENT`).
pub trait Environment {
    const PROVIDED: bool = true;
    /// `name` contains neither NUL nor `=`. `Err(NotFound)` when the variable is absent.
    unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup>;
}
/// Process and thread identity (`CAP_IDENTITY`). Identities are nonzero.
pub trait Identity {
    const PROVIDED: bool = true;
    fn process_id() -> Result<u64>;
    fn thread_id() -> Result<u64>;
}
/// Wall clock in nanoseconds since the Unix epoch (`CAP_REALTIME`).
pub trait Realtime {
    const PROVIDED: bool = true;
    fn realtime_ns() -> Result<u64>;
}
/// Cryptographic entropy (`CAP_ENTROPY`). On error the front end zeroes the buffer.
pub trait Entropy {
    const PROVIDED: bool = true;
    unsafe fn fill(out: *mut u8, size: usize) -> Result<()>;
}
/// Explicit native mappings with protection (`CAP_NATIVE_MEMORY`).
pub trait NativeMapping {
    const PROVIDED: bool = true;
    fn page_size() -> usize;
    unsafe fn allocate(size: usize, protection: u32) -> Result<*mut c_void>;
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()>;
    unsafe fn protect(address: *mut c_void, size: usize, protection: u32) -> Result<()>;
}
/// Module loading and inspection (`CAP_MODULES`).
pub trait Modules {
    const PROVIDED: bool = true;
    /// `None` names the current process image.
    unsafe fn open(name: Option<&[u8]>) -> Result<*mut c_void>;
    unsafe fn symbol(module: *mut c_void, name: &[u8]) -> Result<*mut c_void>;
    unsafe fn close(module: *mut c_void) -> Result<()>;
    unsafe fn info(address: *mut c_void) -> Result<crate::runtime::ModuleInfo>;
}
/// Native helper heap (`CAP_NATIVE_HEAP`), distinct from GC memory.
pub trait NativeHeap {
    const PROVIDED: bool = true;
    unsafe fn allocate(size: usize, zero: bool) -> Result<*mut c_void>;
    /// Preserves the old allocation on failure.
    unsafe fn resize(address: *mut c_void, size: usize) -> Result<*mut c_void>;
    unsafe fn release(address: *mut c_void) -> Result<()>;
}
/// Reader/writer locks (`CAP_RWLOCK`).
pub trait RwLocks {
    const PROVIDED: bool = true;
    unsafe fn create() -> Result<*mut c_void>;
    unsafe fn read(handle: *mut c_void) -> Result<()>;
    unsafe fn write(handle: *mut c_void) -> Result<()>;
    unsafe fn unlock(handle: *mut c_void) -> Result<()>;
    unsafe fn destroy(handle: *mut c_void) -> Result<()>;
}
/// Naming the current thread (`CAP_THREAD_NAME`). `name` has no NUL and at most 255 bytes.
pub trait ThreadName {
    const PROVIDED: bool = true;
    fn set(name: &[u8]) -> Result<()>;
}
/// Fatal diagnostic output (`CAP_DIAGNOSTICS`).
pub trait Diagnostics {
    const PROVIDED: bool = true;
    /// Writes everything or returns the validated progress with the error.
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)>;
}
/// Architecture-bound signal/activation substrate (`CAP_NATIVE_CONTEXT`).
/// The table is raw because register contexts are never portable.
pub trait SignalContext {
    const PROVIDED: bool = true;
    fn ops() -> Option<&'static crate::context::Ops>;
}
/// Raw WASIp1 transport (`CAP_WASI_DISPATCH`): one physical host import.
pub trait WasiTransport {
    const PROVIDED: bool = true;
    unsafe fn dispatch(request: *const crate::wasi::Request) -> u32;
}
/// Terminates the process or instance after an internal invariant failure.
pub trait Abort {
    fn abort() -> !;
}

macro_rules! absent {
    ($trait:ident { $($body:tt)* }) => {
        impl $trait for Absent { const PROVIDED: bool = false; $($body)* }
    };
}
absent!(VirtualMemory {
    fn page_size() -> usize { 0 }
    unsafe fn reserve(_: usize, _: usize) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn commit(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn decommit(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn release(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn reset(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
});
absent!(LinearStorage {
    const GRANULARITY: usize = 0;
    const CAPACITY: usize = 0;
    unsafe fn allocate(_: usize, _: usize) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn zero(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn release(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Clock { fn monotonic_ns() -> Result<u64> { Err(Error::Unsupported) } });
absent!(Scheduler {
    fn sleep_ns(_: u64) -> Result<()> { Err(Error::Unsupported) }
    fn yield_now() -> Result<()> { Err(Error::Unsupported) }
});
absent!(Events {
    unsafe fn create(_: bool, _: bool) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn destroy(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn set(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn reset(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn wait(_: *mut c_void, _: u64) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Mutexes {
    unsafe fn create(_: bool) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn destroy(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn lock(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn unlock(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Threads {
    unsafe fn create(_: crate::kernel::Entry, _: *mut c_void, _: usize) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn join(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn detach(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
});
absent!(ThreadLocal {
    unsafe fn create(_: Option<crate::kernel::Destructor>) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn destroy(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn get(_: *mut c_void) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn set(_: *mut c_void, _: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
});
absent!(StackBounds { fn current() -> Result<(*mut c_void, *mut c_void)> { Err(Error::Unsupported) } });
absent!(ProcessBarrier {
    fn available() -> bool { false }
    fn barrier() -> Result<()> { Err(Error::Unsupported) }
});
absent!(Environment { unsafe fn get(_: &[u8], _: *mut u8, _: usize) -> Result<Lookup> { Err(Error::Unsupported) } });
absent!(Identity {
    fn process_id() -> Result<u64> { Err(Error::Unsupported) }
    fn thread_id() -> Result<u64> { Err(Error::Unsupported) }
});
absent!(Realtime { fn realtime_ns() -> Result<u64> { Err(Error::Unsupported) } });
absent!(Entropy { unsafe fn fill(_: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) } });
absent!(NativeMapping {
    fn page_size() -> usize { 0 }
    unsafe fn allocate(_: usize, _: u32) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn release(_: *mut c_void, _: usize) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn protect(_: *mut c_void, _: usize, _: u32) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Modules {
    unsafe fn open(_: Option<&[u8]>) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn symbol(_: *mut c_void, _: &[u8]) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn close(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn info(_: *mut c_void) -> Result<crate::runtime::ModuleInfo> { Err(Error::Unsupported) }
});
absent!(NativeHeap {
    unsafe fn allocate(_: usize, _: bool) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn resize(_: *mut c_void, _: usize) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn release(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
});
absent!(RwLocks {
    unsafe fn create() -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn read(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn write(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn unlock(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn destroy(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
});
absent!(ThreadName { fn set(_: &[u8]) -> Result<()> { Err(Error::Unsupported) } });
absent!(Diagnostics {
    unsafe fn write_stderr(_: *const u8, _: usize) -> core::result::Result<(), (usize, Error)> { Err((0, Error::Unsupported)) }
});
absent!(SignalContext { fn ops() -> Option<&'static crate::context::Ops> { None } });
absent!(WasiTransport { unsafe fn dispatch(_: *const crate::wasi::Request) -> u32 { 52 } });

/// A platform port: one provider type per capability, `Absent` where none exists.
///
/// `validate` runs once at negotiation. Returning `false` rejects the whole table
/// (the runtime receives NULL), which is how a port reports a malformed required
/// host configuration rather than continuing with pretend services.
pub trait Port {
    type VirtualMemory: VirtualMemory;
    type Linear: LinearStorage;
    type Clock: Clock;
    type Scheduler: Scheduler;
    type Events: Events;
    type Mutexes: Mutexes;
    type Threads: Threads;
    type ThreadLocal: ThreadLocal;
    type StackBounds: StackBounds;
    type ProcessBarrier: ProcessBarrier;
    type Environment: Environment;
    type Identity: Identity;
    type Realtime: Realtime;
    type Entropy: Entropy;
    type NativeMapping: NativeMapping;
    type Modules: Modules;
    type NativeHeap: NativeHeap;
    type RwLocks: RwLocks;
    type ThreadName: ThreadName;
    type Diagnostics: Diagnostics;
    type Context: SignalContext;
    type Wasi: WasiTransport;
    type Abort: Abort;
    fn validate() -> bool { true }
}

/// Declares a port from `Capability = Provider` pairs and exports
/// `dotnet_pal_get_api` for it. Unlisted capabilities are `Absent`.
///
/// ```ignore
/// dotnet_pal_rs::define_pal! {
///     Linear = MyArena,
///     Clock = MyClock,
///     Abort = MyAbort,
/// }
/// ```
#[macro_export]
macro_rules! define_pal {
    ($($key:ident = $provider:ty),* $(,)?) => {
        $crate::declare_port! { pub struct DotnetPalPort; $($key = $provider),* }
        static DOTNET_PAL_TABLE: $crate::Slot = $crate::Slot::new();
        /// The only runtime-facing PAL entry point. Valid before managed runtime startup.
        #[no_mangle]
        pub extern "C" fn dotnet_pal_get_api(version: u32) -> *const $crate::Api {
            $crate::negotiate::<DotnetPalPort>(&DOTNET_PAL_TABLE, version)
        }
    };
}
/// Declares a port type without exporting the entry point.
#[macro_export]
macro_rules! declare_port {
    ($vis:vis struct $name:ident; $($key:ident = $provider:ty),* $(,)?) => {
        $vis struct $name;
        impl $crate::port::Port for $name {
            type VirtualMemory = $crate::__port_lookup!(VirtualMemory; $($key = $provider),*);
            type Linear = $crate::__port_lookup!(Linear; $($key = $provider),*);
            type Clock = $crate::__port_lookup!(Clock; $($key = $provider),*);
            type Scheduler = $crate::__port_lookup!(Scheduler; $($key = $provider),*);
            type Events = $crate::__port_lookup!(Events; $($key = $provider),*);
            type Mutexes = $crate::__port_lookup!(Mutexes; $($key = $provider),*);
            type Threads = $crate::__port_lookup!(Threads; $($key = $provider),*);
            type ThreadLocal = $crate::__port_lookup!(ThreadLocal; $($key = $provider),*);
            type StackBounds = $crate::__port_lookup!(StackBounds; $($key = $provider),*);
            type ProcessBarrier = $crate::__port_lookup!(ProcessBarrier; $($key = $provider),*);
            type Environment = $crate::__port_lookup!(Environment; $($key = $provider),*);
            type Identity = $crate::__port_lookup!(Identity; $($key = $provider),*);
            type Realtime = $crate::__port_lookup!(Realtime; $($key = $provider),*);
            type Entropy = $crate::__port_lookup!(Entropy; $($key = $provider),*);
            type NativeMapping = $crate::__port_lookup!(NativeMapping; $($key = $provider),*);
            type Modules = $crate::__port_lookup!(Modules; $($key = $provider),*);
            type NativeHeap = $crate::__port_lookup!(NativeHeap; $($key = $provider),*);
            type RwLocks = $crate::__port_lookup!(RwLocks; $($key = $provider),*);
            type ThreadName = $crate::__port_lookup!(ThreadName; $($key = $provider),*);
            type Diagnostics = $crate::__port_lookup!(Diagnostics; $($key = $provider),*);
            type Context = $crate::__port_lookup!(Context; $($key = $provider),*);
            type Wasi = $crate::__port_lookup!(Wasi; $($key = $provider),*);
            type Abort = $crate::__port_lookup!(Abort; $($key = $provider),*);
        }
    };
}
/// Internal: finds `$want = Type` in a key list, defaulting to `Absent`.
#[doc(hidden)]
#[macro_export]
macro_rules! __port_lookup {
    (VirtualMemory; VirtualMemory = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Linear; Linear = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Clock; Clock = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Scheduler; Scheduler = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Events; Events = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Mutexes; Mutexes = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Threads; Threads = $t:ty $(, $($rest:tt)*)?) => { $t };
    (ThreadLocal; ThreadLocal = $t:ty $(, $($rest:tt)*)?) => { $t };
    (StackBounds; StackBounds = $t:ty $(, $($rest:tt)*)?) => { $t };
    (ProcessBarrier; ProcessBarrier = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Environment; Environment = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Identity; Identity = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Realtime; Realtime = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Entropy; Entropy = $t:ty $(, $($rest:tt)*)?) => { $t };
    (NativeMapping; NativeMapping = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Modules; Modules = $t:ty $(, $($rest:tt)*)?) => { $t };
    (NativeHeap; NativeHeap = $t:ty $(, $($rest:tt)*)?) => { $t };
    (RwLocks; RwLocks = $t:ty $(, $($rest:tt)*)?) => { $t };
    (ThreadName; ThreadName = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Diagnostics; Diagnostics = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Context; Context = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Wasi; Wasi = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Abort; Abort = $t:ty $(, $($rest:tt)*)?) => { $t };
    ($want:ident; $other:ident = $t:ty $(, $($rest:tt)*)?) => { $crate::__port_lookup!($want; $($($rest)*)?) };
    (Abort;) => { $crate::port::Trap };
    ($want:ident;) => { $crate::port::Absent };
}
/// Default abort for ports that do not name one: a trap instruction or an
/// endless spin, never a return into the caller.
pub struct Trap;
impl Abort for Trap {
    fn abort() -> ! {
        #[cfg(target_arch = "wasm32")]
        core::arch::wasm32::unreachable();
        #[cfg(not(target_arch = "wasm32"))]
        loop { core::hint::spin_loop(); }
    }
}
/// Emits the `no_std` panic handler for a port. Use only in a final `no_std`
/// artifact; a program linked with `std` already has one.
#[macro_export]
macro_rules! define_panic_handler {
    ($abort:ty) => {
        #[panic_handler]
        fn dotnet_pal_panic(_: &core::panic::PanicInfo<'_>) -> ! { <$abort as $crate::port::Abort>::abort() }
    };
}
