use std::ffi::c_void;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PalProtection {
    None = 0,
    Read = 1,
    ReadWrite = 2,
    ReadExecute = 3,
    ReadWriteExecute = 4,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PalResult {
    Ok = 0,
    Unsupported = 1,
    InvalidArgument = 2,
    OsError = 3,
}

#[no_mangle]
pub extern "C" fn dotnet_pal_abi_version() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn dotnet_pal_page_size() -> usize {
    platform::page_size()
}

#[no_mangle]
pub extern "C" fn dotnet_pal_sleep_ns(nanoseconds: u64) {
    thread::sleep(Duration::from_nanos(nanoseconds));
}

#[no_mangle]
pub extern "C" fn dotnet_pal_monotonic_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

#[no_mangle]
pub unsafe extern "C" fn dotnet_pal_vm_reserve(size: usize) -> *mut c_void {
    if size == 0 {
        return ptr::null_mut();
    }
    platform::vm_reserve(size)
}

#[no_mangle]
pub unsafe extern "C" fn dotnet_pal_vm_commit(
    address: *mut c_void,
    size: usize,
    protection: PalProtection,
) -> PalResult {
    if address.is_null() || size == 0 {
        return PalResult::InvalidArgument;
    }
    platform::vm_commit(address, size, protection)
}

#[no_mangle]
pub unsafe extern "C" fn dotnet_pal_vm_protect(
    address: *mut c_void,
    size: usize,
    protection: PalProtection,
) -> PalResult {
    if address.is_null() || size == 0 {
        return PalResult::InvalidArgument;
    }
    platform::vm_protect(address, size, protection)
}

#[no_mangle]
pub unsafe extern "C" fn dotnet_pal_vm_release(address: *mut c_void, size: usize) -> PalResult {
    if address.is_null() || size == 0 {
        return PalResult::InvalidArgument;
    }
    platform::vm_release(address, size)
}

#[cfg(unix)]
mod platform {
    use super::{PalProtection, PalResult};
    use std::ffi::c_void;
    use std::ptr;

    pub fn page_size() -> usize {
        unsafe {
            let value = libc::sysconf(libc::_SC_PAGESIZE);
            if value <= 0 { 4096 } else { value as usize }
        }
    }

    fn prot(value: PalProtection) -> i32 {
        match value {
            PalProtection::None => libc::PROT_NONE,
            PalProtection::Read => libc::PROT_READ,
            PalProtection::ReadWrite => libc::PROT_READ | libc::PROT_WRITE,
            PalProtection::ReadExecute => libc::PROT_READ | libc::PROT_EXEC,
            PalProtection::ReadWriteExecute => libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        }
    }

    pub unsafe fn vm_reserve(size: usize) -> *mut c_void {
        let result = libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        if result == libc::MAP_FAILED { ptr::null_mut() } else { result }
    }

    pub unsafe fn vm_commit(address: *mut c_void, size: usize, protection: PalProtection) -> PalResult {
        vm_protect(address, size, protection)
    }

    pub unsafe fn vm_protect(address: *mut c_void, size: usize, protection: PalProtection) -> PalResult {
        if libc::mprotect(address, size, prot(protection)) == 0 {
            PalResult::Ok
        } else {
            PalResult::OsError
        }
    }

    pub unsafe fn vm_release(address: *mut c_void, size: usize) -> PalResult {
        if libc::munmap(address, size) == 0 {
            PalResult::Ok
        } else {
            PalResult::OsError
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use super::{PalProtection, PalResult};
    use std::ffi::c_void;
    use std::ptr;

    pub fn page_size() -> usize { 4096 }
    pub unsafe fn vm_reserve(_size: usize) -> *mut c_void { ptr::null_mut() }
    pub unsafe fn vm_commit(_address: *mut c_void, _size: usize, _protection: PalProtection) -> PalResult { PalResult::Unsupported }
    pub unsafe fn vm_protect(_address: *mut c_void, _size: usize, _protection: PalProtection) -> PalResult { PalResult::Unsupported }
    pub unsafe fn vm_release(_address: *mut c_void, _size: usize) -> PalResult { PalResult::Unsupported }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_is_stable() {
        assert_eq!(dotnet_pal_abi_version(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn reserve_commit_write_and_release() {
        let size = dotnet_pal_page_size();
        unsafe {
            let memory = dotnet_pal_vm_reserve(size);
            assert!(!memory.is_null());
            assert_eq!(dotnet_pal_vm_commit(memory, size, PalProtection::ReadWrite), PalResult::Ok);
            (memory as *mut u8).write_volatile(0x5a);
            assert_eq!((memory as *const u8).read_volatile(), 0x5a);
            assert_eq!(dotnet_pal_vm_release(memory, size), PalResult::Ok);
        }
    }
}
