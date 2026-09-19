# WebAssembly profiles and the browser host connection

The boundary runs on three kinds of Wasm host. Each one is a distinct Cargo
profile with its own capability bits, and none of them advertises `CAP_VM`.

| Profile | Target | OS services | Storage |
| --- | --- | --- | --- |
| `linear` | `wasm32-unknown-unknown`, `wasm32v1-none`, `wasm32-wasip1` | None; the module imports nothing | 8 MiB static arena |
| `wasi-clock`, `wasi-runtime`, `wasi-dispatch` | `wasm32-wasip1` | Raw WASI Preview 1 imports, or one audited dispatcher | Static arena, or `linear-heap` sharing wasi-libc's allocator |
| `examples/browser-port` (`arena`, `grow`, `heap`) | `wasm32-unknown-unknown`, `wasm32v1-none`, `wasm32-wasip1` | Five typed imports from the page's JavaScript | Static arena, module-owned `memory.grow` pages, or wasi-libc through the C hooks |

The C ABI in `include/dotnet_pal.h` is identical across all three. A consumer
keeps checking `struct_size` and the capability bits before reading a group.
None of these layouts is a WASI Component Model interface or a JavaScript
object contract.

## Capability negotiation

`dotnet_pal_get_api(2)` returns the same append-only table on every profile.
The bits that matter for Wasm are:

| Bit | Contract |
| --- | --- |
| `CAP_VM` (1) | Native reserve/commit with inaccessible pages. Never present on Wasm. |
| `CAP_LINEAR` (2) | Eager, always-accessible storage in 4 KiB units. No protection, no physical decommit. |
| `CAP_DYNAMIC_LINEAR` (262144) | Storage is requested on demand; the module's initial memory stays small. |
| `CAP_CLOCK` (4) | Monotonic nanoseconds from an unspecified epoch. |
| `CAP_ENVIRONMENT`, `CAP_REALTIME`, `CAP_ENTROPY` | Environment snapshot, Unix wall clock in nanoseconds, cryptographic random bytes. |
| `CAP_DIAGNOSTICS` (4194304) | `write_stderr` reaches a host sink. |

The native GC adapter in `crates/dotnet-pal-build/native/gc_vm_adapter.h` requires `CAP_VM` and returns
NULL for a linear-only table; `tests/adapter.cpp` checks that rejection. The
explicit linear GC adapter requires `CAP_LINEAR` and rejects a table that also
claims `CAP_VM`. Checking only the ABI version is not enough on any profile.

Diagnostic counters use pointer-width atomics, so no Wasm profile needs 64-bit
atomics. `read_stats` is present on all of them.

## Browser profile

`examples/browser-port` is a port crate (see [porting](porting.md)) for modules
that a web page instantiates directly. It declares one import module,
`dotnet_pal_browser_v1`, with five functions.
Each takes wasm32 offsets and sizes and returns a `dotnet_pal` status code.

| Import | Backs | Reference host implementation |
| --- | --- | --- |
| `monotonic_ns(out)` | `services.monotonic_ns` | `performance.now()` scaled to nanoseconds, clamped to never decrease |
| `realtime_ns(out)` | `runtime.realtime_ns` | `Date.now()` scaled to nanoseconds since the Unix epoch |
| `random_bytes(out, size)` | `runtime.random_bytes` | `crypto.getRandomValues` in 64 KiB chunks |
| `environment_get(name, len, out, capacity, required)` | `runtime.environment_get` | Lookup in the map the page passed at construction |
| `write_stderr(data, size, written)` | `support.write_stderr` | UTF-8 decode into a sink, `console.error` by default |

`examples/browser-port/host/host.mjs` is that reference host. It uses only Web APIs, so
the same file runs in a browser and in Node. It bounds-checks every pointer
against the live instance memory, rejects shared memory and returns
`INVALID_ARGUMENT` instead of touching memory it cannot verify. That check
protects the page from a broken module; it is not a sandbox for the module's own
memory, which the module can already read and write.

The Rust side treats the host as untrusted for status and output purposes.
Statuses outside the published set become `OS_ERROR`. A failed clock read
leaves zero in the output even if the host scribbled a value. A failed entropy
request zeroes the buffer. A host that reports a short diagnostic write as
success, or an environment result whose required length exceeds the capacity,
is reported as `OS_ERROR`.

Services that a browser cannot honestly provide are absent, not stubbed.
There is no scheduler, because the main thread cannot block and a `sleep_ns`
that returns immediately would be a lie. There is no process or thread
identity, no native memory mapping, no module loading, no native helper heap,
no reader/writer locks and no thread naming. Those callbacks are NULL and the
corresponding bits are clear.

The profile is single-threaded. Compilation with the `atomics` target feature
is rejected, the reference host rejects a `SharedArrayBuffer`, and nothing in
the crate is safe to call from a second Wasm thread.

## Storage in the browser

The `arena` feature keeps the ordinary 8 MiB static arena. The arena is
zero-initialized data, so it costs nothing in the binary, but it is part of the
initial memory of every instance.

The `grow` feature uses `storage::Ledger<storage::Grow>`: pages obtained with
`memory.grow` are handed out from a sorted, coalescing free list under the same
ownership ledger and 8 MiB live-byte budget as the C-hook variant. Freed pages
stay mapped, because Wasm memory never shrinks, and a request that the engine
or a linker `--max-memory` limit cannot satisfy fails with `OUT_OF_MEMORY`
before any side effect. This provider assumes that nothing else grows the same
memory.

The `heap` feature uses `storage::Ledger<storage::Hooks>` with the hooks
implemented by `crates/dotnet-pal-posix/native/linear_heap_posix.c` over wasi-libc's allocator, and a
64 MiB budget. This is the variant linked with the NativeAOT LLVM runtime, whose
libc and BCL already own that allocator.

## What the tests execute

`examples/browser-port/run.sh` builds the `arena` and `grow` variants for both
`wasm32-unknown-unknown` and `wasm32v1-none`, links `tests/browser.c`
freestanding into each module and runs it twice: under Node with
`tests/browser.mjs`, and inside a real headless Chromium through a generated
page. The page embeds the module as base64 and inlines the host and the probe,
so a `file://` load needs no server. The probe writes `BROWSER PASS` plus the
browser's user agent into the DOM, and the script fails unless that line is
present without console errors.

The C test asserts the exact capability set, that every absent callback is
NULL, nondecreasing clock readings, a wall clock after the year 2000, entropy
above the 64 KiB chunk boundary, environment lookups with too-small buffers,
empty values and missing names, diagnostic output, and the storage contracts.
The `grow` module is linked with a 6 MiB memory maximum, so the test
observes a 3 MiB allocation growing memory, a request over the 8 MiB budget
failing before the provider, a 4 MiB request failing inside `memory.grow`
without disturbing existing data, and first-fit reuse of a freed block.

Five further instances inject host failures: scribbled clock outputs,
partially filled entropy, an unknown status, a short diagnostic write reported
as success, and a required length above the capacity. Each must surface as the
documented error with sanitized outputs. `tests/browser_host.test.mjs` unit-tests
the JavaScript host against a raw `WebAssembly.Memory` for bounds, alignment,
shared-memory rejection, chunking and status mapping.

```sh
rustup target add wasm32-unknown-unknown wasm32v1-none
bash examples/browser-port/run.sh
```

The script needs Clang with a Wasm backend, `wasm-ld`, Node and a Chromium-based
browser. It looks for `google-chrome`, `chromium`, a Playwright
`chrome-headless-shell` or macOS Google Chrome; set `BROWSER_BIN` to override.
A missing browser is a failure, not a skipped step.

## C# in the browser

`examples/browser-port/app` is a C# program compiled by the experimental
NativeAOT LLVM toolchain for `wasi-wasm` and linked against the `heap` variant
of the port plus the adapters that `build.rs` produces through
`dotnet-pal-build` (the GC `--wrap` definitions, `minipal` entropy, the errno
formatter and the storage hooks). The module imports two host modules: the five
boundary functions and the WASI Preview 1 subset that wasi-libc and the BCL
use. `host/wasi.mjs` provides that subset in a page: arguments, environment,
clocks, entropy, console output, a spinning `poll_oneoff` for clock waits, and
"not available" errors for files and sockets.

`build-app.sh` runs on Linux (the compiler host pack exists for Linux and
Windows only; `integration/llvm-wasi/container` provides the toolchain) and
executes the module under Node; `run-app.sh` executes the generated page in
headless Chromium. The program asserts from inside managed code that GC storage
and the runtime clock crossed the Rust boundary, that finalizers, exception
filters, entropy, the wall clock and console output work, and the runner
asserts that the page never had to open a file.

Threads, WebAssembly GC reference objects, WASIp2 components, DOM or network
access, file systems and any JavaScript interop beyond the five imports remain
out of scope. The source-rebuilt runtime of the WASI pipeline has not been
linked against this port. The readiness notes list these as open work.

## History

The first Wasm proposal for this repository (pull request #1) introduced a
weaker linear-memory capability separate from `CAP_VM`, counters that do not
require 64-bit atomics, execution of the real table in Node for two Wasm
targets, a regression test that the native GC adapter rejects linear storage,
and a host-initialized arena entry point. All but the last are now in the main
tree under their current names (`CAP_LINEAR`, `counter.rs`, `scripts/wasm.sh`,
`tests/adapter.cpp`). The host-initialized arena was replaced by the versioned
storage hooks of `linear-heap`, which cover the same need without a second
runtime-facing entry point, and by the `storage::Grow` provider above.
