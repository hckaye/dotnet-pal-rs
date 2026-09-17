# Clock and scheduling boundary (0.4)

These capabilities extend ABI 2 **after** the VM/statistics/linear prefix. Offsets
of existing fields, the VM host table and the original ABI entry point are kept.
Consumers must check `header.struct_size` before reading the new group and check
its capability bit before calling a function. A struct size alone is not support.

| Profile | CLOCK | SCHEDULER | Storage |
| --- | --- | --- | --- |
| `linux` | `CLOCK_MONOTONIC` | `nanosleep` with EINTR retry, `sched_yield` | Sparse VM |
| `host` | Absent | Absent | Existing host VM ABI |
| `host-services` | Required host callback | Required host callbacks | Host VM |
| `linear` | Absent | Absent | Bounded linear storage |
| `wasi-clock` on `wasm32-wasip1` | Actual Preview 1 import | Absent | Bounded linear storage |
| `examples/browser-port` on wasm32 | `performance.now()` through a JS import | Absent; a page cannot block | Arena, `memory.grow`, or wasi-libc hooks |

`dotnet_pal_get_api(2)` is still the sole runtime-facing entry point. The optional
`host-services` feature additionally requires `dotnet_pal_host_services_v2()` on
the *provider* side. An old `host` build does not require this symbol. Providers
must make the separate services table immutable and available before startup.
Malformed/missing tables cause API negotiation to fail; no Linux fallback exists.

## Contract

`monotonic_ns` returns an unsigned timestamp in nanoseconds. Its epoch is
unspecified, host-defined, and not UTC. The host must provide nondecreasing
readings for sequential calls; equal readings are permitted. This is not a
cross-machine or cross-instance timestamp format. Suspend-time accounting is
host-defined. Precision=1 in the WASI import is a request, not a 1 ns accuracy
claim. The boundary clears valid output storage on failure and does not leak an
output accidentally written by a failing callback. Unknown host statuses become
`OS_ERROR`. Null/unaligned outputs are rejected without invoking the OS callback;
other pointer validity requirements remain the caller's responsibility.

`sleep_ns` is relative, blocking and retries interruptions on Linux. No promise
of timely wakeup or fairness is made. `yield_thread` is a scheduling hint, not a
promise that another thread runs. Neither is exposed in the current WASI profile;
absence is represented by capability bits and NULL callbacks, never success from
a no-op. These APIs are not for signal handlers, interrupts or reentry from the
runtime into managed code. Callbacks must not throw or unwind across the C ABI.

Counters are saturating diagnostics, not a transactional snapshot or a benchmark.
This adds work per call. Production deployment still needs overhead measurements.

## Actual NativeAOT integration

The pinned source patch now routes these five `GCToOSInterface` definitions:
`QueryPerformanceCounter`, `QueryPerformanceFrequency`, `GetLowPrecisionTimeStamp`,
`Sleep`, and `YieldThread`, in addition to six GC VM definitions. The counter uses
nanoseconds with an explicit 1 GHz frequency; low-resolution timestamps divide
by one million. A value outside .NET's signed counter range is rejected.
Missing capabilities or errors abort because these upstream APIs cannot report
failure. Do not substitute a timestamp or silently skip a required sleep.

Source-rebuilt native runtime tests run on Linux x64 and ARM64 with both Linux and
host callbacks. The C# probe imports observers only. It asserts that clock counts
increase during the managed workload. Baseline and VM-only `--wrap` configurations
assert zero service counts, preventing an ordinary FFI call from being mistaken
for internal GC integration. Sleep/yield counts are logged, not required nonzero:
a workload may not take those runtime paths. Their behavior is separately tested
through the C ABI, including an interrupted sleep and concurrent callers.

This does not redirect `Thread.Sleep`, `Stopwatch`, all BCL clocks, thread creation,
TLS, mutexes or GC suspension. It does not establish an end-to-end NativeAOT Wasm
runtime. Those are distinct acceptance gates, not implications of passing these
tests. See [readiness](readiness.md).

## Reproduce

```sh
bash scripts/check.sh
bash scripts/services.sh
bash scripts/nativeaot.sh
bash scripts/source-runtime.sh /clean/pinned/runtime
rustup target add wasm32-wasip1
bash scripts/wasi-clock.sh
```

The WASI test runs a real `node:wasi` Preview 1 clock, checks the exact import
allowlist (only `clock_time_get`), rejects instantiation with no host import, and
uses a second isolated instance to inject a failed syscall that corrupts its
output. No filesystem preopens or environment access are given. Node is a test
host, not a security sandbox for untrusted modules. No WASIp2/Component Model
support is claimed. Scripts have process timeouts; CI artifacts include source
archive digest, symbol inventory, run counters and generated Wasm modules.

Primary contracts:
- [Rust WASIp1 target](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html)
- [WASI Preview 1](https://github.com/WebAssembly/WASI/tree/wasi-0.1/preview1)
- [Node WASI embedding](https://nodejs.org/api/wasi.html)
- [Pinned .NET GC OS implementation](https://github.com/dotnet/runtime/blob/60629d14374c56f1cb51819049ad1fa529307f8d/src/coreclr/gc/unix/gcenv.unix.cpp)
