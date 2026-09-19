# Executable static-TLS qualification profile

`source-runtime.sh` explicitly sets `DOTNET_PAL_STATIC_TLS=ON`. C/C++ compilation
and the hand-written x64 TLS macro both use initial-exec TLS; the executable's
initial TLS block is supplied by its loader (or the bare-metal boot code). This
profile is not a promise that the runtime can be loaded later as an arbitrary DSO.
The original general-dynamic assembly path remains available with the option off.
The isolation gate continues to reject `__tls_get_addr`, rather than approving its
dynamic allocation and loader behavior as a non-OS contract. `system-native.sh`
builds an executable/initial-load archive with the same TLS model.

The two vxsort ISA helpers are reviewed by exact symbol and GC object owner.
Their pinned enabled implementation reads the audited minipal CPU feature result
and local state; the disabled implementation performs no OS operation.

The facilities probe's custom-payload ICMP check requires `CAP_NET_RAW` on Linux.
CI sets `PROBE_GRANT_NET_RAW=1`, which grants only that file capability to the
published test executable and removes it afterwards. The build is not run as
root, the ICMP assertions are not skipped, and a failure to grant the capability
fails the job. Other environments may supply the privilege themselves or opt in
to the same setting with `setcap` and noninteractive sudo available.

A successful symbol inventory is a link-reference check, not a syscall trace,
proof of a complete call graph, or a claim that every runtime configuration is
OS-independent. The static-TLS profile and all probes still require execution
on the target platforms.
