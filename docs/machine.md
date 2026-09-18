# Machine measurements and CPU placement

The append-only `machine` ABI group uses `CAP_MACHINE`. A Rust port supplies the
`Machine` trait or selects `Absent`; `declare_port!` defaults to `Absent`. Manual
`Port` implementations must add `type Machine = ...`. Existing C table-prefix
sizes and every older field offset stay unchanged. No Linux selector, `cpu_set_t`,
`rlimit` or `sysinfo` layout crosses the public boundary.

Measurements use bytes or CPU indices. `ADDRESS_LIMIT` returns `UINT64_MAX` for
an unlimited virtual address space; unsupported cache queries return Unsupported.
CPU queries distinguish online count from the maximum possible index plus one:
sparse Linux CPU lists cannot safely be sized using a population count. The
reference provider supports indices 0 through 65535 and rejects oversized or
unreadable possible-CPU descriptions rather than allocating an undersized mask.

Process affinity means the process leader's allowed CPU set. Lists are sorted,
unique indices, not native-endian bit masks. `BUFFER_TOO_SMALL` reports a required
element count and returns no partial list. Sizing and copying are separate
snapshots; callers retry boundedly if administrative changes grow the set. A bind
applies only to the calling thread. Invalid geometry/overlapping outputs are
rejected without access; valid output buffers are sanitized on provider failures.
Pointers remain trusted native caller borrows, not sandboxed addresses.

The native source adapter routes CPU discovery/placement, physical/page/cache
measurements, available swap and virtual address limits through this group.
Existing cgroup quota/memory-limit parsing and explicit .NET configuration
precedence are retained; this is not an implementation of all container policy
in Rust. The Linux reference and the independent C host provider implement the
same contract. `host-machine` requires an immutable `dotnet_pal_host_machine_v2`
table before negotiation. Existing `host` builds need no new host symbol.

Run `scripts/machine.sh` for real Linux query/affinity checks and binding on a
separate worker, never on the test's main thread. The Rust core tests exercise
malformed output lengths, duplicates, invalid CPUs, overlap, unsupported queries
and failure sanitization. Source-built GC probes read only counters; a baseline
must remain zero while a routed runtime must show query and affinity calls.
Both sanitizer configurations include the Linux and host machine tests.

These functions are not async-signal-safe. A machine capability and an archive
cross-build do not supply target code generation, exception metadata, or a full
NativeAOT port. Exact-commit CI logs determine qualification, not this document.
