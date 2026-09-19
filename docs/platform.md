# Topology, process, image and stream groups, the isolation gate and the BCL native layer

These four capability groups complete the OS-service surface of the source-integrated
NativeAOT runtime: with them, the rebuilt runtime archive and the rebuilt `minipal`
archive reference nothing outside the boundary except reviewed non-OS contracts.
The same groups back the boundary's own implementation of System.Native, the BCL's
native layer. The groups that layer is built on are described elsewhere: files, sockets
and fault reporting in [io](io.md), system facts, notifications, child processes and the
terminal in [system](system.md), change watching, file mappings, volumes and network
information in [facilities](facilities.md).

## Groups

All four are append-only groups after `support` in `dotnet_pal_api`. A consumer
checks `struct_size` against the group's `*_API_SIZE` and the capability bit
before reading the callbacks.

| Group | Capability bit | Callbacks | Trait |
| --- | --- | --- | --- |
| `topology` | `DOTNET_PAL_CAP_TOPOLOGY` | `cpu_max`, `cpu_count`, `current_cpu`, `process_affinity`, `set_thread_affinity`, `physical_memory`, `memory_limit`, `virtual_limit`, `cache_size`, `cpu_features` | `port::Topology` |
| `process` | `DOTNET_PAL_CAP_PROCESS` | `exit`, `debugger_present`, `crash_dump` | `port::Process` |
| `image` | `DOTNET_PAL_CAP_IMAGE` | `unwind_info`, `readable`, `build_id` | `port::Image` |
| `streams` | `DOTNET_PAL_CAP_STREAMS` | `write`, `read`, `is_terminal` | `port::Streams` |

Topology answers are logical CPU counts, a little-endian affinity bitmap, memory
figures within the limit in force (a container limit when one exists), the largest
per-CPU data cache and two target-defined CPU feature words (Linux arm64: `AT_HWCAP`
and `AT_HWCAP2`). A question the target cannot answer returns `UNSUPPORTED` for that
call; the capability stays whole. NUMA placement is not part of the boundary: every
consumer sees one node.

`process.exit` never returns. `crash_dump` runs an argument vector as a crash-dump
utility that may inspect the process and waits for it; a port without such a facility
answers `UNSUPPORTED`, and a runtime asked for dumps through `DOTNET_DbgEnableMiniDump`
then refuses to start instead of silently producing none.

`image.unwind_info` names the image containing an address and its DWARF tables
(`eh_frame_hdr` when the image has one). `readable` says whether a range can be read
without faulting, which the unwinder needs to recognize signal frames. `build_id` copies
the image's build identifier.

`streams` carries the three standard streams only: 0 is input, 1 output, 2 error
output. `write` never reports zero bytes with success; `read` reports zero bytes with
success at end of input. It is not a file API.

Providers: the Linux backend (`src/linux_platform.rs`), the desktop `std` port
(`crates/dotnet-pal-std/src/system.rs`, image inspection on Linux only), C host tables
under the `host-topology`, `host-process`, `host-image` and `host-streams` features with
reference providers in `tests/*_host.c`, and the bare-metal example. `scripts/platform.sh`
runs the conformance tests against the kernel's own answers.

## Runtime source integration

`integration/dotnet10/topology_patch.py`, `process_patch.py`, `image_patch.py` and
`minipal_patch.py` route the remaining OS calls of the pinned runtime through these
groups:

- The GC's CPU and memory queries, affinity and cache size use `topology`; the cgroup,
  cgroup CPU and NUMA readers are compiled out.
- The crash-dump launch uses `process.crash_dump`; the runtime keeps its argument
  building and the utility path comes from the `runtime` group's module name.
- The unwinder's section lookup, its readability probe and its diagnostic lines use
  `image` and the diagnostics channel; the runtime's build-id lookup uses `image.build_id`.
- `minipal` (clock, thread id, debugger, entropy, log output, mutexes, CPU count,
  CPU features) uses the C front end `native/minipal_pal_adapter.h`; the rebuilt
  `libaotminipal.a` replaces the SDK's in the source overlay.
- The thread identity of the GC's `EEThreadId` uses the `runtime` group.

A port may omit the signal substrate (`Context`). The runtime then installs no signal
handlers and injects no activations: suspension relies on the trap flag and on the
preemptive-mode transitions every blocking port call makes. Hardware faults reach the
runtime through the `faults` group when the port provides it ([io](io.md)); without
it a fault ends the run instead of becoming a managed exception. The environment, entropy and
module loading are optional too; a runtime without them sees every variable unset, no
entropy source and no loadable modules.

## The isolation gate

`scripts/audit_dependencies.py --require-isolated` passes only when every external
reference of the audited archive set is the boundary entry point or a contract listed in
`integration/dotnet10/reviewed_references.json`: names the compiler output defines, the
Itanium C++ ABI, compiler runtime builtins by exact family, the freestanding C runtime
contract (string, memory, formatting, number parsing, errno, termination), libm, the
event tracing archives, and two assembler artifacts of the write barriers. A name that
matches none of them is unreviewed and fails the gate; a prefix is never approval.
`scripts/source-runtime.sh` audits the rebuilt runtime and minipal archives together and
requires isolation.

The gate is a link-reference statement about the archives, not a syscall trace of the
final executable. The Linux executable still links glibc for the reviewed C runtime
contract, and the Rust Linux backend makes the OS calls the boundary routes.

## System.Native over the boundary

Five units implement System.Native on the negotiated table. `native/system_native_pal.c`
has the native heap, threads and low-level monitors, clocks, entropy, environment lookup,
error-code translation and diagnostics. `native/system_native_io.c` has the descriptor
table, the standard streams, files and directories over the `files` group, change watching,
file mappings and volumes. `native/system_native_net.c` has sockets, readiness events and
name resolution over the `sockets` group, network interfaces, reverse lookup and
multicast membership over the `network` group, Unix domain sockets over the
`local_sockets` group, and packet information over the `packets` group. `native/system_native_sys.c` has the
environment enumeration, OS, user and process facts, accounts, group lists and priorities,
signal registrations over the `notifications` group, the terminal and module loading. `native/system_native_proc.c` has
child processes, also under another identity, and their pipes. [io](io.md), [system](system.md) and
[facilities](facilities.md) describe the groups behind them. The facilities probe links 224
entry points, all from these units. What the boundary does not carry (sessions, resource limits, raw sockets other than ICMP,
control messages other than packet information, network change events) reports `ENOTSUP`,
`ENOENT` or `EAFNOSUPPORT`, so managed code sees an honest failure. Without the `files`
group no path exists, and without the `sockets` group no address family does.
`native/system_native_abi.h` pins the enumerations and structure layouts of the audited
runtime commit; `tests/test_system_native_abi.py` compares them with the pinned headers
when a checkout is available.

`scripts/console-probe.sh` publishes `samples/ConsoleProbe` with the source-built runtime
and this System.Native in place of the SDK's, runs it, checks from the link map that
every `SystemNative_*` symbol came from the boundary's objects, and audits the runtime,
minipal and System.Native archives together with the gate. `scripts/io-probe.sh`,
`scripts/system-probe.sh`, `scripts/facilities-probe.sh` and `scripts/terminal-probe.sh` do
the same with `samples/IoProbe`, `samples/SystemProbe`, `samples/FacilitiesProbe` and
`samples/TerminalProbe`, the last on a pseudo-terminal.

## The freestanding C runtime

`native/freestanding` is the C runtime contract for targets without a libc: string and
memory functions, `vsnprintf` and `sscanf` subsets, number parsing, errno, `abort`,
`exit`, `atexit`, the C++ ABI allocation operators and guards, the stack protector and
cache maintenance builtins, and a heap over the boundary's native heap. It contains no OS
calls; the two hooks `dotnet_pal_freestanding_abort` and `dotnet_pal_freestanding_exit`
are the port's. Its README lists the subset limits; `selftest.sh` compares it with glibc.

## The bare-metal example

`examples/baremetal-aarch64` links the ILCompiler output for linux-arm64, the source-built
runtime and minipal, the boundary's System.Native, the freestanding C runtime and the
port into one image for `qemu-system-aarch64 -M virt` with no OS and no libc
(`build-app.sh`). The console probe runs there: exceptions, GC and finalizers, thread pool
tasks and monitors, math and formatting. So does the I/O probe: the BCL's file APIs on the
in-memory file system, null references that arrive as `NullReferenceException` through the
exception vector and the `faults` group, and the honest failure of socket creation. The
system probe runs links, modes, times, locks and the working directory on that file system
and sees processes, signal registrations and module loading fail as absent. The facilities
probe watches and maps files of the in-memory file system, reads it as a drive through
`DriveInfo`, and sees network information and local sockets fail as absent. The example's README lists
what that machine provides and what it does not.
