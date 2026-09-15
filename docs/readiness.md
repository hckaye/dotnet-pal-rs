# Readiness gates

The supported unit of evidence is a **configuration at a specific commit**, not
"every Rust target". CI must fail closed; compilation and runtime execution have
separate labels. No production readiness or complete NativeAOT port is claimed.

## Current acceptance gates

- Native Linux VM semantics and both backend paths pass C ABI tests and real GC
  baseline/positive controls on x64 and ARM64.
- The source adapter rebuilds the native NativeAOT component on x64 and runs the
  workload without linker wrapping. Published compiler/BCL remain pinned.
- Linear storage has distinct capabilities and is refused by the VM-only adapter.
  Its C-to-Rust suite executes on three Wasm target builds and natively.
- Freestanding host archives build on ARM64, RISC-V64 and Cortex-M (32-bit atomics).
  This does not include board execution or unresolved host callback final linkage.

Read the workflow conclusion/logs for the exact commit to determine whether these
gates passed. Defining a gate is not evidence that it passed.

## Remaining product qualification work

1. Inventory all OS dependencies for a selected runtime feature profile, then
   route and test each supported group. Fail startup when required capabilities
   are missing instead of treating them as optional successful no-ops.
2. Define a GC adaptation for linear memory, covering memory limits, page model,
   GC metadata, stack roots and allocation failure. Running a Rust arena in Wasm
   does not validate a managed GC or NativeAOT Wasm code generation.
3. Add long-running/low-memory/fault-injection GC tests, finalizer and thread
   interactions, crash analysis and per-architecture source-rebuild coverage.
4. Audit host callback ownership, error side effects, ABI sizes and startup order.
   C pointers cannot be made safe by simple null/alignment checks. The linear
   arena rejects non-owned geometry, but cannot detect address reuse/ABA.
5. Measure overhead, code size, native memory high-water marks and contention for
   an actual workload. The bounded 8 MiB arena is a validation profile, not a
   general-purpose runtime heap or an automatically expanding allocator.
6. Track upstream servicing and re-audit pins. The research .NET 10.0.0 and Rust
   1.85.1 pins are not a recommendation to deploy old versions in production.

Not covered: shared-memory Wasm, WASIp2 components, WebAssembly GC reference
objects, runtime EH/stack-map/codegen adaptation, interrupt safety, a sandbox for
untrusted C callers, or a stable vendor-neutral ABI outside the documented groups.
