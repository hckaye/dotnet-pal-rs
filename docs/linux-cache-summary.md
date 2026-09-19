# Linux cache summary consistency

`Topology::cache_size` reports the largest per-CPU data cache, not simply L2,
an instruction-only cache, or the first source of information that answers.
Both Linux providers now compute the maximum of their four per-level queries.
Each query can use sysfs when sysconf does not know that particular level.

The no_std provider previously changed byte 41 (the slash after `index0`) when
constructing its fallback path. That produced `index00size` rather than
`index0/size` and lost all sysfs-only cache sizes. Its summary also stopped
consulting sysfs once any sysconf level answered. The desktop provider had the
same partial-sysconf problem, examined only indices 0..3, and did not exclude
instruction-only caches. Reusing the existing per-level lookup avoids divergent
path construction and gives both providers the same data/unified selection.

`tests/test_cache_summary.py` compiles the actual methods, size parsers and path
builder with controlled sysconf/file observations. It covers seven scenarios
per provider: sysfs-only data, partial sysconf data, a sysfs fourth level,
non-monotonic level sizes, instruction-only caches, absent data, and a malformed
size alongside a valid cache. High sysfs indices ensure index and level are not
confused. Both previous implementations fail this test; both revised ones pass.
These focused fixtures supplement, rather than replace, full provider and C ABI
tests on Linux x64 and ARM64.

No capability bits, ABI layouts, version policy or conformance assertions are
changed by this correction. Temporary patch-staging workflows are not retained
in the final tree; the persistent file-lock regression workflow is read-only.
