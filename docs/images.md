# Loaded-image metadata boundary

`images` is an append-only ABI 2 capability group for **ELF64 little-endian**
metadata enumeration and address-to-symbol information. It does not pretend that
ELF program headers are Mach-O/COFF headers, and does not claim code generation
for another architecture. Linux x64/ARM64 supply the implementation. `host-images`
allows a compatible host to supply immutable callbacks; freestanding archive
compilation does not supply an executable loader or exception ABI.

The provider borrows loaded-image names and program-header arrays while holding
the platform loader's enumeration synchronization. A visitor is synchronous and
must not load/unload modules, retain the borrowed view or unwind through the C
ABI. Early nonzero callback returns are preserved exactly. Continued callbacks
after an early stop, malformed views, unknown formats and contradictory provider
results are rejected. Name storage includes a readable NUL terminator at the
advertised length. As elsewhere, pointer geometry checks do not sandbox C callers.

The view includes loader generation counters for libunwind's cache invalidation.
The NativeAOT-side adapter reconstructs only the audited dl_phdr_info fields; it
reports the corresponding prefix size and never fabricates TLS metadata. This
keeps platform structures outside the public Rust ABI. Symbol-info strings are
borrowed while the owning module remains loaded; the caller must prevent unload
when using a saved result.

Source changes reroute NativeAOT PDB/build-id image iteration, dump path discovery,
and libunwind image/symbol discovery. The glibc-specific `_dl_find_object` fast
path is not retained as an untracked function-pointer bypass: this configuration
uses the existing program-header implementation instead. Other builds retain
upstream behavior. No global `dladdr`/`dl_iterate_phdr` linker wrapping is used.

`scripts/images.sh` tests real loaded metadata against the loader, actual symbol
identity, early termination, eight concurrent readers and 13 synthetic host
modes. The synthetic tests can run on non-ELF hosts without claiming native ELF
execution there. Both sanitizers cover Rust and C callers/providers. The managed
source probe reads statistics only and exercises real stack discovery; separate
startup tests verify dump path discovery without generating a crash dump.

This moves loader dependencies out of the native runtime; it does not remove the
Linux backend's dependency on the OS loader or solve every BCL native dependency.
Qualification is determined by the exact revision's workflow logs.
