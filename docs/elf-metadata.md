# ELF metadata capability

CAP_ELF64_METADATA appends an optional group without moving older ABI fields. It
provides loaded-image enumeration and address-to-image/symbol lookup. The boundary
uses its own descriptor and explicitly identified ELF64 program-header bytes, not
an operating-system dl_phdr_info or Dl_info object. Borrowed descriptors and header
arrays are valid during the callback only. The callback may stop enumeration with
any nonzero int32 value, which must be preserved exactly. Loader counter metadata
is optional and flagged; TLS loader metadata is not falsely advertised.

Linux x64/ARM64 implement the group in Rust through dl_iterate_phdr and dladdr.
The optional host-elf profile instead validates a separate C provider table. Invalid
tables reject negotiation, and callback protocol violations/unknown statuses/failed
output writes do not leak to the caller. This is not a sandbox for invalid C pointers
and does not make module loading/unloading or ELF payloads portable to other formats.
Callbacks must not unload modules, unwind or call managed code. No async-signal
safety is promised.

The source adapter routes the native PAL build-ID lookup and libunwind's ELF
lookup/enumeration through the group. It deliberately selects the existing
enumeration fallback instead of obtaining _dl_find_object from dlsym and then
calling that OS function outside the boundary. Original ELF parsing, unwind
metadata, stack maps and exception algorithms remain upstream. This can be slower
than glibc's direct object lookup; the existing benchmarks continue to record the
actual end-to-end workload costs rather than claiming a universal bound.

scripts/elf.sh checks real containing-image and exported-symbol discovery, early
stop, concurrent callbacks and eleven malformed/failing host modes. Both sanitizers
instrument these tests. Source-built native GC probes also generate a managed
exception stack trace and assert that metadata enumeration counts are nonzero only
in routed configurations. The source archive audit rejects residual direct
pthread_rwlock/dl_iterate_phdr/dlsym references for both collectors. Other OS services
remain separately inventoried; eliminating these dependencies is not proof that
all native runtime or BCL OS dependencies are isolated.
