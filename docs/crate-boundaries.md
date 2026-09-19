# Crate boundaries and consumer migration

## Dependency direction

```text
custom port ──────────────────────────────> dotnet-pal-rs (no_std, no dependencies)
Linux / macOS / Windows / WASI / host ────> dotnet-pal-rs
storage / memory file system ─────────────> dotnet-pal-rs
Wasm growth ──> portable storage ─────────> dotnet-pal-rs
native build ──> dotnet-pal-build ─────────> dotnet-pal-rs + cc
                    └─ optional posix ───> dotnet-pal-posix (C source assets)
```

The desktop implementations additionally share `dotnet-pal-std-common`, a
macro-only, dependency-free package containing portable std service implementations.
It contains no OS-specific implementation. Each OS crate owns its remaining
source files: there are no path inclusions, symlinks or build-time generation
that reach into another platform's source directory.

`dotnet-pal-std` retains the old name and Rust `Std`/`StdPort`/`api()` interface,
but now re-exports one target-specific dependency. Direct OS packages are the
preferred dependencies for platform-specific applications. Selecting the Linux
implementation does not compile or link the macOS or Windows implementations.
Cargo's workspace lockfile can mention all members; that is not the active
dependency graph of a downstream consumer.

## Package ownership

| Concern | Package / location |
| --- | --- |
| Service traits, statuses, capability bits, ABI, validators | `dotnet-pal-rs` |
| C contract header | Core's `include/dotnet_pal.h` |
| .NET bridge C/C++ sources and build helper | `dotnet-pal-build` (use as a build dependency) |
| Linux libc-backed no_std implementation | `dotnet-pal-linux` |
| Linux desktop std implementation | `dotnet-pal-linux-std` |
| macOS desktop implementation | `dotnet-pal-macos` |
| Windows desktop implementation | `dotnet-pal-windows` |
| Caller-supplied C host tables | `dotnet-pal-host` |
| WASI Preview 1 services and transport | `dotnet-pal-wasip1` |
| Wasm linear memory.grow | `dotnet-pal-wasm` |
| Portable arena / backing ledger / foreign storage hooks | `dotnet-pal-storage` |
| In-memory file system | `dotnet-pal-memfs` |
| C POSIX reference helper implementations | `dotnet-pal-posix`, opt-in |
| Browser / bare-metal boards | Separate example crates |
| Test profile assembly, old backend feature vocabulary | `tools/dotnet-pal-standalone`, `publish = false` |
| Runtime source patchers, compiler pinning and qualification scripts | Repository development tools under `integration/` and `scripts/` |

Bridge headers are found through the core package's `C_INCLUDE_DIR`. Bridge
sources are found relative to the build package's own manifest directory, not
`../..`. A consuming build does not need this repository's native directory or
a checked-out .NET runtime just to compile an adapter. Building the runtime
itself still requires the appropriate pinned runtime sources and toolchains.

## Migration from the old root package

| Old selection | New consumer selection |
| --- | --- |
| core `features = ["linux"]` | depend on `dotnet-pal-linux`; use `Linux` or `LinuxPort` |
| core `features = ["host-kernel", ...]` | depend on `dotnet-pal-host` with the corresponding explicit groups |
| core `features = ["storage-arena"]` | depend on `dotnet-pal-storage` with `storage-arena` |
| `dotnet_pal_rs::storage::Arena/Ledger/Hooks` | `dotnet_pal_storage::Arena/Ledger/Hooks` |
| `dotnet_pal_rs::storage::Grow` | `dotnet_pal_wasm::Grow` |
| `wasi-clock` / `wasi-runtime` | compose `dotnet-pal-wasip1` service providers and explicit storage |
| desktop `dotnet-pal-std` | retained facade, or select the concrete OS crate directly |
| native adapter paths at repo `native/` | `crates/dotnet-pal-build/native/` |
| C POSIX helpers bundled in the bridge | enable `dotnet-pal-build/posix` only when selected |

Host ports must validate their negotiated foreign tables in `Port::validate`:
`dotnet_pal_host::validate::<true>()` requires sparse VM, while `<false>()` is for
a composing port with its own linear storage. Group features still control which
host services are provided. `define_pal!` does not automatically validate foreign
host initialization; a composing host port should implement `Port` explicitly,
as the qualification assembler demonstrates.

This is a Rust package/source-layout change. C ABI layout, status values and
capability bits do not change. No version bump is asserted for unreleased packages.
Existing native callers still negotiate `dotnet_pal_get_api(2)`.

## No implicit global runtime ownership

Reusable ports have empty default feature sets. Enable `entry` only on the one
selected platform that owns the exported C symbol (and, for the no_std Linux
archive, its panic policy). A custom application can instead compose providers
and define the entry point and panic/allocator policy itself. Core never installs
an allocator, handler or platform implementation. Portable arena storage and
POSIX helper sources are opt-in as well.

## Verification

`python3 scripts/check-crate-boundaries.py` creates consumers outside the workspace,
checks their resolved dependency graphs for several targets, inspects Cargo's
package file lists, builds a no_std core-only consumer, and compiles the C bridge
using only relocated package-listed files. It also verifies explicit POSIX opt-in.
`tests/test_crate_boundaries.py` protects the structural contract in ordinary tests.
Existing C ABI, managed, Wasm, sanitizer and bare-metal qualification runs target
the same providers via the unpublished assembler; no assertion is removed.

A repository checkout and `cargo test --workspace` intentionally include development
members. Use `cargo test` for core only, or select a package with `-p`. There is no
meaningful `--workspace --all-features` configuration: the test assembler has
mutually exclusive profiles. This split is not itself a crates.io publication or
a new claim about production readiness of any port.
