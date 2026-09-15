# Servicing and release policy

A green run qualifies only its exact recorded source/toolchain/profile. This
repository does not replace the .NET or Rust security-servicing programs.

## Immutable inputs

Native experiments use the audited .NET runtime source commit recorded by the
adapter and SDK 10.0.100. LLVM/WASI experiments separately pin the compiler,
host compiler pack, target runtime pack, source revision, SDK archive and hashes.
The two runtime families are not interchangeable. The package check refuses an
extra, missing, version-divergent or digest-mismatched compiler package.

A source-build manifest identifies each GC archive independently by SHA-256.
The linker audit checks the actual archive chosen by MSBuild, not a guessed cache
layout. An old WorkstationGC digest cannot authorize a ServerGC archive. Manifest
files are build evidence, not cryptographic signatures or a trust root by themselves.

## Upgrade gate

Before adopting a runtime servicing update: review source changes at every
intercepted definition, adjust the exact pin and expected signatures, build both
GC profiles on both native architectures, and run all negative/positive, OOM,
fault, finalizer, callback, concurrency and ABI tests. Repeat dependency inventory
and compare remaining references and performance reports. For LLVM also verify
compiler/CoreLib compatibility, WASI imports, the native-runtime source rebuild,
managed roots/EH and memory-limit behavior. Never suppress a failing source guard
or substitute a new package under an old version identifier.

A runtime release with a relevant known vulnerability must not be deployed merely
because its reproducibility tests are green. Reproduction pins are not endorsed
production versions. A deployment must select a maintained upstream release and
complete the upgrade gate for that exact release; no automatic servicing support
is claimed for untested versions.

## API stability

ABI 2 extensions preserve old prefix offsets and use both size and capability
checks. A semantic incompatibility requires a new ABI version, not a capability
bit that silently weakens an existing contract. Linear storage is not VM. Native
pthread storage or target exception layouts must never leak into the portable
callback table. Unsupported required capabilities reject startup.

## Release evidence

Retain commit IDs, source diffs, compiler versions, package/archive digests,
source-runtime logs, actual backend counters, memory-cap preflight, per-profile
stress/OOM/fault results, artifact sizes, RSS/timing samples and dependency reports.
Run sanitizers and longer stress on the actual deployment workload as well.
A small benchmark does not establish a universal performance bound or production
readiness. Debug symbols and reproduction commands should accompany crash reports.
