# Readiness gates

The supported unit of evidence is a **configuration at a specific commit**, not
"every Rust target". Compilation and runtime execution have separate labels.
No production readiness or complete NativeAOT port is claimed.

## Current acceptance gates

- Linux x64/ARM64 VM semantics, both backends, and actual GC baseline/positive controls.
- Rebuilt native NativeAOT runtime on x64/ARM64, with VM and clock/scheduling source
  adapters and both Linux/host-services implementations. No --wrap in these runs.
  Published compiler/BCL remain pinned. Clock counts must grow from GC execution;
  VM-only/baseline service counters must stay zero. Sleep/yield usage is not forced
  in GC probes and is tested separately through the C ABI.
- Distinct linear storage capability rejected by the VM adapter; C-to-Rust tests
  execute on three Wasm target builds and natively.
- Real WASIp1 clock import, absent scheduling callbacks, missing-import rejection,
  isolated failure injection, output sanitization and exact import allowlist.
- Legacy VM host linkage remains valid. Optional host-services rejects malformed
  tables and sanitizes failures. Linux sleep retries interruption, clocks remain
  monotonic, and concurrent service calls preserve counters.
- ARM64, RISC-V64 and Cortex-M freestanding host/host-services archives build without
  std, alloc or 64-bit atomic requirements. Not board execution or final linkage.

Workflow conclusions/logs for the exact commit determine which gates passed.
A configured test is not evidence of a successful execution.

## Remaining product qualification

1. Inventory OS dependencies for a selected runtime profile; route synchronization,
   threads/TLS, suspension, EH and required BCL shims through audited capabilities.
2. Adapt a managed GC/runtime for linear memory, including memory limits, roots,
   metadata and allocation failure. Wasm boundary tests do not supply codegen.
3. Add sustained/low-memory/GC fault-injection and finalizer stress, and failure
   diagnosis across supported runtime configurations (not only workstation GC).
4. Audit ownership, startup/reentry and pointer lifetimes; geometry checks cannot
   make foreign pointers safe or detect ABA/reuse of released addresses.
5. Measure workload overhead, code size, memory pressure and contention. Counters
   add runtime cost. The bounded 8 MiB arena is a validation profile, not a general
   runtime heap. Clock accuracy and sleep wakeup latency remain host-dependent.
6. Define servicing and re-audit upstream pins. .NET 10.0.0 and Rust 1.85.1 are
   research reproducibility pins, not production deployment recommendations.

Not covered: shared-memory Wasm, WASIp2 components, WebAssembly GC reference
objects, full runtime/ABI/codegen adaptation, interrupt safety, sandboxing of
untrusted C callers, or certification of any platform.
