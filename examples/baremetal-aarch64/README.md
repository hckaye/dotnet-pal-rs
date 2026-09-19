# Bare-metal AArch64 port

A port of the dotnet-pal-rs boundary to a machine with no operating system:
`qemu-system-aarch64 -M virt` with 1 GiB of RAM, no libc, no firmware beyond
QEMU's own `-kernel` loader. The crate is the whole platform. It boots the core,
maps memory, drives the serial port and runs threads, and then answers
`dotnet_pal_get_api(2)` on top of that.

The example is its own workspace and is not part of the repository workspace.

## Building and running

```sh
cargo build --manifest-path examples/baremetal-aarch64/Cargo.toml --target aarch64-unknown-none --release
bash examples/baremetal-aarch64/run-qemu.sh
```

`run-qemu.sh` builds the archive, compiles the freestanding C and C++ test,
links the image with `link.sh` and runs it. It keeps the serial output in
`artifacts/qemu.log` and exits with the status the image asked for through
semihosting. A successful run prints:

```text
MEMORY PROTECTION PASS reserve, decommit, release, RO, NX, RX and instruction cache
TABLE PASS threads=60 waits=... timeouts=... locks=... reserves=... heap=... rejected=... region=... free=...
EXIT code=0
```

`SKIP_BUILD=1 bash run-qemu.sh` runs the image that is already linked. `CLANG`,
`LD` and `LIBGCC` override the compiler, the linker and an extra archive to link
(nothing in the current image needs one).

Everything needs a Linux toolchain: Rust 1.98.1 with the `aarch64-unknown-none`
target, clang and `ld.lld` 18, and `qemu-system-aarch64`. `container/Dockerfile`
builds an image with all of them plus the pinned .NET SDK, for macOS developers.

## What the port provides

| Capability | Provider |
| --- | --- |
| `VirtualMemory`, `NativeMapping` | identity-backed RAM with 4 KiB page protection |
| `NativeHeap` | first fit over a 64 MiB slice of that RAM, 16-byte alignment |
| `Clock`, `Realtime`, `Scheduler` | `CNTVCT_EL0`, plus a fixed epoch for the wall clock |
| `Events`, `Mutexes`, `Threads`, `ThreadLocal`, `RwLocks` | the cooperative scheduler |
| `StackBounds`, `Identity`, `ThreadName` | the thread table |
| `ProcessBarrier` | `dsb ish` |
| `Diagnostics`, `Streams` | the PL011 UART at 0x0900_0000; reads report end of input |
| `Topology` | one CPU, the RAM figures, CPU feature words from the ID registers |
| `Process` | exit through semihosting; no debugger, no crash dump utility |
| `Image` | the image's own `.eh_frame_hdr`/`.eh_frame` from the linker script; readability checks the RAM page descriptors; device space is not readable |
| `Modules` | the one image, named `app`, base at the load address |
| `Faults` | the synchronous exception vector: the interrupted registers go to the installed handler, which may edit them and resume |
| `Files`, `Volumes`, `Watches`, `Mappings` | `dotnet-pal-memfs`, an in-memory file system on the port's heap through the Rust global allocator, with links, modes, times, locks, change events and file mappings; times from the port's wall clock; a reader that waits sleeps in the port's scheduler; one volume at `/` with the capacity of the file system |
| `SystemInfo` | the machine's name and the crate version as OS texts, the time since reset as uptime, `/app` as the executable's path; an empty environment enumeration; no user and no CPU accounting |

Absent, and reported as absent capability bits with NULL callbacks:
`Environment`, `Entropy`, `Context`, `Sockets`, `Network`, `Notifications`,
`Processes`, `Terminal`, `LocalSockets`, `Accounts`, `Priority`, `Wasi` and linear storage.

## Running a C# program

```sh
docker run --rm -v "$PWD:/work" -v dotnet-pal-nuget:/nuget -w /work dotnet-pal-baremetal \
  bash examples/baremetal-aarch64/build-app.sh
```

`build-app.sh` needs `artifacts/source-sdk` from `scripts/source-runtime.sh`. It
builds the port, the freestanding C runtime (`native/freestanding`) and the boundary's
System.Native (`native/system_native_*.c`), publishes the sample for linux-arm64 to get
the ILCompiler object, links everything with `link.ld` and runs the image. The
argument names the sample: `ConsoleProbe` (the default) prints `CONSOLE PROBE PASS`,
`IoProbe` prints `IO PROBE PASS`, `SystemProbe` prints `SYSTEM PROBE PASS`,
`FacilitiesProbe` prints `FACILITIES PROBE PASS`, and the machine exits 0. `PAL_TRACE=1` builds the
port with the `trace` feature, which prints negotiation, reservations, commits,
object creation, thread creation, resumed faults and exits on the serial port.

The managed program sees one processor, no environment variables, an input stream
at end of input, an empty file system rooted at `/` that lives in RAM, and no
network: creating a socket fails as `AddressFamilyNotSupported`. Symbolic links,
permission bits, timestamps, file locks and the working directory work on that file
system, `FileSystemWatcher` and `MemoryMappedFile` work on it, and `DriveInfo` reports it
as one drive of format `memfs`. Starting a child process, registering for a signal,
loading a native library, listing network interfaces and opening a Unix domain socket
fail as unsupported, which `SystemProbe` and `FacilitiesProbe` assert. An anonymous
`MemoryMappedFile.CreateNew` fails because the machine has no entropy for the `Guid` the
BCL names its backing file with.
A null dereference
in managed code becomes `NullReferenceException`. The vector reports the fault, the
runtime's handler redirects the frame to its throw helper, and the port resumes
there. A fault nobody claims (in native code, or a second fault inside the handler)
prints the syndrome and ends the run with status 132.

## What it does not provide

- **One core, cooperatively scheduled.** A thread keeps the CPU until it waits
  or yields. There is no timer interrupt and no preemption; interrupts stay
  masked for the whole run. A thread that spins without calling the port hangs
  the machine.
- **Waiting is polling.** Blocking on an event, a mutex, a reader/writer lock, a
  join or a sleep means giving the CPU to the next thread and checking again
  when it comes back. With nothing else to run, the waiter spins on the counter.
  Waiting costs CPU, and a wait for something nobody will do hangs the run
  instead of being reported.
- **Bounded, identity-backed memory.** Reservations and decommitted pages are
  inaccessible, and native mappings enforce read/write/execute permissions at
  4 KiB granularity. New commits are zeroed; repeated commits preserve data.
  There is no overcommit, swapping or relocation of physical backing: reserving
  addresses still consumes the finite RAM budget, and decommit does not return
  that budget until release. Write-only and execute-only mappings are rejected
  rather than widened to readable mappings. The boot image is not hardened to
  W^X and there are still no stack guard pages.
- **No wall clock.** `realtime_ns` is the fixed epoch 2026-01-01T00:00:00Z plus
  the time since reset. The machine has no battery-backed clock and no network.
- **No entropy, no environment, no signals, no network.** Synchronous exceptions
  are reported through the `faults` group; every other exception (SError, an
  interrupt, anything from a lower level) ends the run with the syndrome and status
  132.
- **Files do not persist.** The file system is a tree in RAM, empty at every boot
  and bounded at 64 MiB of content, the size of the heap slice it shares with the
  runtime's native allocations.
- **No console input.** `console_read` is always `None`.
- Fixed limits: 16 threads, 64 thread-local slots, 64 events, 64 mutexes, 32
  reader/writer locks. Exhausting one is `Error::OutOfMemory`.

## Memory

The image links at 0x4008_0000 and QEMU loads it there. RAM is
0x4000_0000 to 0x8000_0000, which is why the machine must be run with `-m 1024`.

| Range | Content |
| --- | --- |
| 0x4008_0000 | `.text`, `.rodata`, `.eh_frame`, `.data`, `.init_array`, `.tdata`, `.tbss`, `.bss` |
| after `.bss`, 2 MiB | boot thread stack (`__boot_stack_bottom` to `__boot_stack_top`) |
| `__heap_start` to 0x8000_0000 | the static region, about 1019 MiB (depending on image size) |

The region is an address-ordered free list with coalescing, first fit, and no
per-allocation metadata: the caller returns the size it was given, which is what
the C boundary requires of `release`. Virtual memory, native mappings, thread
stacks (128 KiB by default, one page granularity) and per-thread TLS blocks (one
page each) all come from it. The native helper heap takes one 64 MiB slice on its
first allocation and manages that slice itself.

The identity map has one level-1 table. Its first entry points to a level-2 table
of 2 MiB blocks for 0 to 1 GiB: the first block is invalid, so the null page
faults, and the rest is Device-nGnRnE and execute never, which covers the UART.
Its second entry points to another level-2 table and 512 level-3 tables covering
RAM with Normal write-back cacheable 4 KiB pages. The image, boot stack and page
tables remain mapped. Free pages are inaccessible; the allocator maps owned
storage RW/NX, and VM reservations revoke access until commit. Permission changes
use break-before-make and invalidate the old translations. Executable mappings
synchronize the data and instruction caches, including repeated RW-to-RX changes.
Everything else is unmapped, so a stray access faults instead of reaching a device.

## Boot

`_start` in `src/boot.S` runs with the MMU and caches off. It drops to EL1 if
QEMU started it at EL2, sets the stack, zeroes `.bss`, allows FP/SIMD, builds the
identity map, enables the MMU and both caches and installs the exception vectors.
`pal_rust_start` then publishes the region, builds the boot thread's ELF TLS
block, runs `.init_array` and calls `main(1, {"app", NULL})`. The status `main`
returns leaves the machine through the semihosting call `SYS_EXIT_EXTENDED`, so
`qemu -semihosting` exits with it. `.fini_array` is not run.

Thread-local variables use the aarch64 variant I layout: `TPIDR_EL0` points at a
16-byte TCB, and the thread's copy of `.tdata` followed by its zeroed `.tbss`
begins right after it. The linker script asserts that no thread-local asks for
more than 16-byte alignment, which is what lets the block sit at a fixed offset
from the thread pointer.

A context switch (`src/switch.S`) saves x19-x28, the frame pointer, the link
register, d8-d15 and `TPIDR_EL0`. A new thread starts from a frame built by hand
in the same shape.

A synchronous exception at EL1 (`pal_fault_entry` in `src/boot.S`) pushes the
boundary's AArch64 fault frame (x0-x30, sp, pc, pstate) and the FP/SIMD state on
the faulting thread's stack and calls `pal_fault` in `src/fault.rs`, which maps the
syndrome to a fault kind and calls `dotnet_pal_rs::faults::deliver`. When the
handler answers "resume", the frame, as edited, becomes the register state again
and the vector returns with `eret`. `TPIDR_EL1` is the scratch register of that
last step.

## The test

`tests/table.c` links against the port archive and exercises the negotiated
table: the advertised and absent capabilities, reserve/commit/decommit/release
with the page rules, the native heap, native mappings, clock monotonicity and
the wall clock, sleep, manual and auto-reset events including timeouts and a
cross-thread set, a recursive mutex, four worker threads plus fifty detached
ones, thread-local slots with a destructor observed at thread exit, stack
bounds, reader/writer locks including a reader that has to wait for a writer,
thread names, diagnostic output, and ELF thread-locals that stay per thread
across context switches. It installs a fault handler and loads through null and
through addresses up to the end of the null page, on the boot thread and on a
second one: the handler supplies the value the load could not, steps over the
instruction, and the caller-saved integer and FP registers of the interrupted
code come back unchanged. It writes a megabyte to a file in the in-memory file
system and reads it back, renames, enumerates and removes. `tests/ctor.cpp` proves
`.init_array` ran, and
`tests/registers.S` checks that the callee-saved registers survive a switch by
hand, because a C compiler is free to spill them around a call.

The memory checks also require hardware faults after reserve, decommit and
release, reject writes to read-only mappings and instruction fetches from NX
mappings, execute generated code after RW-to-RX transitions, and repeat after
rewriting the instructions. Return codes alone cannot satisfy those checks.

The test uses no libc: the only functions it calls outside itself are the ones
the port exports. It prints `TABLE PASS` and exits 0, or `FAIL <what>` and exits 1.
