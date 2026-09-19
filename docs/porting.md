# Porting .NET NativeAOT to a new platform with dotnet-pal-rs

A port is a Rust crate that depends on `dotnet-pal-rs`, implements service
traits and exports the negotiated C entry point. Core is always platform-neutral:
there is no backend feature that changes its dependency graph or adds OS code.

## Crates

[Crate boundaries and migration](crate-boundaries.md) lists all packages and
examples. Use `dotnet-pal-linux`, `dotnet-pal-linux-std`, `dotnet-pal-macos`,
`dotnet-pal-windows`, `dotnet-pal-host` or `dotnet-pal-wasip1` only when that provider
is wanted. `dotnet-pal-std` is a small target-selected compatibility facade.
Portable storage is in `dotnet-pal-storage`, Wasm growth in `dotnet-pal-wasm`, and
native adapters are packaged with the build-only `dotnet-pal-build` crate.

## Writing a port

Implement the traits in `dotnet_pal_rs::port` for the capabilities the platform
has. Every trait has a `PROVIDED` constant that defaults to `true`; the marker
type `Absent` implements every trait with `PROVIDED = false`. A capability left
absent is reported to the runtime as a clear capability bit and a NULL callback.
There is no way to make an absent service look successful.

```rust
#![no_std]
use dotnet_pal_rs::port::{self, Error, Result};

struct MyPlatform;
impl port::Clock for MyPlatform {
    fn monotonic_ns() -> Result<u64> { my_os::ticks_ns().ok_or(Error::Os) }
}
impl port::Diagnostics for MyPlatform {
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
        let bytes = unsafe { core::slice::from_raw_parts(data, size) };
        my_os::debug_write(bytes).map_err(|written| (written, Error::Os))
    }
}

dotnet_pal_rs::define_pal! {
    Linear = dotnet_pal_storage::Arena,   // optional dotnet-pal-storage dependency
    Clock = MyPlatform,
    Diagnostics = MyPlatform,
    Abort = MyPlatform,                       // or omit for a trap/spin
}
dotnet_pal_rs::define_panic_handler!(MyPlatform);
```

`define_pal!` declares a port type from `Capability = Provider` pairs (anything
unlisted is `Absent`) and exports `dotnet_pal_get_api`. The runtime negotiates
once; the table is then immutable for the process or instance lifetime. A port
can reject negotiation from `Port::validate` when a required host configuration
is malformed, which is how the built-in C-table providers behave.

The capability keys and the traits behind them:

| Key | Trait | Runtime service |
| --- | --- | --- |
| `VirtualMemory` | `VirtualMemory` | GC reserve/commit/decommit with inaccessible reservations |
| `Linear` | `LinearStorage` | Eager storage in 4 KiB units (Wasm and freestanding targets) |
| `Clock`, `Scheduler` | `Clock`, `Scheduler` | Monotonic time; blocking sleep and yield |
| `Events`, `Mutexes`, `Threads`, `ThreadLocal`, `StackBounds`, `ProcessBarrier` | same names | Runtime synchronization, threads, TLS, stack limits, membarrier |
| `Environment`, `Identity`, `Realtime`, `Entropy`, `NativeMapping`, `Modules` | same names | Environment, process/thread ids, wall clock, random bytes, native mappings, module loading |
| `NativeHeap`, `RwLocks`, `ThreadName`, `Diagnostics` | same names | Helper heap, reader/writer locks, thread names, fatal output |
| `Topology` | `Topology` | CPU counts and affinity, memory figures, cache size, CPU feature words |
| `Process` | `Process` | Orderly exit, debugger presence, crash-dump utility launch |
| `Image` | `Image` | Unwind tables of the image containing an address, readability probes, build ids |
| `Streams` | `Streams` | The three standard streams |
| `Files` | `Files` | Files and directories behind the BCL's file APIs |
| `Sockets` | `Sockets` | TCP and UDP sockets, readiness polling, name resolution |
| `Faults` | `Faults` | CPU faults reported by the port's own trap path; see below |
| `SystemInfo` | `SystemInfo` | The environment as an enumeration, the executable's path, OS texts, the user, CPU time and uptime |
| `Notifications` | `Notifications` | Requests from outside the process: the interrupt key, a request to terminate, a resized terminal window |
| `Processes` | `Processes` | Child processes with pipes to their standard streams |
| `Terminal` | `Terminal` | Window size, line and raw input, readiness of input, editing characters |
| `Watches` | `Watches` | Changes to files and directories, queued for a reader |
| `Mappings` | `Mappings` | Files mapped into memory; provided by the type that provides `Files` |
| `Volumes` | `Volumes` | Mount points, capacity, free space and format of a volume |
| `Network` | `Network` | Network interfaces, reverse lookup, multicast membership; provided by the type that provides `Sockets` |
| `LocalSockets` | `LocalSockets` | Unix domain sockets by path and the user at their other end; provided by the type that provides `Sockets` |
| `Accounts` | `Accounts` | Users by id and by name, group lists |
| `Priority` | `Priority` | Scheduling priority of a process |
| `Packets` | `Packets` | The interface and destination address of a received datagram; provided by the type that provides `Sockets` |
| `SpawnAs` | `SpawnAs` | A child started as another user; provided by the type that provides `Processes` |
| `Context` | `SignalContext` | Architecture-bound signal substrate (raw table); optional, see below |
| `Wasi` | `WasiTransport` | The single-import WASIp1 transport |
| `Abort` | `Abort` | Termination after an internal invariant failure |

Provider functions receive raw pointers exactly as the C ABI passes them. The
front ends have already checked NULL, alignment, overflow and size rules and
will re-check results (a reserve that returns an unaligned pointer, an
environment lookup that reports more bytes than the buffer holds). What a
provider cannot know, such as whether a handle is still alive, remains the
caller's contract as documented in `include/dotnet_pal.h`.

The source-integrated runtime requires `VirtualMemory`, `Clock`, `Scheduler`, the
kernel group, `Identity`, `Realtime`, `NativeMapping`, the support group,
`Topology` and `Image`. `Environment`, `Entropy`, `Modules`, `Process` and
`Context` may be absent: variables read as unset, there is no entropy source, no
module loads and dumps are unavailable. Without `Context` the runtime installs no
signal handlers. A port that owns a trap path provides `Faults` instead: its
handler builds a `faults::Frame` and calls `faults::deliver`, and the runtime
turns a null dereference in managed code into `NullReferenceException` (on ARM64;
elsewhere, and without either capability, a hardware fault ends the run). The
bare-metal example is such a port. `Streams`, `Files`, `Sockets` and the thirteen
groups after `Faults` are what the boundary's System.Native builds the BCL's
console, file, network, process and environment APIs on; a program that uses none
of them needs none of them. See [io](io.md), [system](system.md) and
[facilities](facilities.md).

## The BCL native layer and the C runtime

A NativeAOT program also links System.Native, the BCL's native layer.
Five units implement it on the boundary. `crates/dotnet-pal-build/native/system_native_pal.c` has the
native heap, threads, monitors, clocks, entropy, the environment lookup and error
codes. `system_native_io.c` has descriptors, streams, files and directories,
change watching, file mappings and volumes. `system_native_net.c` has sockets,
readiness events, name resolution and network interfaces. `system_native_sys.c`
has system facts, signal registrations, the terminal and module loading.
`system_native_proc.c` has child processes and their pipes. What the boundary
does not carry reports `ENOTSUP`, `ENOENT` or `EAFNOSUPPORT`. A port compiles the
units against Linux headers, because the errno values and the runtime archive
follow that ABI, and links them in place of the SDK's `libSystem.Native.a`. A
port without storage hardware can name `dotnet_pal_memfs::MemFs` as its `Files`
and `Volumes` provider.

A target with no libc links `crates/dotnet-pal-build/native/freestanding`, the C runtime contract the
runtime archives and System.Native need, and defines the two hooks
`dotnet_pal_freestanding_abort` and `dotnet_pal_freestanding_exit`. The math
library comes from the port (the bare-metal example exports the pure-Rust `libm`).
See [platform](platform.md).

## Storage without an OS

`dotnet-pal-storage` supplies portable providers with no OS dependency:

- `Arena` (feature `storage-arena`): a bounded static arena, reusable, part of
  the initial memory. 8 MiB by default, 64 MiB with `linear-gc-small`, 256 MiB
  with `linear-gc`.
- `Ledger<B>`: an ownership ledger with a hard live-byte budget over a
  `Backing` that supplies blocks on demand. `Hooks` (feature `storage-hooks`)
  calls the C hooks `dotnet_pal_storage_allocate_v2`/`release_v2`, which an
  embedder implements with its allocator (`crates/dotnet-pal-posix/native/linear_heap_posix.c` uses
  `posix_memalign`). `dotnet_pal_wasm::Grow` (a separate wasm32-only package) grows the
  instance memory itself and is only correct when no other allocator shares it.

None of these advertise `CAP_VM`; the native GC adapter refuses them and the
explicit linear GC adapter accepts them.

## The build.rs helper

The native side of a port consists of small C/C++ adapters that connect the
audited runtime sources to the boundary (GC wrappers, entropy for `minipal`,
storage hooks). `dotnet-pal-build` compiles the ones a port selects, from the
sources shipped inside its own package, into one archive and emits the Cargo link
directives:

```rust
// build.rs; this example requires dotnet-pal-build features = ["posix"]
fn main() {
    let artifacts = dotnet_pal_build::Build::new()
        .target(dotnet_pal_build::Target::Wasm32Wasip1)
        .adapter(dotnet_pal_build::Adapter::LlvmGcLinear { observer_only: false })
        .adapter(dotnet_pal_build::Adapter::MinipalEntropy)
        .adapter(dotnet_pal_build::Adapter::P1ErrorText)
        .adapter(dotnet_pal_build::Adapter::LinearHeapPosix)
        .compile()
        .expect("native adapters");
    dotnet_pal_build::write_nativeaot_props(
        std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("port.props"),
        std::path::Path::new("target/wasm32-wasip1/release/libmy_port.a"),
        &artifacts, &["-Wl,--max-memory=134217728"]).unwrap();
}
```

`write_nativeaot_props` produces an MSBuild file with `NativeLibrary` and
`LinkerArg` items; a NativeAOT project imports it so `dotnet publish` links the
port and its adapters. The compiler for the adapters comes from the usual `cc`
environment variables (`CC_wasm32-wasip1` and friends), which a port's
`build.rs` can point at the WASI SDK. The helper never downloads toolchains.

## The desktop port

`dotnet-pal-std` selects the current OS implementation; its
integration test exercises the negotiated C table on whatever host runs
`cargo test -p dotnet-pal-std`: reserve/commit/zero-recommit, sleep and clocks,
events with timeouts, recursive mutexes, threads, thread-local destructors,
stack bounds, environment, entropy, native mappings, module inspection, the
helper heap, reader/writer locks and diagnostics. With the `entry` feature the
crate exports `dotnet_pal_get_api` and links into a NativeAOT runtime build the
same way the Linux reference configuration does. The repository CI checks the concrete OS providers and the facade together;
inspect the results for the exact commit before treating a target as qualified.

## Standalone configurations

The `linux`, `host*`, `linear*` and `wasi*` features of `tools/dotnet-pal-standalone` assemble a
static library from separate provider dependencies and export the entry point themselves;
they exist for the qualification scripts and for C/C++ SDK providers that supply
the `dotnet_pal_host_*_v2` tables. Build them with an explicit crate type:

```sh
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --features linux
cargo rustc -p dotnet-pal-standalone --lib --crate-type staticlib --release --no-default-features --features linear --target wasm32-unknown-unknown
```

A library consumer enables none of these features.

## Migrating old root-crate features

The core no longer accepts `linux`, `host-*`, `linear-*`, `storage-*`, or `wasi-*`
features. Select the owning package explicitly; see [the migration table](crate-boundaries.md).
The old names remain only on the unpublished repository qualification assembler.
Its archive is `libdotnet_pal_standalone.a`. They are not a supported way for a
consumer to select platforms through the core.
