# Readiness gates and uncompleted work

Evidence is a **configuration at an exact commit**, not a blanket claim about
all Rust targets. The project is not yet a complete OS-independent NativeAOT port.
The following gates describe what the executable suites actually establish.

## Implemented qualification

| Gate | Implementation / evidence producer |
| --- | --- |
| Native VM and host replacement | C ABI tests, GC negative/positive controls, Linux x64/ARM64 |
| Machine measurements and CPU placement | Neutral CPU lists and memory queries; Linux and host implementations; source adapters preserve cgroup policy |
| Clocks, sleep, yield | Linux, immutable host tables, real WASIp1 clock/error tests |
| Events, recursive locks, threads, TLS, stacks, barriers | Linux and host-kernel contracts; native runtime/GC source adapters |
| Native source builds | Both WorkstationGC and ServerGC archives; actual x64/ARM64 execution without --wrap |
| GC roots/EH/thread/finalizer interaction | Concurrent managed stress, pinned/weak roots, filters/rethrow/finally, exact finalizers, native-thread callbacks |
| Low-memory recovery | Effective 128 MiB limit asserted; three exhaustion/recovery waves per regular backend and collector |
| Injected failure recovery | Test-only fail-before-side-effect VM commit; exactly one hit observed by actual GC, then recovery |
| Measured overhead | Alternating baseline/candidate workload trials, wall/CPU/RSS and file size reports |
| Linear storage contracts | Separate capability, invalid geometry/exhaustion/reuse/neighbor/concurrency tests |
| Managed WASIp1 | Audited LLVM compiler, real C# GC/roots/exception workload, observer-only negative/positive controls |
| WASI source build | Audited native LLVM runtime rebuilt, digest checked, executed without linker wrappers |
| Library layout | Trait-based ports (`define_pal!`), the std desktop port with its table test on Linux/macOS, the `build.rs` adapter helper |
| Browser host connection | C-to-Rust boundary with JS-imported clock/wall time/entropy/environment/diagnostics and `memory.grow` storage, executed under Node and headless Chromium |
| Managed browser execution | C# GC/finalizer/exception/BCL workload linked against the browser port, executed under Node with the page hosts and in headless Chromium; GC storage and clock counted through Rust |
| Mixed-language ASan | Rust/core and C/C++ boundary code instrumented together with leak checking |
| Dependency inventory | Actual runtime/PAL unresolved symbols plus executable imports, retaining unknowns and bypasses |
| Servicing policy | Version/digest guards and documented mandatory re-audit/qualification on upgrades |
| Runtime OS isolation | Topology, process, image and stream groups; the rebuilt runtime and minipal archives pass `--require-isolated` against the reviewed-contract manifest |
| BCL native layer | `native/system_native_*.c` on the boundary; the console, I/O, system and facilities probes executed on Linux with every `SystemNative_*` symbol from the boundary, audited with the gate |
| Files and sockets | `files` and `sockets` groups: Linux and host-table providers checked against the kernel, the `std::fs` and in-memory providers through the C table; `File`, `Directory`, `FileStream`, `Socket`, `TcpClient`, `UdpClient` and `Dns` executed from C# on Linux ARM64, synchronously and asynchronously |
| Faults without signals | `faults` group: real CPU faults reported, edited and resumed through a host table (ARM64; x86-64 under emulation), and through the bare-metal exception vector; null references caught as `NullReferenceException` on a port with no signal substrate |
| System facts, notifications, processes, terminal | `system`, `notifications`, `processes` and `terminal` groups and the eleven optional file operations: Linux and host-table providers checked against the kernel, the `std` providers through the C table on Linux and macOS; `Environment`, `Process`, `PosixSignalRegistration`, links, modes, times and file locks executed from C# on Linux ARM64 |
| Local sockets, accounts, priorities | `local_sockets`, `accounts` and `priority` groups: Linux and host-table providers checked against the kernel, the `std` providers through the C table on Linux and macOS; Unix domain sockets, named pipes with `CurrentUserOnly`, account lookups and `Process.PriorityClass` executed from C# on Linux ARM64 |
| Packet information, raw ICMP, another identity | `packets` and `spawn_as` groups, the raw socket kind and the `DONT_FRAGMENT` and `RECEIVE_ERRORS` options: Linux and host-table providers checked against the kernel, the `std` providers through the C table; `Socket.ReceiveMessageFrom`, `Ping` and `Process.Start` with a user name executed from C# on Linux ARM64 as root |
| Interactive console | `System.Console` executed on a pseudo-terminal on Linux ARM64: window size and resize, key reads, `KeyAvailable`, Ctrl+C as input and as `CancelKeyPress`, line editing, and the terminal restored at exit |
| Watching, mappings, volumes, network | `watches`, `mappings`, `volumes` and `network` groups and seven more socket options: Linux and host-table providers checked against the kernel, the `std` providers through the C table on Linux and macOS; `FileSystemWatcher`, `MemoryMappedFile`, `DriveInfo`, `NetworkInterface`, reverse lookup and IPv4 multicast executed from C# on Linux ARM64 |
| Bare-metal execution | The console, I/O, system and facilities probes executed on `aarch64-unknown-none` under QEMU virt with no OS and no libc, through the bare-metal port, the freestanding C runtime and the source-built runtime |

The workflows `boundary-validation`, `llvm-managed-validation` and
`boundary-sanitizers` must all succeed at the candidate revision. The source-bundle
and developer-input workflows are convenience artifacts, not qualification gates.
A completed table row is not a claim that every method in its subsystem uses Rust.

## What is NOT complete

1. **The rest of the BCL native layer.** Sessions and resource limits are not carried by
   the boundary, and no caller of them was found in the BCL libraries that were read.
   Neither are raw sockets of protocols other than ICMP, control messages other than
   packet information, the abstract socket names of Linux, and network change events
   (the BCL wraps a netlink descriptor in a `Socket` for those). The corresponding
   `SystemNative_*` entry points report `ENOTSUP`, `ENOENT` or `EAFNOSUPPORT`, and a
   program that needs them fails honestly. Gateways, routes and network statistics are
   not groups of the boundary either: the Linux BCL reads them from `/proc` and `/sys`
   through the `files` group, which the facilities probe exercises, and a target without
   those files has none. The cryptography, TLS, globalization and compression native
   libraries of the BCL are outside the boundary. The layer is verified with the five
   probes, not with the BCL's own test suites. The sockets path has carried loopback
   traffic, Unix domain sockets, ICMP echo to the loopback address and multicast inside
   one container only. `System.Console` has run on a pseudo-terminal, not under a
   person's hands. The Windows branches of the desktop `std` port compile and have not
   run; there it has no file mappings, volumes, network information, local sockets,
   accounts, packet information or children under another identity, and its change
   watching compares directory snapshots, as it does on macOS. Raw sockets and children
   under another identity need root, so on macOS only their refusal has run.
2. **Faults and suspension without a signal substrate.** The runtime uses the
   `faults` group on ARM64 only; the x86-64 frame is defined and exercised at the C
   level (under emulation, see [qualification](qualification.md)), but the runtime does
   not read it, so an x86-64 port without `Context` still ends the run on a hardware fault, as does any port with neither capability. A
   stack overflow is not recoverable anywhere. Activation injection is absent
   without `Context`, so suspension relies on preemptive-mode transitions, which a
   cooperative single-core port satisfies and a preemptive multi-core port without
   signals would not.
3. **Arbitrary targets and execution models.** Targets are the CPU architectures
   ILCompiler generates code for. The bare-metal example runs on QEMU's virt
   machine only: no real board, no interrupts, no protection, one core. RISC-V and
   Cortex-M builds compile the boundary only. Existing NativeAOT implementations
   are reused on the validated targets, not replaced by Rust.
4. **Additional Wasm profiles.** The managed tests are single-threaded: WASIp1
   under Node with the published or source-rebuilt runtime, and the browser
   example with the published runtime only. Shared-memory threads, WASIp2
   components, WebAssembly-GC reference objects, files, sockets, DOM access and
   JavaScript interop beyond the five boundary imports are not implemented or
   qualified. Managed OOM/finalizer qualification in Wasm does not yet match the
   wider native suite. The desktop `std` port's tests have been executed by hand on
   Linux and macOS; the `windows-std` workflow job runs them on Windows, where the
   providers of this repository's newer groups were compile-checked only, so that
   job's result belongs to the revision and is not assumed here.
   NUMA-aware heap placement is not exposed by the boundary on any target.
5. **Product qualification.** A maintained upstream release must be selected and
   re-audited, the actual application's needed BCL surface must be tested, and
   longer deployment-specific stress/performance/security qualification is needed.
   The recorded finite tests and ASan runs cannot establish absence of every race,
   undefined behavior or security vulnerability. No production support is claimed.

Do not close these items by renaming them, suppressing tests, returning success
from unsupported operations, or relabeling an archive cross-build as execution.
See [qualification](qualification.md), [architecture](architecture.md) and
[servicing](servicing.md) for precise contracts and reproduction commands.
