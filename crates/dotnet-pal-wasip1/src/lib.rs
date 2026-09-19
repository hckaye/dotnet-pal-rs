#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(target_arch = "wasm32", target_os = "wasi", target_env = "p1"))]
//! WASI Preview 1 providers for `wasm32-wasip1`: real raw imports, or every
//! import routed through the single `dotnet_pal_host.dispatch_v1` transport.
//! Missing native identity/mapping/loader services stay absent.
use dotnet_pal_rs::port::{self, Error, Lookup, Result};
use core::ptr;

pub struct Wasi;

#[cfg(not(feature = "wasi-dispatch"))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn environ_sizes_get(count: *mut usize, size: *mut usize) -> u16;
    fn environ_get(entries: *mut *mut u8, buffer: *mut u8) -> u16;
    fn clock_time_get(id: u32, precision: u64, out: *mut u64) -> u16;
    fn random_get(out: *mut u8, size: usize) -> u16;
}
#[cfg(feature = "wasi-dispatch")]
mod routed {
    //! Same runtime-facing table, not a second implicit WASI route.
    use dotnet_pal_rs::wasi::schema;
    unsafe fn call(op: u32, args: &[u64]) -> u16 {
        unsafe { dotnet_pal_rs::wasi::invoke::<super::Dispatch>(op, args.as_ptr(), args.len() as u32) as u16 }
    }
    pub unsafe fn environ_sizes_get(count: *mut usize, size: *mut usize) -> u16 { unsafe { call(schema::ENVIRON_SIZES_GET, &[count as u64, size as u64]) } }
    pub unsafe fn environ_get(entries: *mut *mut u8, bytes: *mut u8) -> u16 { unsafe { call(schema::ENVIRON_GET, &[entries as u64, bytes as u64]) } }
    pub unsafe fn clock_time_get(id: u32, precision: u64, out: *mut u64) -> u16 { unsafe { call(schema::CLOCK_TIME_GET, &[id as u64, precision, out as u64]) } }
    pub unsafe fn random_get(out: *mut u8, size: usize) -> u16 { unsafe { call(schema::RANDOM_GET, &[out as u64, size as u64]) } }
}
#[cfg(feature = "wasi-dispatch")]
use routed::*;

fn errno(code: u16) -> Result<()> { if code == 0 { Ok(()) } else { Err(Error::Os) } }

impl port::Clock for Wasi {
    fn monotonic_ns() -> Result<u64> {
        // WASI Preview 1: clockid monotonic=1, timestamps/precision are u64 nanoseconds.
        let mut value = 0;
        errno(unsafe { clock_time_get(1, 1, &mut value) })?;
        Ok(value)
    }
}
impl port::Realtime for Wasi {
    fn realtime_ns() -> Result<u64> {
        let mut value = 0;
        errno(unsafe { clock_time_get(0, 1, &mut value) })?;
        Ok(value)
    }
}
impl port::Entropy for Wasi {
    unsafe fn fill(out: *mut u8, size: usize) -> Result<()> { errno(unsafe { random_get(out, size) }) }
}
/// Environment snapshots are explicitly bounded to 256 entries and 16 KiB; an
/// oversized host snapshot fails instead of silently omitting variables.
impl port::Environment for Wasi {
    unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup> {
        let (mut count, mut size) = (0, 0);
        errno(unsafe { environ_sizes_get(&mut count, &mut size) })?;
        if count > 256 || size > 16384 { return Err(Error::OutOfMemory); }
        let mut entries = [ptr::null_mut::<u8>(); 256];
        let mut bytes = [0u8; 16384];
        errno(unsafe { environ_get(entries.as_mut_ptr(), bytes.as_mut_ptr()) })?;
        let base = bytes.as_ptr() as usize;
        for entry in entries.iter().take(count) {
            let Some(offset) = (*entry as usize).checked_sub(base) else { return Err(Error::Os); };
            if offset >= size { return Err(Error::Os); }
            let remaining = &bytes[offset..size];
            let Some(end) = remaining.iter().position(|&b| b == 0) else { return Err(Error::Os); };
            let value = &remaining[..end];
            if value.len() > name.len() && &value[..name.len()] == name && value[name.len()] == b'=' {
                let needed = value.len() - name.len(); // includes the trailing NUL
                if capacity < needed { return Ok(Lookup::TooSmall(needed)); }
                unsafe { ptr::copy_nonoverlapping(value.as_ptr().add(name.len() + 1), out, needed) };
                return Ok(Lookup::Copied(needed));
            }
        }
        Err(Error::NotFound)
    }
}

/// The single physical host import of the isolated profile.
#[cfg(feature = "wasi-dispatch")]
pub struct Dispatch;
#[cfg(feature = "wasi-dispatch")]
impl port::WasiTransport for Dispatch {
    unsafe fn dispatch(request: *const dotnet_pal_rs::wasi::Request) -> u32 {
        #[link(wasm_import_module = "dotnet_pal_host")]
        extern "C" { fn dispatch_v1(request: *const dotnet_pal_rs::wasi::Request) -> u32; }
        unsafe { dispatch_v1(request) }
    }
}
