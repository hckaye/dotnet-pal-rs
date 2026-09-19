# Files, sockets and fault reporting

Three capability groups follow `streams` in `dotnet_pal_api`. `files` and `sockets`
are what the boundary's System.Native builds the BCL's file and network APIs on.
`faults` lets a port without POSIX signals hand CPU faults to the runtime, which turns
a null dereference in compiled managed code into `NullReferenceException`. A port
implements each group as one Rust trait, or leaves it absent.

| Group | Capability bit | Trait | Callbacks |
| --- | --- | --- | --- |
| `files` | `DOTNET_PAL_CAP_FILES` | `port::Files` | `open`, `close`, `read_at`, `write_at`, `set_size`, `flush`, `status`, `path_status`, `remove`, `rename`, `directory_create`, `directory_remove`, `directory_open`, `directory_read`, `directory_close`, `current_directory` |
| `sockets` | `DOTNET_PAL_CAP_SOCKETS` | `port::Sockets` | `create`, `close`, `bind`, `listen`, `accept`, `connect`, `send`, `receive`, `shutdown`, `local_address`, `peer_address`, `set_blocking`, `get_option`, `set_option`, `poll`, `wake`, `resolve`, `host_name` |
| `faults` | `DOTNET_PAL_CAP_FAULTS` | `port::Faults` | `frame_tag`, `frame_size`, `install` |

## Statuses

Both I/O groups report conditions with the boundary's own status codes
(`DOTNET_PAL_NOT_FOUND`, `ALREADY_EXISTS`, `ACCESS_DENIED`, `IS_DIRECTORY`,
`NOT_DIRECTORY`, `NOT_EMPTY`, `NO_SPACE`, `WOULD_BLOCK`, `BROKEN_PIPE`,
`CONNECTION_REFUSED`, `CONNECTION_RESET`, `IN_PROGRESS` and the rest of the list in
`include/dotnet_pal.h`). In Rust they are variants of `port::Error`. A provider never
sees or produces an errno value: `native/system_native_io.c` maps each status to the
Linux errno the managed side expects. The front ends hold every call to the set of
statuses its group defines, so a provider that returns anything else is reported as
`OS_ERROR`.

## Files

Paths are byte strings without NUL, 1 to 4095 bytes, `/`-separated, passed to the
provider as managed code wrote them. A file handle has no position. `read_at` and
`write_at` name the offset on every call, and System.Native keeps the cursor that
`Read`, `Write` and `LSeek` need, one per open file and shared by descriptors that
`Dup` made. That leaves a provider with no seek state to get wrong: an in-memory
table, a flash file system and `pread`/`pwrite` all fit the same sixteen calls.

`open` takes `READ`, `WRITE`, `CREATE`, `EXCLUSIVE` and `TRUNCATE`; opening a
directory is `IS_DIRECTORY`. A transfer the handle was not opened for is
`ACCESS_DENIED`. `rename` replaces an existing destination file. `directory_read`
returns one name of at most 255 bytes per call and `NOT_FOUND` after the last; `.` and
`..` are never listed. `status` and `path_status` fill `dotnet_pal_file_status`: node
kind, permission bits, size, four timestamps in nanoseconds since the Unix epoch (zero
when the target keeps none), and an identity and device number that the BCL compares
to decide whether two paths are the same file.

Eleven further operations are optional: a provider that lacks one answers
`UNSUPPORTED` and keeps the capability. They are `set_mode` and `set_file_mode`,
`set_times` and `set_file_times` (a time given as `DOTNET_PAL_TIME_KEEP` stays
unchanged), `link`, `symlink`, `read_link`, `real_path`, `set_current_directory`,
`lock` and `lock_range`. Locks are advisory and held by the open handle: shared locks
coexist, an exclusive one excludes every other, and a lock that is not free is
`WOULD_BLOCK` unless the caller asked to wait. `lock_range` never waits; a shared range
needs a handle opened with `READ`, an exclusive one a handle opened with `WRITE`. A
handle that converts its whole-file lock may lose the old lock when the conversion is
refused (`flock` on Linux does that), so a consumer that needs the old lock takes it
again. `FileShare.None` and `FileStream.Lock` rest on these: with a provider that has
locks, a second open of an exclusively opened file fails as a sharing violation; with
one that has none, System.Native reports `ENOTSUP` and the BCL carries on without, as
it does on a file system that refuses `flock`.

Memory-mapped files and change notifications are groups of their own
([facilities](facilities.md)). `File.Copy` copies content, and the times and permission
bits of the source where the provider has `set_file_times` and `set_file_mode`.

## Sockets

TCP streams and UDP datagrams over IPv4 and IPv6. An address is
`dotnet_pal_socket_address` (family, port in host order, scope, 16 address bytes),
not a platform `sockaddr`. Managed code never looks inside a socket address: it asks
System.Native for the sizes and goes through its accessors for every field, so the
address buffer the BCL carries around is the boundary's structure.

Sockets start blocking. `set_blocking(socket, 0)` makes `accept`, `send` and
`receive` report `WOULD_BLOCK` and `connect` report `IN_PROGRESS`. On a blocking
socket an expired `RECEIVE_TIMEOUT` or `SEND_TIMEOUT` is `TIMEOUT`. Options are
integers: flags are 0 or 1, sizes are bytes, timeouts are milliseconds, `LINGER` is 0
when off and seconds plus one when on, `ERROR` reads and clears the pending error as a
status code, `AVAILABLE` is the number of bytes that can be received without waiting,
`KEEP_ALIVE_IDLE` and `KEEP_ALIVE_INTERVAL` are seconds, `KEEP_ALIVE_COUNT` is a number
of probes, `HOPS` and `MULTICAST_HOPS` are hop limits, `MULTICAST_INTERFACE` is an
interface index. An option the socket's protocol does not have is `UNSUPPORTED`.

`poll` is level-triggered and takes an optional wake channel. `wake(channel)` makes
the poll in progress on that channel return early, or the next one when none is in
progress, and never a poll on another channel. Channels exist because a process has
several waiters (the BCL runs one event loop per engine, and `Socket.Poll` calls
arrive from any thread): with a single shared wake, the waiter that consumes it is not
always the one it was meant for.

The BCL's socket engine expects edge-triggered readiness, as `epoll` with `EPOLLET`
gives it: one event when a socket becomes ready, and the next only after an operation
reported that it would block. `native/system_native_net.c` derives that from `poll`
and `wake`. An event port records, per registered socket, which events it has
delivered, and polls only for the rest. An operation that returns `WOULD_BLOCK` or
`IN_PROGRESS` clears the delivered mark of its direction and wakes the port's channel,
and so does every change to the registrations. A socket that is closed while a waiter
holds it in a poll is released after that poll returns, which the close triggers
through the same wake.

Multicast membership, reverse lookup and the list of network interfaces belong to the
`network` group ([facilities](facilities.md)). Unix domain and raw sockets, out-of-band
data, control messages and packet information are not carried; System.Native answers
`EAFNOSUPPORT` or `ENOTSUP`.

A program that touches the `Socket` type makes the BCL create its event ports before
it asks for any socket. System.Native therefore creates ports on a boundary without
the group as well; their waiter sleeps on a boundary event until the port is closed,
and the socket request itself then fails as `AddressFamilyNotSupported`.

## Fault reporting

The native context group gives the runtime raw signal machinery, which only a POSIX
target has. `faults` works the other way round. The port owns its trap path (an
exception vector, an exception port, a signal it keeps for itself) and reports each
synchronous CPU fault with the interrupted registers in a frame the boundary defines
per architecture:

| Tag | Frame |
| --- | --- |
| `DOTNET_PAL_FRAME_ARM64` | `x[31]`, `sp`, `pc`, `pstate` |
| `DOTNET_PAL_FRAME_X64` | sixteen general registers in instruction-encoding order, `rip`, `rflags` |

A port's trap handler builds the frame and calls one function:

```rust
use dotnet_pal_rs::faults::{self, Disposition, Frame};

fn on_data_abort(frame: &mut Frame, address: usize) {
    if faults::deliver(faults::ACCESS, address, frame) == Disposition::Resume {
        return; // restore the registers from `frame` and return from the exception
    }
    // nobody claimed it: end the run as before
}
```

`deliver` is lock-free and allocation-free. The consumer's handler, installed once
through `install`, runs on that path in the faulting thread; it may edit the frame and
answer `RESUME`, and it may not block, allocate, unwind or call the boundary. A fault
taken while the handler runs on the same thread ends the run, and telling that from a
concurrent fault on another thread is the port's job. The port may keep its own state
below the interrupted stack pointer, so a handler that lowers `sp` writes nothing
there, except inside a red zone the ABI makes the port skip (128 bytes on x86-64
System V and in Apple's AArch64 ABI, none in the standard AArch64 one).

The runtime patch (`integration/dotnet10/context_patch.py`, `native/faults_adapter.inl`)
uses the group when the signal substrate is absent. The adapter converts the frame
into the runtime's limited context, calls the handler the runtime registers for
hardware exceptions, and on "continue execution" writes back the control registers and
the two argument registers, so the port resumes in the runtime's throw helper. The
conversion exists for ARM64, the architecture a port has executed it on; on other
architectures the runtime leaves the capability unused and a fault ends the run.

The faulting address has to fault for any of this to happen: the bare-metal example
leaves the first 2 MiB of the address space unmapped for that reason.

## Providers

| Provider | Files | Sockets | Faults |
| --- | --- | --- | --- |
| Linux backend (`linux` feature) | `src/linux_files.rs` | `src/linux_sockets.rs`: one eventfd per wake channel | absent: Linux delivers faults as signals through `context` |
| Desktop `std` port | `std::fs` (`crates/dotnet-pal-std/src/files.rs`) | `socket2` (`crates/dotnet-pal-std/src/sockets.rs`): one socket pair per wake channel | SIGSEGV, SIGBUS, SIGFPE, SIGILL and SIGTRAP on Linux and macOS (`crates/dotnet-pal-std/src/faults.rs`); absent on Windows |
| `crates/dotnet-pal-memfs` | an in-memory file system, `no_std` plus `alloc`, with all eleven optional operations | | |
| C host tables | `host-files` | `host-sockets` | `host-faults`: the host calls the deliver callback it receives from `enable` |
| Bare-metal example | `dotnet-pal-memfs` over the port's heap | absent: the machine has no network device | the synchronous exception vector |

The Linux backend and `dotnet-pal-memfs` have all eleven optional file operations; the
Linux range locks are open-file-description locks. The `std` port has them through
`std::fs` and `File::lock`, with range locks on Linux and macOS and `UNSUPPORTED`
elsewhere. The bare-metal example gets them from `dotnet-pal-memfs`.

`dotnet-pal-memfs` is for ports without storage hardware. It keeps one process-wide
tree, takes its timestamps from a clock the port sets with `set_clock`, bounds file
content with `set_capacity`, and needs a global allocator from the final image. Hard
links and symbolic links behave as on Linux, and locks work between the handles of the
one process. It is not persistent, and it stores permission bits without enforcing
them.

`scripts/files.sh`, `scripts/sockets.sh` and `scripts/faults.sh` run the conformance
tests: the Linux providers and independent C reference providers behind host tables,
each compared with what the kernel itself answers, plus providers that break the
contract on purpose to show what the front ends sanitize. `scripts/faults.sh` takes
real faults: a null store resumed at a recovery function with edited argument
registers, on the main thread and on a second one.

## Managed evidence

`samples/IoProbe` is a C# program that uses `File`, `Directory`, `FileStream`,
`Socket`, `TcpClient`, `UdpClient` and `Dns`, synchronously and asynchronously, and
dereferences null through a field load, a reference store, an array element, a
virtual call and an interface call. `scripts/io-probe.sh` publishes it against the
source-built runtime and the boundary's System.Native, runs it on Linux, checks from
the link map that every `SystemNative_*` symbol came from the boundary, and applies
the isolation gate to the runtime, minipal and System.Native archives together.
`examples/baremetal-aarch64/build-app.sh IoProbe` runs the same program with no OS: the
files live in `dotnet-pal-memfs`, the null dereferences arrive through the exception
vector and the faults group, and socket creation fails as
`AddressFamilyNotSupported`, which the probe asserts.
