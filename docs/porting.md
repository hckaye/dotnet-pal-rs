# Porting .NET NativeAOT to a new platform with dotnet-pal-rs

A port is a Rust crate that depends on `dotnet-pal-rs`, implements the trait
for each OS service the platform can provide, and exports the single C entry
point the runtime calls. The core crate contains no OS code of its own when used
this way: it validates arguments, sanitizes outputs and counts calls around the
port's providers, and ships the pure pieces every port reuses (linear storage,
counters, the WASI transport).

## Crates

| Crate | Role | `std` |
| --- | --- | --- |
| `dotnet-pal-rs` | Boundary traits (`port`), C ABI types, front ends, pure providers (`storage`), optional built-in Linux/host/WASI providers | no |
| `dotnet-pal-std` | A complete desktop port built on the Rust standard library (Linux, macOS, Windows) | yes |
| `dotnet-pal-build` | `build.rs` helper: compiles the native adapter objects and writes MSBuild link inputs | yes (build time only) |
| `examples/browser-port` | A port whose services are JavaScript imports, with a C# application running in a page | no |

`dotnet-pal-std` is a separate crate so that `no_std` ports never pull the
standard library, `libc` or `getrandom` into their dependency graph.

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
    Linear = dotnet_pal_rs::storage::Arena,   // pure, bounded storage from the core
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
| `Context` | `SignalContext` | Architecture-bound signal substrate (raw table) |
| `Wasi` | `WasiTransport` | The single-import WASIp1 transport |
| `Abort` | `Abort` | Termination after an internal invariant failure |

Provider functions receive raw pointers exactly as the C ABI passes them. The
front ends have already checked NULL, alignment, overflow and size rules and
will re-check results (a reserve that returns an unaligned pointer, an
environment lookup that reports more bytes than the buffer holds). What a
provider cannot know, such as whether a handle is still alive, remains the
caller's contract as documented in `include/dotnet_pal.h`.

## Storage without an OS

`dotnet_pal_rs::storage` has three providers that need no OS:

- `Arena` (feature `storage-arena`): a bounded static arena, reusable, part of
  the initial memory. 8 MiB by default, 64 MiB with `linear-gc-small`, 256 MiB
  with `linear-gc`.
- `Ledger<B>`: an ownership ledger with a hard live-byte budget over a
  `Backing` that supplies blocks on demand. `Hooks` (feature `storage-hooks`)
  calls the C hooks `dotnet_pal_storage_allocate_v2`/`release_v2`, which an
  embedder implements with its allocator (`native/linear_heap_posix.c` uses
  `posix_memalign`). `Grow` (feature `storage-grow`, wasm32 only) grows the
  instance memory itself and is only correct when no other allocator shares it.

None of these advertise `CAP_VM`; the native GC adapter refuses them and the
explicit linear GC adapter accepts them.

## The build.rs helper

The native side of a port consists of small C/C++ adapters that connect the
audited runtime sources to the boundary (GC wrappers, entropy for `minipal`,
storage hooks). `dotnet-pal-build` compiles the ones a port selects, from the
sources shipped in this repository, into one archive and emits the Cargo link
directives:

```rust
// build.rs of a port crate
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

`dotnet-pal-std` implements every provider a desktop OS can supply. Its
integration test exercises the negotiated C table on whatever host runs
`cargo test -p dotnet-pal-std`: reserve/commit/zero-recommit, sleep and clocks,
events with timeouts, recursive mutexes, threads, thread-local destructors,
stack bounds, environment, entropy, native mappings, module inspection, the
helper heap, reader/writer locks and diagnostics. With the `entry` feature the
crate exports `dotnet_pal_get_api` and links into a NativeAOT runtime build the
same way the Linux reference configuration does. The Windows providers compile
(`cargo check --target x86_64-pc-windows-gnu`) but have not been executed here.

## Standalone configurations

The `linux`, `host*`, `linear*` and `wasi*` features of the core assemble a
static library from built-in providers and export the entry point themselves;
they exist for the qualification scripts and for C/C++ SDK providers that supply
the `dotnet_pal_host_*_v2` tables. Build them with an explicit crate type:

```sh
cargo rustc --lib --crate-type staticlib --release --features linux
cargo rustc --lib --crate-type staticlib --release --no-default-features --features linear --target wasm32-unknown-unknown
```

A library consumer enables none of these features.
