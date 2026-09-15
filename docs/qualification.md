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

## Sanitizer coverage

`sanitize.sh address` uses a pinned nightly solely for AddressSanitizer, rebuilding
Rust core/compiler-builtins and instrumenting Rust and C/C++ boundary callers in
the same process. It exercises Linux, host-kernel and linear contracts with leak
checking. It does not instrument the managed NativeAOT runtime, replace the stable
release compiler, or constitute a complete undefined-behavior proof. Review the
sanitizer workflow result at the exact commit rather than assuming it passed.

See [servicing](servicing.md) for pin upgrades and release evidence requirements.
