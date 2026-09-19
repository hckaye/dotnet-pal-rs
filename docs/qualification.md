# Qualification profiles and evidence

This document describes executable tests, not a claim that all possible .NET or
Rust targets are supported. Every test must pass at the exact published commit.

## Native managed qualification

`source-runtime.sh` builds both native collectors from the audited .NET source,
then `qualify.sh` builds baseline, direct Rust Linux, host-kernel and fault-provider
variants for Workstation GC and Server GC. The actual collector is asserted from
`GCSettings.IsServerGC`; Server GC uses two heaps. CI executes natively on x64
and ARM64, not by treating cross-compilation as execution evidence.

The concurrent stress combines managed allocations, recursive monitors, per-thread
state, GC inside exception filters, throw/rethrow/finally, pinned roots, weak roots,
exact finalizer accounting, and reverse P/Invoke from a native-created thread.
The managed callback catches errors before returning through the C ABI. The
stress duration is configurable with `PAL_STRESS_SECONDS` (1..3600, CI default 10).
A finite stress test is not a proof against all timing-dependent bugs.

OOM qualification retains arrays until a 128 MiB managed heap limit is reached,
checks retained contents, drops references, collects, and verifies allocation
recovery. It repeats three times. A startup preflight checks the effective
`TotalAvailableMemoryBytes`; an ignored heap setting is a failure, not success.
The pinned NativeAOT RhConfig parser accepts hexadecimal digits WITHOUT `0x`.
Limits apply only to child probes, not to the compiler process.

The separate fault binary arms exactly one VM commit failure before any OS side
effect. The GC must encounter it, preserve existing roots, then recover. The fault
control is test-only and is not exported by the PAL library. This does not claim
recovery from every possible partially completed host operation.

## Linear-memory managed qualification

The LLVM experiment uses a different audited compiler/runtime family. Both the
link-interposition experiment and the source-rebuilt native runtime execute the
same C# allocation, root retention, zeroing and exception workload in Node's real
WASIp1 host. Only statistics observers are imported by the C# test. Negative
controls leave Rust/adapter counters zero; positive controls must show real GC
startup storage acquisition, further logical commits and clock activity.

The source configuration contains no `--wrap` helpers. Its native runtime is
rebuilt; the matching published compiler and BCL are not rebuilt. The explicit
linear adapter uses eager storage, not sparse VM. Logical decommit zeros storage,
keeps it accessible, and does not reclaim physical Wasm pages. Its bounded ledger
rejects unknown ranges and partial release. The 256 MiB `linear-gc` arena is an
opt-in test profile; the ordinary linear boundary remains 8 MiB. Neither provides
`memory.grow`, shared-memory threads or WebAssembly-GC reference objects.

## Performance and dependencies

Native qualification records three alternating baseline/candidate trials of the
same managed workload. Reports include wall/user/system time, peak RSS, executable
size and machine metadata. These are measurements, not a universal overhead bound;
compare results on the actual deployment hardware and workload before release.

`audit_dependencies.py` records global unresolved references in the rebuilt
runtime and Rust archive, separately from the final executable's dynamic imports.
It lists remaining direct OS references and unknown symbols needing review.
The tool does not hide those references in a success summary. Its explicit
`--require-isolated` gate fails if such dependencies remain. Archive membership
alone does not establish execution reachability; this inventory is not a syscall
trace or a complete proof of OS isolation.

## Isolation gate, console probe and bare metal

`scripts/source-runtime.sh` ends with `audit_dependencies.py --require-isolated`
over the rebuilt runtime and minipal archives. `scripts/console-probe.sh` publishes
`samples/ConsoleProbe` with those archives and `native/system_native_*.c` in place
of the SDK's System.Native, checks the link map, runs the program and audits all three
archives. `scripts/io-probe.sh` does the same with `samples/IoProbe`, which uses the
BCL's file and socket APIs and dereferences null, and additionally requires the file
and socket entry points to be in the image. `scripts/system-probe.sh` and
`scripts/facilities-probe.sh` do it with `samples/SystemProbe` and
`samples/FacilitiesProbe`. `examples/baremetal-aarch64/build-app.sh`
links one of the four managed objects into a bootable image and runs it under
`qemu-system-aarch64`; the run passes when the probe prints its `PASS` line and the
machine exits with status 0 through semihosting. The evidence is the QEMU serial log
and the link map.

## Files, sockets and faults

`scripts/files.sh` and `scripts/sockets.sh` run `tests/files.c` and `tests/sockets.c`
against the Linux providers and against independent C providers behind host tables.
Every answer is compared with the kernel's own (the same operation through the C
library, `/proc/self/fd`, loopback traffic), every status of the groups that the
environment can provoke is provoked, and a provider that breaks its contract shows
what the front ends sanitize. `scripts/faults.sh` takes real faults through
`tests/faults_host.c`: a null store on two threads resumed at a recovery function
with edited argument registers, an unhandled fault that kills a child by the signal,
and a fault inside the handler. It has run on ARM64 natively and on x86-64 under
Rosetta emulation, not on an x86-64 CPU. `cargo test -p dotnet-pal-memfs` and
`cargo test -p dotnet-pal-std` cover the in-memory and `std::fs` providers through
the negotiated C table.

## System facts, notifications, child processes and the terminal

`scripts/system.sh`, `notifications.sh`, `processes.sh` and `terminal.sh` follow the same
plan: the Linux provider and an independent C provider behind a host table, each compared
with what the C library and the kernel say, and a provider that breaks its contract.
`tests/system.c` compares the environment enumeration, the texts, CPU time, uptime and
user ids with `environ`, `/proc`, `getrusage` and `getpwuid`. `tests/notifications.c`
sends real signals: reports arrive on a thread of the provider's own in ordinary thread
context, a kind that is not enabled keeps the action it had, and the fatal and stopping
default actions are observed in children. `tests/processes.c` starts real children (`sh`,
`cat`, `sleep`, `env`) with pipes, timed waits and both kinds of termination, checks that
a child inherits no descriptor of the parent, and runs a second time with `pidfd_open`
refused by a seccomp filter, as on a kernel before 5.3. `tests/terminal.c` makes its own
pseudo-terminal, puts its slave side on descriptors 0 to 2 before the table is negotiated,
and plays the keyboard from the master side. `scripts/system-probe.sh` runs
`samples/SystemProbe` the way `io-probe.sh` runs the I/O probe.

## Change watching, file mappings, volumes and network information

`scripts/watches.sh`, `mappings.sh`, `volumes.sh` and `network.sh` follow the same plan
again. `tests/watches.c` makes real changes in a scratch directory and keeps an inotify
instance of its own beside the boundary: both must report the same kinds, names and
cookies in the same order, each for its own watch. It also releases a reader that waits
without a limit by removing the watch from a second thread. `tests/mappings.c` checks
that a shared write reaches the file and a second mapping, that a private one reaches
neither, that the mapping outlives the handle, that the bytes after the end of the file
read as zero, and that a handle opened the wrong way is refused. `tests/volumes.c`
compares the mount list with `getmntent` and the figures with `statvfs`; run with the
right to mount, it also lists a tmpfs whose mount point has a space, a tab, a backslash
and a newline in its name. `tests/network.c` compares interfaces and addresses with
`getifaddrs`, `if_nameindex` and `ioctl`, enumerates from two threads at once, looks up
the loopback address and an address without a name, and sends datagrams to a group it
joins and leaves on the container's multicast-capable interface; a datagram sent through
the loopback interface must not reach a member on another one. `tests/sockets.c` sets
each of the seven new socket options, reads it back and checks its effect on real
traffic through the hop limit and the arrival interface the kernel reports. IPv6 group
traffic runs only where the interface has IPv6 switched on, which a default Docker
container has not. `tests/local_sockets.c` binds, connects and talks through Unix domain
sockets in a scratch directory and compares paths and the peer's user with
`getsockname` and `SO_PEERCRED`; as root it checks the refusals a permission causes in a
child that has given its privileges up. `tests/accounts.c` compares every account
`getpwent` lists, by id and by name, and the group lists with `getgroups` and
`getgrouplist`, the first in a child that has set its groups. `tests/priority.c`
compares with `getpriority` and `/proc/<pid>/stat` for this process and for a child with
several threads; lowering a nice value again needs `CAP_SYS_NICE`, which a default
Docker container does not grant, and the test then asserts the refusal.
`tests/packets.c` compares the destination and the arrival interface of datagrams with
its own `recvmsg`, builds an ICMP echo request itself and receives the reply through a
raw socket, and shows `DONT_FRAGMENT` by the DF bit of what goes out. `tests/sockets.c`
shows `RECEIVE_ERRORS` with an unconnected UDP socket that sends to a closed port: the
refusal is dropped without the option and is the status of the next receive with it.
`tests/spawn_as.c` starts children as another user and compares `id` and
`/proc/<pid>/status` with what was asked, checks that the working directory is entered
as the new user, that several threads can start such children at once without any thread
of the parent changing its own ids, and runs a second time without privileges. `scripts/facilities-probe.sh` runs `samples/FacilitiesProbe` the way
`io-probe.sh` runs the I/O probe, and `scripts/terminal-probe.sh` runs
`samples/TerminalProbe` on a pseudo-terminal whose keyboard it plays.

## x86-64 under emulation

No x86-64 machine has run the suites. The same container image built for `linux/amd64`
runs them on an ARM64 host under Rosetta emulation. Seventeen of the nineteen group
suites pass there, among them `faults.sh` and `context.sh`, which take real faults and
signals with the x86-64 frame, and so do the core tests and all tests of the `std` port
and of `dotnet-pal-memfs` except the four named below. What fails is traced to three
behaviours of the emulator, two of which a plain C program without any boundary code
shows as well:

- `posix_spawn` of a program that does not exist returns success and the child exits
  with 127, where the kernel reports `ENOENT`. `processes.sh` and two tests of the `std`
  processes provider expect `NOT_FOUND` there.
- `rt_sigprocmask` with an unmapped pointer ends the process with a segmentation fault,
  where the kernel answers `EFAULT`. The image group's readability probe relies on that
  answer, so `platform.sh` and the `std` port's image test stop there.
- An allocation of 10^15 bytes, which `dotnet-pal-memfs` expects the allocator to refuse,
  ends the process with an error message of the emulator.

These results say that the providers compile and behave on the x86-64 ABI (structure
layouts, signal frames, system call numbers). They do not replace a run on x86-64
hardware. The runtime turns faults into managed exceptions through the `faults` group on
ARM64 only; the conversion for x86-64 is not written, because no x86-64 port without
signals exists to execute it on, and this emulator is no substitute.

## Sanitizer coverage

`sanitize.sh address` and `sanitize.sh thread` use a pinned nightly solely for the
sanitizers, rebuilding Rust core/compiler-builtins and instrumenting Rust and C/C++
boundary callers in the same process, for the architecture of the machine they run
on. They exercise the Linux, host-kernel and linear contracts, and the conformance
test of every Linux provider behind System.Native (files to priorities), with leak
checking in the address run. Both have passed on Linux ARM64 in a container, which
needs the clang sanitizer runtime (`libclang-rt-18-dev`), `--cap-add SYS_PTRACE` and
`--security-opt seccomp=unconfined`. It does not instrument the managed NativeAOT runtime, replace the stable
release compiler, or constitute a complete undefined-behavior proof. Review the
sanitizer workflow result at the exact commit rather than assuming it passed.

See [servicing](servicing.md) for pin upgrades and release evidence requirements.
