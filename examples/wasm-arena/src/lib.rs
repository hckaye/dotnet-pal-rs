//! Executes the actual ABI 2 table in WASM. This is NOT a NativeAOT WASM program.
#![no_std]
use core::{ffi::c_void, ptr};
use dotnet_pal_rs::*;

#[repr(align(65536))]
struct Backing([u8; 4 * 65536]);
static mut BACKING: Backing = Backing([0; 4 * 65536]);

/// Host must serialize calls and must not share the instance's memory.
#[no_mangle]
pub extern "C" fn wasm_arena_probe() -> u32 {
    unsafe {
        let base = ptr::addr_of_mut!(BACKING.0).cast::<c_void>();
        if dotnet_pal_get_api(ABI_VERSION).is_null()
            && dotnet_pal_arena_init(base, 4 * 65536, 65536) != OK { return 1; }
        if dotnet_pal_arena_init(base, 4 * 65536, 65536) != INVALID_ARGUMENT { return 2; }
        let raw = dotnet_pal_get_api(ABI_VERSION);
        if raw.is_null() { return 3; }
        let p = &*raw;
        if p.header.capabilities & CAP_VM != 0 { return 4; }
        if p.header.capabilities & (CAP_VM_LINEAR | CAP_ZERO_RECOMMIT) != (CAP_VM_LINEAR | CAP_ZERO_RECOMMIT) { return 5; }
        let mut a = ptr::null_mut();
        if p.vm.reserve.unwrap()(2 * 65536, 65536, 0, &mut a) != OK { return 6; }
        if p.vm.commit.unwrap()(a, 2 * 65536) != OK { return 7; }
        ptr::write_bytes(a, 0x5a, 2 * 65536);
        if p.vm.commit.unwrap()(a, 65536) != OK || *a.cast::<u8>() != 0x5a { return 8; }
        if p.vm.decommit.unwrap()(a, 65536) != OK || p.vm.commit.unwrap()(a, 65536) != OK { return 9; }
        if !core::slice::from_raw_parts(a.cast::<u8>(), 65536).iter().all(|v| *v == 0) { return 10; }
        if !core::slice::from_raw_parts(a.cast::<u8>().add(65536), 65536).iter().all(|v| *v == 0x5a) { return 11; }
        if p.vm.release.unwrap()(a, 65536) != INVALID_ARGUMENT { return 12; }
        if p.vm.release.unwrap()(a, 2 * 65536) != OK { return 13; }
        if p.vm.commit.unwrap()(a, 65536) != INVALID_ARGUMENT { return 14; }
        let mut again = ptr::null_mut();
        if p.vm.reserve.unwrap()(2 * 65536, 65536, 0, &mut again) != OK || again != a { return 15; }
        let mut too_large = again;
        if p.vm.reserve.unwrap()(4 * 65536, 65536, 0, &mut too_large) != OS_ERROR || !too_large.is_null() { return 16; }
        if p.vm.reset.unwrap()(again, 65536) != OK { return 17; }
        if p.vm.release.unwrap()(again, 2 * 65536) != OK { return 18; }
        let mut stats = Stats::default();
        let status = p.read_stats.unwrap()(&mut stats, core::mem::size_of::<Stats>());
        if p.header.capabilities & CAP_STATS == 0 && status != UNSUPPORTED { return 19; }
        if p.header.capabilities & CAP_STATS != 0 && (status != OK || stats.reserve_ok == 0) { return 20; }
        0
    }
}
