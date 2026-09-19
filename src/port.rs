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
    // I/O conditions of the files and sockets groups; older groups never report them.
    AlreadyExists = crate::io::ALREADY_EXISTS,
    AccessDenied = crate::io::ACCESS_DENIED,
    IsDirectory = crate::io::IS_DIRECTORY,
    NotDirectory = crate::io::NOT_DIRECTORY,
    NotEmpty = crate::io::NOT_EMPTY,
    NoSpace = crate::io::NO_SPACE,
    WouldBlock = crate::io::WOULD_BLOCK,
    BrokenPipe = crate::io::BROKEN_PIPE,
    ConnectionRefused = crate::io::CONNECTION_REFUSED,
    ConnectionReset = crate::io::CONNECTION_RESET,
    ConnectionAborted = crate::io::CONNECTION_ABORTED,
    NotConnected = crate::io::NOT_CONNECTED,
    AlreadyConnected = crate::io::ALREADY_CONNECTED,
    AddressInUse = crate::io::ADDRESS_IN_USE,
    AddressNotAvailable = crate::io::ADDRESS_NOT_AVAILABLE,
    NetworkUnreachable = crate::io::NETWORK_UNREACHABLE,
    HostUnreachable = crate::io::HOST_UNREACHABLE,
    InProgress = crate::io::IN_PROGRESS,
    TooManyHandles = crate::io::TOO_MANY_HANDLES,
    NameTooLong = crate::io::NAME_TOO_LONG,
    ReadOnly = crate::io::READ_ONLY,
    CrossDevice = crate::io::CROSS_DEVICE,
    MessageTooLarge = crate::io::MESSAGE_TOO_LARGE,
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
            crate::io::ALREADY_EXISTS => Self::AlreadyExists,
            crate::io::ACCESS_DENIED => Self::AccessDenied,
            crate::io::IS_DIRECTORY => Self::IsDirectory,
            crate::io::NOT_DIRECTORY => Self::NotDirectory,
            crate::io::NOT_EMPTY => Self::NotEmpty,
            crate::io::NO_SPACE => Self::NoSpace,
            crate::io::WOULD_BLOCK => Self::WouldBlock,
            crate::io::BROKEN_PIPE => Self::BrokenPipe,
            crate::io::CONNECTION_REFUSED => Self::ConnectionRefused,
            crate::io::CONNECTION_RESET => Self::ConnectionReset,
            crate::io::CONNECTION_ABORTED => Self::ConnectionAborted,
            crate::io::NOT_CONNECTED => Self::NotConnected,
            crate::io::ALREADY_CONNECTED => Self::AlreadyConnected,
            crate::io::ADDRESS_IN_USE => Self::AddressInUse,
            crate::io::ADDRESS_NOT_AVAILABLE => Self::AddressNotAvailable,
            crate::io::NETWORK_UNREACHABLE => Self::NetworkUnreachable,
            crate::io::HOST_UNREACHABLE => Self::HostUnreachable,
            crate::io::IN_PROGRESS => Self::InProgress,
            crate::io::TOO_MANY_HANDLES => Self::TooManyHandles,
            crate::io::NAME_TOO_LONG => Self::NameTooLong,
            crate::io::READ_ONLY => Self::ReadOnly,
            crate::io::CROSS_DEVICE => Self::CrossDevice,
            crate::io::MESSAGE_TOO_LARGE => Self::MessageTooLarge,
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
/// Machine topology and memory accounting (`CAP_TOPOLOGY`). A method the target
/// cannot answer returns `Err(Unsupported)`; the capability stays whole.
pub trait Topology {
    const PROVIDED: bool = true;
    /// Highest possible logical CPU count of the machine (nonzero).
    fn cpu_max() -> Result<u32>;
    /// Logical CPUs this process may run on after affinity and quota limits (nonzero).
    fn cpu_count() -> Result<u32>;
    /// The CPU executing the caller right now.
    fn current_cpu() -> Result<u32>;
    /// Writes the process affinity bitmap (bit n = CPU n) into `capacity` bytes and
    /// returns the number of bytes the whole bitmap needs.
    unsafe fn process_affinity(mask: *mut u8, capacity: usize) -> Result<usize>;
    fn set_thread_affinity(cpu: u32) -> Result<()>;
    /// `(total, available)` bytes within the limit in force for the process.
    fn physical_memory() -> Result<(u64, u64)>;
    /// Container or job limit on physical memory; 0 when none.
    fn memory_limit() -> Result<u64>;
    /// Address-space limit; 0 when none.
    fn virtual_limit() -> Result<u64>;
    /// Largest per-CPU data cache in bytes; 0 when unknown.
    fn cache_size() -> Result<usize>;
    /// Two target-defined CPU feature words (Linux arm64: `AT_HWCAP`, `AT_HWCAP2`).
    fn cpu_features() -> Result<(u64, u64)>;
}
/// Process lifetime services (`CAP_PROCESS`).
pub trait Process {
    const PROVIDED: bool = true;
    /// Orderly termination with a status.
    fn exit(code: i32) -> !;
    fn debugger_present() -> Result<bool>;
    /// Runs `argv` (NUL-terminated strings) as a crash-dump utility allowed to
    /// inspect this process and waits for it. May write a NUL-terminated message
    /// of at most `capacity` bytes to `error` on failure.
    unsafe fn crash_dump(argv: &[*const u8], error: *mut u8, capacity: usize) -> Result<()>;
}
/// Executable image inspection (`CAP_IMAGE`).
pub trait Image {
    const PROVIDED: bool = true;
    /// The image containing `address` and its DWARF unwind tables; `Err(NotFound)` when none.
    unsafe fn unwind_info(address: usize) -> Result<crate::image::UnwindInfo>;
    /// Whether `size` bytes at `address` can be read without faulting.
    unsafe fn readable(address: usize, size: usize) -> Result<bool>;
    /// Copies the build identifier of the image at `base` and returns its full length;
    /// `Err(NotFound)` when the image has none.
    unsafe fn build_id(base: usize, out: *mut u8, capacity: usize) -> Result<usize>;
}
/// Standard streams (`CAP_STREAMS`): 0 input, 1 output, 2 error output.
pub trait Streams {
    const PROVIDED: bool = true;
    /// Writes to stream 1 or 2; returns the bytes accepted (at least one).
    unsafe fn write(stream: u32, data: *const u8, size: usize) -> Result<usize>;
    /// Reads from stream 0; `Ok(0)` is end of input.
    unsafe fn read(stream: u32, out: *mut u8, capacity: usize) -> Result<usize>;
    fn is_terminal(stream: u32) -> Result<bool>;
}
/// Files and directories (`CAP_FILES`). Paths are non-empty byte strings without
/// NUL, `/`-separated and passed verbatim. File handles carry no position: the
/// consumer names the offset on every transfer. Closing consumes the handle even
/// when it reports an error.
pub trait Files {
    const PROVIDED: bool = true;
    /// `flags` is a checked combination of `files::READ`, `WRITE`, `CREATE`,
    /// `EXCLUSIVE` and `TRUNCATE`; `mode` applies to a file this call creates.
    unsafe fn open(path: &[u8], flags: u32, mode: u32) -> Result<*mut c_void>;
    unsafe fn close(file: *mut c_void) -> Result<()>;
    /// `Ok(0)` is end of file.
    unsafe fn read_at(file: *mut c_void, offset: u64, out: *mut u8, capacity: usize) -> Result<usize>;
    /// Returns the bytes accepted (at least one); extends the file past its end.
    unsafe fn write_at(file: *mut c_void, offset: u64, data: *const u8, size: usize) -> Result<usize>;
    unsafe fn set_size(file: *mut c_void, size: u64) -> Result<()>;
    /// Makes accepted writes durable as far as the target can.
    unsafe fn flush(file: *mut c_void) -> Result<()>;
    unsafe fn status(file: *mut c_void) -> Result<crate::files::Status>;
    /// `follow` resolves a final symbolic link instead of describing it.
    unsafe fn path_status(path: &[u8], follow: bool) -> Result<crate::files::Status>;
    /// Deletes a non-directory.
    unsafe fn remove(path: &[u8]) -> Result<()>;
    /// Replaces an existing destination file.
    unsafe fn rename(from: &[u8], to: &[u8]) -> Result<()>;
    unsafe fn create_directory(path: &[u8], mode: u32) -> Result<()>;
    /// `Err(NotEmpty)` when the directory has entries.
    unsafe fn remove_directory(path: &[u8]) -> Result<()>;
    unsafe fn open_directory(path: &[u8]) -> Result<*mut c_void>;
    /// Writes the next entry name into `capacity` (at least `files::MAX_ENTRY_NAME`)
    /// bytes and returns its length and node kind; `Err(NotFound)` after the last
    /// entry. Never reports `.` or `..`.
    unsafe fn read_directory(directory: *mut c_void, name: *mut u8, capacity: usize) -> Result<(usize, u32)>;
    unsafe fn close_directory(directory: *mut c_void) -> Result<()>;
    /// Copies the working directory and its NUL when it fits; returns the length
    /// needed including the NUL either way.
    unsafe fn current_directory(out: *mut u8, capacity: usize) -> Result<usize>;

    // The operations below are optional: the default says the target has no such facility.
    /// Permission bits of a path (following a final link) or of an open file.
    unsafe fn set_mode(_path: &[u8], _mode: u32) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn set_file_mode(_file: *mut c_void, _mode: u32) -> Result<()> { Err(Error::Unsupported) }
    /// Access and modification times; `None` leaves that time unchanged.
    unsafe fn set_times(_path: &[u8], _follow: bool, _accessed_ns: Option<u64>, _modified_ns: Option<u64>) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn set_file_times(_file: *mut c_void, _accessed_ns: Option<u64>, _modified_ns: Option<u64>) -> Result<()> { Err(Error::Unsupported) }
    /// A second name for an existing file.
    unsafe fn link(_existing: &[u8], _created: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    /// A symbolic link at `created` whose target text is stored as given.
    unsafe fn symlink(_target: &[u8], _created: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    /// The target text of a symbolic link; `Err(InvalidArgument)` for a path that is not one.
    /// Same buffer contract as `current_directory`.
    unsafe fn read_link(_path: &[u8], _out: *mut u8, _capacity: usize) -> Result<usize> { Err(Error::Unsupported) }
    /// The absolute path with every link resolved. Same buffer contract as `current_directory`.
    unsafe fn real_path(_path: &[u8], _out: *mut u8, _capacity: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn set_current_directory(_path: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    /// Advisory lock on the whole file, held by the handle: `files::LOCK_SHARED`,
    /// `LOCK_EXCLUSIVE` or `LOCK_UNLOCK`. `Err(WouldBlock)` when `wait` is false and it is not free.
    unsafe fn lock(_file: *mut c_void, _mode: u32, _wait: bool) -> Result<()> { Err(Error::Unsupported) }
    /// The same for bytes `[offset, offset + length)`; never waits.
    unsafe fn lock_range(_file: *mut c_void, _offset: u64, _length: u64, _mode: u32) -> Result<()> { Err(Error::Unsupported) }
}
/// Internet sockets (`CAP_SOCKETS`): TCP streams and UDP datagrams. Addresses use
/// the boundary's own layout. Sockets start blocking.
pub trait Sockets {
    const PROVIDED: bool = true;
    unsafe fn create(family: u32, kind: u32) -> Result<*mut c_void>;
    unsafe fn close(socket: *mut c_void) -> Result<()>;
    unsafe fn bind(socket: *mut c_void, address: &crate::sockets::Address) -> Result<()>;
    unsafe fn listen(socket: *mut c_void, backlog: u32) -> Result<()>;
    /// The accepted socket and its peer. `Err(WouldBlock)` on a non-blocking socket with no connection pending.
    unsafe fn accept(socket: *mut c_void) -> Result<(*mut c_void, crate::sockets::Address)>;
    /// `Err(InProgress)` on a non-blocking socket; completion shows as writability and the `ERROR` option.
    unsafe fn connect(socket: *mut c_void, address: &crate::sockets::Address) -> Result<()>;
    /// Never raises a signal; a closed peer is `Err(BrokenPipe)`.
    unsafe fn send(socket: *mut c_void, data: *const u8, size: usize, to: Option<&crate::sockets::Address>) -> Result<usize>;
    /// `Ok((0, _))` on a stream is the peer's shutdown. The address is the sender when the target knows it.
    unsafe fn receive(socket: *mut c_void, out: *mut u8, capacity: usize, flags: u32) -> Result<(usize, Option<crate::sockets::Address>)>;
    unsafe fn shutdown(socket: *mut c_void, how: u32) -> Result<()>;
    unsafe fn local_address(socket: *mut c_void) -> Result<crate::sockets::Address>;
    unsafe fn peer_address(socket: *mut c_void) -> Result<crate::sockets::Address>;
    unsafe fn set_blocking(socket: *mut c_void, blocking: bool) -> Result<()>;
    unsafe fn get_option(socket: *mut c_void, option: u32) -> Result<u64>;
    unsafe fn set_option(socket: *mut c_void, option: u32, value: u64) -> Result<()>;
    /// Level-triggered readiness. Fills `triggered` of every entry; returns early
    /// (with nothing triggered) on timeout or after `wake` on its channel. At most
    /// one poll at a time names a given channel (below `sockets::POLL_CHANNELS`).
    unsafe fn poll(entries: &mut [crate::sockets::PollEntry], timeout_ns: u64, channel: Option<u32>) -> Result<()>;
    /// Interrupts the `poll` in progress on `channel`, or its next one when none
    /// is, and no other channel's. Callable from any thread.
    fn wake(channel: u32) -> Result<()>;
    /// Writes up to `capacity` addresses of `name`; returns how many. `Err(NotFound)` when it has none.
    unsafe fn resolve(name: &[u8], family: u32, out: *mut crate::sockets::Address, capacity: usize) -> Result<usize>;
    /// Same buffer contract as `Files::current_directory`.
    unsafe fn host_name(out: *mut u8, capacity: usize) -> Result<usize>;
}
/// Synchronous CPU fault reporting without POSIX signals (`CAP_FAULTS`).
///
/// The port owns the trap path (an exception vector, an exception port, a signal
/// it keeps for itself). Once `enable` has returned `Ok`, it builds a
/// [`crate::faults::Frame`] for every fault it takes and calls
/// [`crate::faults::deliver`]; on `Resume` it continues from the edited frame,
/// otherwise it ends the run as it would have without a handler.
pub trait Faults {
    const PROVIDED: bool = true;
    /// Arms the trap path. Called once, when the consumer installs its handler.
    fn enable() -> Result<()>;
}
/// Facts about the process and the machine (`CAP_SYSTEM`). Every question is
/// optional per call: `Err(Unsupported)` when the target cannot answer.
pub trait SystemInfo {
    const PROVIDED: bool = true;
    /// The `index`-th `NAME=value` text of the environment snapshot, with the buffer
    /// contract of `Files::current_directory`; `Err(NotFound)` past the last one.
    unsafe fn environment_entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize>;
    /// One text per `system::TEXT_*` selector, same buffer contract.
    unsafe fn text(what: u32, out: *mut u8, capacity: usize) -> Result<usize>;
    /// CPU time this process has consumed: `(user_ns, kernel_ns)`.
    fn process_times() -> Result<(u64, u64)>;
    /// Time since the machine started.
    fn uptime_ns() -> Result<u64>;
    /// Numeric identity of the user the process runs as and of its primary group.
    fn user_ids() -> Result<(u32, u32)>;
}
/// Requests from outside the process (`CAP_NOTIFICATIONS`): the interrupt key, a
/// request to terminate, a resized terminal window and the like.
///
/// The port owns the mechanism. For an enabled kind it calls
/// [`crate::notifications::deliver`] from a thread of its own, never from an
/// interrupt or signal context; a kind that is not enabled keeps the target's
/// default action.
pub trait Notifications {
    const PROVIDED: bool = true;
    /// Called once, when the consumer installs its handler.
    fn start() -> Result<()>;
    /// `Err(Unsupported)` for a kind the target does not have.
    fn enable(kind: u32) -> Result<()>;
    fn disable(kind: u32) -> Result<()>;
    /// Performs the target's default action for a kind the consumer chose not to handle.
    fn default_action(kind: u32) -> Result<()>;
}
/// Child processes and the pipes to their standard streams (`CAP_PROCESSES`).
pub trait Processes {
    const PROVIDED: bool = true;
    /// `arguments` and `environment` are NUL-terminated texts; `None` inherits the
    /// parent's environment. `pipes` is a combination of `processes::PIPE_*`.
    unsafe fn spawn(program: &[u8], arguments: &[*const u8], environment: Option<&[*const u8]>, directory: Option<&[u8]>, pipes: u32) -> Result<crate::processes::Spawned>;
    /// The exit code (128 plus the signal number for a child a signal ended);
    /// `Err(Timeout)` while it runs. May be called again after it has reported.
    unsafe fn wait(process: *mut c_void, timeout_ns: u64) -> Result<i32>;
    unsafe fn terminate(process: *mut c_void, forceful: bool) -> Result<()>;
    /// Gives the handle back; does not end the child.
    unsafe fn release(process: *mut c_void) -> Result<()>;
    /// `Ok(0)` once the other end is closed.
    unsafe fn pipe_read(pipe: *mut c_void, out: *mut u8, capacity: usize) -> Result<usize>;
    /// `Err(BrokenPipe)` once the other end is closed.
    unsafe fn pipe_write(pipe: *mut c_void, data: *const u8, size: usize) -> Result<usize>;
    unsafe fn pipe_close(pipe: *mut c_void) -> Result<()>;
}
/// The interactive terminal behind the standard streams (`CAP_TERMINAL`).
pub trait Terminal {
    const PROVIDED: bool = true;
    /// `(columns, rows)` of the terminal a stream is connected to; `Err(NotFound)` when it is none.
    fn window_size(stream: u32) -> Result<(u32, u32)>;
    /// Line mode (`raw` false) or raw mode, where a read returns once `min_bytes`
    /// have arrived or `timeout_ds` tenths of a second have passed.
    fn set_input_mode(raw: bool, min_bytes: u8, timeout_ds: u8, interrupt_as_input: bool) -> Result<()>;
    fn input_ready() -> Result<bool>;
    /// The byte of a `terminal::CONTROL_*` editing function; `Err(NotFound)` when the terminal has none.
    fn control_character(which: u32) -> Result<u8>;
}
/// Changes to files and directories, queued for a reader (`CAP_WATCHES`).
pub trait Watches {
    const PROVIDED: bool = true;
    fn open() -> Result<*mut c_void>;
    /// # Safety
    /// `watcher` came from `open`, no read is in progress and nothing uses it afterwards.
    unsafe fn close(watcher: *mut c_void) -> Result<()>;
    /// Watches `path` for `events`: `watches::ACCESS` to `DELETE`, with `ONLY_DIRECTORY`
    /// and `NO_FOLLOW`. The id is never 0, and a path the watcher already watches keeps its id.
    unsafe fn add(watcher: *mut c_void, path: &[u8], events: u32) -> Result<u32>;
    /// Ends a watch and queues `REMOVED` for it. May run beside a `read` that waits.
    unsafe fn remove(watcher: *mut c_void, watch: u32) -> Result<()>;
    /// Takes one event; `Err(Timeout)` when none came within `timeout_ns`
    /// (`watches::FOREVER`: without limit). One thread reads at a time.
    unsafe fn read(watcher: *mut c_void, timeout_ns: u64, event: &mut crate::watches::Event) -> Result<()>;
}
/// Files mapped into memory (`CAP_MAPPINGS`). `file` is a handle of [`Files`], so
/// the type that provides that trait provides this one.
pub trait Mappings {
    const PROVIDED: bool = true;
    /// Maps bytes `[offset, offset + length)`. `access` is `runtime::READ`, `WRITE` and
    /// `EXECUTE`; `shared` writes reach the file, the others stay in the mapping.
    unsafe fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8>;
    /// # Safety
    /// `address` and `length` are what one `map` returned and took, and nothing uses the range afterwards.
    unsafe fn unmap(address: *mut u8, length: usize) -> Result<()>;
    unsafe fn sync(address: *mut u8, length: usize) -> Result<()>;
}
/// Mounted volumes (`CAP_VOLUMES`).
pub trait Volumes {
    const PROVIDED: bool = true;
    /// The `index`-th mount point as a path, with the buffer contract of
    /// `Files::current_directory`; `Err(NotFound)` past the last one.
    unsafe fn entry(index: usize, out: *mut u8, capacity: usize) -> Result<usize>;
    /// Capacity, free space and format of the volume that holds `path`.
    fn status(path: &[u8]) -> Result<crate::volumes::Status>;
}
/// Network interfaces, reverse lookup and multicast membership (`CAP_NETWORK`).
/// `socket` is a handle of [`Sockets`], so the type that provides that trait provides this one.
pub trait Network {
    const PROVIDED: bool = true;
    /// The `index`-th interface; `Err(NotFound)` past the last one.
    fn interface_entry(index: usize) -> Result<crate::network::Interface>;
    /// The `index`-th address of any interface; `Err(NotFound)` past the last one.
    fn address_entry(index: usize) -> Result<crate::network::InterfaceAddress>;
    /// The host name of `address`, with the buffer contract of `Sockets::host_name`.
    unsafe fn reverse_lookup(address: &crate::sockets::Address, out: *mut u8, capacity: usize) -> Result<usize>;
    /// Joins or leaves a multicast group on a datagram socket; interface 0 is the target's choice.
    unsafe fn membership(socket: *mut c_void, group: &crate::sockets::Address, interface_index: u32, join: bool) -> Result<()>;
}
/// Unix domain sockets (`CAP_LOCAL_SOCKETS`). `socket` is a handle of [`Sockets`] created with
/// `sockets::LOCAL`, so the type that provides that trait provides this one.
pub trait LocalSockets {
    const PROVIDED: bool = true;
    /// Gives the socket a path in the file system. The path is created and nobody removes it.
    unsafe fn bind(socket: *mut c_void, path: &[u8]) -> Result<()>;
    /// Follows `Sockets::connect`: `Err(InProgress)` on a non-blocking socket that has to wait.
    unsafe fn connect(socket: *mut c_void, path: &[u8]) -> Result<()>;
    /// The path of the socket, or of its peer, with the buffer contract of `Sockets::host_name`;
    /// `Err(NotFound)` for a socket without a path.
    unsafe fn address(socket: *mut c_void, peer: bool, out: *mut u8, capacity: usize) -> Result<usize>;
    /// The numeric user the process at the other end of a connected stream socket ran as when it connected.
    unsafe fn peer_user(socket: *mut c_void) -> Result<u32>;
}
/// Users and groups of the target (`CAP_ACCOUNTS`).
pub trait Accounts {
    const PROVIDED: bool = true;
    /// `Err(NotFound)` when no account has the id. `accounts::Account::new` builds the answer.
    fn user_by_id(user_id: u32) -> Result<crate::accounts::Account>;
    fn user_by_name(name: &[u8]) -> Result<crate::accounts::Account>;
    /// Writes up to `capacity` supplementary group ids of this process and returns how many there are.
    unsafe fn process_groups(out: *mut u32, capacity: usize) -> Result<usize>;
    /// The same for the groups an account belongs to, `primary_group` included.
    unsafe fn user_groups(name: &[u8], primary_group: u32, out: *mut u32, capacity: usize) -> Result<usize>;
}
/// Scheduling priority of a process (`CAP_PRIORITY`): a niceness from -20 to 19; process 0 is this one.
pub trait Priority {
    const PROVIDED: bool = true;
    fn get(process: u64) -> Result<i32>;
    fn set(process: u64, value: i32) -> Result<()>;
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
absent!(Topology {
    fn cpu_max() -> Result<u32> { Err(Error::Unsupported) }
    fn cpu_count() -> Result<u32> { Err(Error::Unsupported) }
    fn current_cpu() -> Result<u32> { Err(Error::Unsupported) }
    unsafe fn process_affinity(_: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    fn set_thread_affinity(_: u32) -> Result<()> { Err(Error::Unsupported) }
    fn physical_memory() -> Result<(u64, u64)> { Err(Error::Unsupported) }
    fn memory_limit() -> Result<u64> { Err(Error::Unsupported) }
    fn virtual_limit() -> Result<u64> { Err(Error::Unsupported) }
    fn cache_size() -> Result<usize> { Err(Error::Unsupported) }
    fn cpu_features() -> Result<(u64, u64)> { Err(Error::Unsupported) }
});
absent!(Process {
    fn exit(_: i32) -> ! { Trap::abort() }
    fn debugger_present() -> Result<bool> { Err(Error::Unsupported) }
    unsafe fn crash_dump(_: &[*const u8], _: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Streams {
    unsafe fn write(_: u32, _: *const u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn read(_: u32, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    fn is_terminal(_: u32) -> Result<bool> { Err(Error::Unsupported) }
});
absent!(Files {
    unsafe fn open(_: &[u8], _: u32, _: u32) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn close(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn read_at(_: *mut c_void, _: u64, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn write_at(_: *mut c_void, _: u64, _: *const u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn set_size(_: *mut c_void, _: u64) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn flush(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn status(_: *mut c_void) -> Result<crate::files::Status> { Err(Error::Unsupported) }
    unsafe fn path_status(_: &[u8], _: bool) -> Result<crate::files::Status> { Err(Error::Unsupported) }
    unsafe fn remove(_: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn rename(_: &[u8], _: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn create_directory(_: &[u8], _: u32) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn remove_directory(_: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn open_directory(_: &[u8]) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn read_directory(_: *mut c_void, _: *mut u8, _: usize) -> Result<(usize, u32)> { Err(Error::Unsupported) }
    unsafe fn close_directory(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn current_directory(_: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
});
absent!(Sockets {
    unsafe fn create(_: u32, _: u32) -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn close(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn bind(_: *mut c_void, _: &crate::sockets::Address) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn listen(_: *mut c_void, _: u32) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn accept(_: *mut c_void) -> Result<(*mut c_void, crate::sockets::Address)> { Err(Error::Unsupported) }
    unsafe fn connect(_: *mut c_void, _: &crate::sockets::Address) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn send(_: *mut c_void, _: *const u8, _: usize, _: Option<&crate::sockets::Address>) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn receive(_: *mut c_void, _: *mut u8, _: usize, _: u32) -> Result<(usize, Option<crate::sockets::Address>)> { Err(Error::Unsupported) }
    unsafe fn shutdown(_: *mut c_void, _: u32) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn local_address(_: *mut c_void) -> Result<crate::sockets::Address> { Err(Error::Unsupported) }
    unsafe fn peer_address(_: *mut c_void) -> Result<crate::sockets::Address> { Err(Error::Unsupported) }
    unsafe fn set_blocking(_: *mut c_void, _: bool) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn get_option(_: *mut c_void, _: u32) -> Result<u64> { Err(Error::Unsupported) }
    unsafe fn set_option(_: *mut c_void, _: u32, _: u64) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn poll(_: &mut [crate::sockets::PollEntry], _: u64, _: Option<u32>) -> Result<()> { Err(Error::Unsupported) }
    fn wake(_: u32) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn resolve(_: &[u8], _: u32, _: *mut crate::sockets::Address, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn host_name(_: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
});
absent!(Faults { fn enable() -> Result<()> { Err(Error::Unsupported) } });
absent!(SystemInfo {
    unsafe fn environment_entry(_: usize, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn text(_: u32, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    fn process_times() -> Result<(u64, u64)> { Err(Error::Unsupported) }
    fn uptime_ns() -> Result<u64> { Err(Error::Unsupported) }
    fn user_ids() -> Result<(u32, u32)> { Err(Error::Unsupported) }
});
absent!(Notifications {
    fn start() -> Result<()> { Err(Error::Unsupported) }
    fn enable(_: u32) -> Result<()> { Err(Error::Unsupported) }
    fn disable(_: u32) -> Result<()> { Err(Error::Unsupported) }
    fn default_action(_: u32) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Processes {
    unsafe fn spawn(_: &[u8], _: &[*const u8], _: Option<&[*const u8]>, _: Option<&[u8]>, _: u32) -> Result<crate::processes::Spawned> { Err(Error::Unsupported) }
    unsafe fn wait(_: *mut c_void, _: u64) -> Result<i32> { Err(Error::Unsupported) }
    unsafe fn terminate(_: *mut c_void, _: bool) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn release(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn pipe_read(_: *mut c_void, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn pipe_write(_: *mut c_void, _: *const u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn pipe_close(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Terminal {
    fn window_size(_: u32) -> Result<(u32, u32)> { Err(Error::Unsupported) }
    fn set_input_mode(_: bool, _: u8, _: u8, _: bool) -> Result<()> { Err(Error::Unsupported) }
    fn input_ready() -> Result<bool> { Err(Error::Unsupported) }
    fn control_character(_: u32) -> Result<u8> { Err(Error::Unsupported) }
});
absent!(Watches {
    fn open() -> Result<*mut c_void> { Err(Error::Unsupported) }
    unsafe fn close(_: *mut c_void) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn add(_: *mut c_void, _: &[u8], _: u32) -> Result<u32> { Err(Error::Unsupported) }
    unsafe fn remove(_: *mut c_void, _: u32) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn read(_: *mut c_void, _: u64, _: &mut crate::watches::Event) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Mappings {
    unsafe fn map(_: *mut c_void, _: u64, _: usize, _: u32, _: bool) -> Result<*mut u8> { Err(Error::Unsupported) }
    unsafe fn unmap(_: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn sync(_: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Volumes {
    unsafe fn entry(_: usize, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    fn status(_: &[u8]) -> Result<crate::volumes::Status> { Err(Error::Unsupported) }
});
absent!(Network {
    fn interface_entry(_: usize) -> Result<crate::network::Interface> { Err(Error::Unsupported) }
    fn address_entry(_: usize) -> Result<crate::network::InterfaceAddress> { Err(Error::Unsupported) }
    unsafe fn reverse_lookup(_: &crate::sockets::Address, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn membership(_: *mut c_void, _: &crate::sockets::Address, _: u32, _: bool) -> Result<()> { Err(Error::Unsupported) }
});
absent!(LocalSockets {
    unsafe fn bind(_: *mut c_void, _: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn connect(_: *mut c_void, _: &[u8]) -> Result<()> { Err(Error::Unsupported) }
    unsafe fn address(_: *mut c_void, _: bool, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn peer_user(_: *mut c_void) -> Result<u32> { Err(Error::Unsupported) }
});
absent!(Accounts {
    fn user_by_id(_: u32) -> Result<crate::accounts::Account> { Err(Error::Unsupported) }
    fn user_by_name(_: &[u8]) -> Result<crate::accounts::Account> { Err(Error::Unsupported) }
    unsafe fn process_groups(_: *mut u32, _: usize) -> Result<usize> { Err(Error::Unsupported) }
    unsafe fn user_groups(_: &[u8], _: u32, _: *mut u32, _: usize) -> Result<usize> { Err(Error::Unsupported) }
});
absent!(Priority {
    fn get(_: u64) -> Result<i32> { Err(Error::Unsupported) }
    fn set(_: u64, _: i32) -> Result<()> { Err(Error::Unsupported) }
});
absent!(Image {
    unsafe fn unwind_info(_: usize) -> Result<crate::image::UnwindInfo> { Err(Error::Unsupported) }
    unsafe fn readable(_: usize, _: usize) -> Result<bool> { Err(Error::Unsupported) }
    unsafe fn build_id(_: usize, _: *mut u8, _: usize) -> Result<usize> { Err(Error::Unsupported) }
});

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
    type Topology: Topology;
    type Process: Process;
    type Image: Image;
    type Streams: Streams;
    type Files: Files;
    type Sockets: Sockets;
    type Faults: Faults;
    type SystemInfo: SystemInfo;
    type Notifications: Notifications;
    type Processes: Processes;
    type Terminal: Terminal;
    type Watches: Watches;
    type Mappings: Mappings;
    type Volumes: Volumes;
    type Network: Network;
    type LocalSockets: LocalSockets;
    type Accounts: Accounts;
    type Priority: Priority;
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
            type Topology = $crate::__port_lookup!(Topology; $($key = $provider),*);
            type Process = $crate::__port_lookup!(Process; $($key = $provider),*);
            type Image = $crate::__port_lookup!(Image; $($key = $provider),*);
            type Streams = $crate::__port_lookup!(Streams; $($key = $provider),*);
            type Files = $crate::__port_lookup!(Files; $($key = $provider),*);
            type Sockets = $crate::__port_lookup!(Sockets; $($key = $provider),*);
            type Faults = $crate::__port_lookup!(Faults; $($key = $provider),*);
            type SystemInfo = $crate::__port_lookup!(SystemInfo; $($key = $provider),*);
            type Notifications = $crate::__port_lookup!(Notifications; $($key = $provider),*);
            type Processes = $crate::__port_lookup!(Processes; $($key = $provider),*);
            type Terminal = $crate::__port_lookup!(Terminal; $($key = $provider),*);
            type Watches = $crate::__port_lookup!(Watches; $($key = $provider),*);
            type Mappings = $crate::__port_lookup!(Mappings; $($key = $provider),*);
            type Volumes = $crate::__port_lookup!(Volumes; $($key = $provider),*);
            type Network = $crate::__port_lookup!(Network; $($key = $provider),*);
            type LocalSockets = $crate::__port_lookup!(LocalSockets; $($key = $provider),*);
            type Accounts = $crate::__port_lookup!(Accounts; $($key = $provider),*);
            type Priority = $crate::__port_lookup!(Priority; $($key = $provider),*);
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
    (Topology; Topology = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Process; Process = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Image; Image = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Streams; Streams = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Files; Files = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Sockets; Sockets = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Faults; Faults = $t:ty $(, $($rest:tt)*)?) => { $t };
    (SystemInfo; SystemInfo = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Notifications; Notifications = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Processes; Processes = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Terminal; Terminal = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Watches; Watches = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Mappings; Mappings = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Volumes; Volumes = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Network; Network = $t:ty $(, $($rest:tt)*)?) => { $t };
    (LocalSockets; LocalSockets = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Accounts; Accounts = $t:ty $(, $($rest:tt)*)?) => { $t };
    (Priority; Priority = $t:ty $(, $($rest:tt)*)?) => { $t };
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
