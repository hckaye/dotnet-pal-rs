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

The workflows `boundary-validation`, `llvm-managed-validation` and
`boundary-sanitizers` must all succeed at the candidate revision. The source-bundle
and developer-input workflows are convenience artifacts, not qualification gates.
A completed table row is not a claim that every method in its subsystem uses Rust.

## What is NOT complete

1. **Whole-runtime OS isolation.** Current native archive inventories still expose
   direct OS references outside the boundary. These include signal/context and
   activation handling, module inspection/loading, remaining container/NUMA policy I/O,
   crash-dump/diagnostic I/O and native allocation. Machine measurements and
   affinity are routed, with a separate member-level no-bypass gate. The strict
   `audit_dependencies.py --require-isolated` gate is expected to reject this
   state. A successful reporting run must not be described as an isolation pass.
2. **All BCL native dependencies.** File, network, cryptography, process and other
   native shims have not all been rerouted to a platform-independent callback
   surface. Existing Linux or WASI services remain dependencies of the tested
   configurations. This is not a replacement for a complete target runtime pack.
3. **Arbitrary targets and execution models.** Freestanding ARM64/RISC-V/Cortex-M
   tests build the boundary only. Device linkage/startup, target code generation,
   exception/unwind metadata, register-context/stack-map adaptation, hardware fault
   behavior and packaging remain target-port obligations. Existing NativeAOT
   implementations are reused on the validated targets, not replaced by Rust.
4. **Additional Wasm profiles.** The managed tests are single-threaded: WASIp1
   under Node with the published or source-rebuilt runtime, and the browser
   example with the published runtime only. Shared-memory threads, WASIp2
   components, WebAssembly-GC reference objects, files, sockets, DOM access and
   JavaScript interop beyond the five boundary imports are not implemented or
   qualified. Managed OOM/finalizer qualification in Wasm does not yet match the
   wider native suite. The desktop `std` port executes its table contracts on Linux, macOS
   and Windows. This does not qualify a source-rebuilt Windows managed runtime.
5. **Product qualification.** A maintained upstream release must be selected and
   re-audited, the actual application's needed BCL surface must be tested, and
   longer deployment-specific stress/performance/security qualification is needed.
   The recorded finite tests and ASan runs cannot establish absence of every race,
   undefined behavior or security vulnerability. No production support is claimed.

Do not close these items by renaming them, suppressing tests, returning success
from unsupported operations, or relabeling an archive cross-build as execution.
See [qualification](qualification.md), [architecture](architecture.md) and
[servicing](servicing.md) for precise contracts and reproduction commands.
