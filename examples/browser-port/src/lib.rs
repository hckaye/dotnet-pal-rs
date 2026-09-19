//! Browser port: every OS-like service the boundary needs is a typed import
//! from the `dotnet_pal_browser_v1` JavaScript module (`host/host.mjs`).
//!
//! Imported functions return dotnet_pal status codes, not WASI errno values;
//! statuses outside the published set are normalized by the boundary front
//! ends before they reach a caller. There is no scheduler: a browser main
//! thread cannot block, so sleep/yield are absent rather than emulated by a
//! successful no-op. Process identity, native mappings, module loading, a
//! native helper heap, reader/writer locks and thread names are absent too.
//!
//! Pointer arguments are wasm32 linear-memory offsets. The host bounds-checks
//! them against the current instance memory and rejects shared memory. Calls
//! are serialized by the single-threaded Wasm instance; nothing here is
//! reentrant-safe or usable from another Wasm thread.
#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(not(target_arch = "wasm32"))]
compile_error!("browser-port targets wasm32-unknown-unknown, wasm32v1-none or wasm32-wasip1 (NativeAOT link)");
#[cfg(target_feature = "atomics")]
compile_error!("browser-port is single-threaded; shared-memory/atomics builds are rejected");

use dotnet_pal_rs::port::{self, Error, Lookup, Result};

#[link(wasm_import_module = "dotnet_pal_browser_v1")]
extern "C" {
    fn monotonic_ns(out: *mut u64) -> u32;
    fn realtime_ns(out: *mut u64) -> u32;
    fn random_bytes(out: *mut u8, size: usize) -> u32;
    fn environment_get(name: *const u8, len: usize, out: *mut u8, capacity: usize, required: *mut usize) -> u32;
    fn write_stderr(data: *const u8, size: usize, written: *mut usize) -> u32;
}

/// The page-provided services.
pub struct Browser;

impl port::Clock for Browser {
    fn monotonic_ns() -> Result<u64> {
        let mut value = 0;
        port::from_status(unsafe { monotonic_ns(&mut value) })?;
        Ok(value)
    }
}
impl port::Realtime for Browser {
    fn realtime_ns() -> Result<u64> {
        let mut value = 0;
        port::from_status(unsafe { realtime_ns(&mut value) })?;
        Ok(value)
    }
}
impl port::Entropy for Browser {
    unsafe fn fill(out: *mut u8, size: usize) -> Result<()> { port::from_status(unsafe { random_bytes(out, size) }) }
}
impl port::Environment for Browser {
    unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup> {
        let mut needed = 0;
        match port::from_status(unsafe { environment_get(name.as_ptr(), name.len(), out, capacity, &mut needed) }) {
            Ok(()) => Ok(Lookup::Copied(needed)),
            Err(Error::BufferTooSmall) => Ok(Lookup::TooSmall(needed)),
            Err(e) => Err(e),
        }
    }
}
impl port::Diagnostics for Browser {
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
        let mut written = 0;
        match port::from_status(unsafe { write_stderr(data, size, &mut written) }) {
            Ok(()) if written == size => Ok(()),
            Ok(()) => Err((0, Error::Os)), // a short write reported as success is a broken host: progress is unknowable
            Err(e) => Err((written.min(size), e)),
        }
    }
}

#[cfg(all(feature = "arena", not(feature = "grow"), not(feature = "heap")))]
type Storage = dotnet_pal_storage::Arena;
#[cfg(all(feature = "grow", not(feature = "heap")))]
type Storage = dotnet_pal_storage::Ledger<dotnet_pal_wasm::Grow>;
#[cfg(feature = "heap")]
type Storage = dotnet_pal_storage::Ledger<dotnet_pal_storage::Hooks>;

dotnet_pal_rs::define_pal! {
    Linear = Storage,
    Clock = Browser,
    Environment = Browser,
    Realtime = Browser,
    Entropy = Browser,
    Diagnostics = Browser,
}
// A trap ends the instance; the page observes the RuntimeError.
dotnet_pal_rs::define_panic_handler!(dotnet_pal_rs::port::Trap);
