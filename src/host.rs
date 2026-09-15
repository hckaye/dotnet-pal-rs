//! SDK integration seam: the host supplies immutable callbacks before runtime startup.
//! No std, alloc, libc, global allocator or dynamic registration is required here.
use crate::{HostApi, Header, ABI_VERSION, CAP_VM, UNSUPPORTED};
use core::{ffi::c_void, mem};

extern "C" {
    fn dotnet_pal_host_v2() -> *const HostApi;
    fn dotnet_pal_host_abort() -> !;
}

fn api() -> Option<&'static HostApi> {
    // SAFETY: host contract guarantees an immutable static table, at least Header
    // bytes readable, and the declared struct_size bytes readable if version matches.
    let raw = unsafe { dotnet_pal_host_v2() };
    if raw.is_null() || (raw as usize) % mem::align_of::<HostApi>() != 0 { return None; }
    let header = unsafe { &*raw.cast::<Header>() };
    if header.abi_version != ABI_VERSION || (header.struct_size as usize) < mem::size_of::<HostApi>()
        || header.capabilities & CAP_VM == 0 { return None; }
    let api = unsafe { &*raw };
    let vm = &api.vm;
    if vm.page_size.is_none() || vm.reserve.is_none() || vm.commit.is_none()
        || vm.decommit.is_none() || vm.release.is_none() || vm.reset.is_none() { return None; }
    Some(api)
}

pub fn page_size() -> usize {
    match api().and_then(|a| a.vm.page_size) { Some(f) => unsafe { f() }, None => 0 }
}

pub unsafe fn reserve(size: usize, alignment: usize, out: *mut *mut c_void) -> u32 {
    match api().and_then(|a| a.vm.reserve) {
        Some(f) => unsafe { f(size, alignment, 0, out) }, None => UNSUPPORTED,
    }
}

macro_rules! forward {
    ($name:ident) => {
        pub unsafe fn $name(address: *mut c_void, size: usize) -> u32 {
            match api().and_then(|a| a.vm.$name) {
                Some(f) => unsafe { f(address, size) }, None => UNSUPPORTED,
            }
        }
    };
}
forward!(commit);
forward!(decommit);
forward!(release);
forward!(reset);
#[cfg(not(test))]
pub unsafe fn abort() -> ! { unsafe { dotnet_pal_host_abort() } }
