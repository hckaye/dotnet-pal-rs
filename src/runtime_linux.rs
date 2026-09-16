use super::*;
use core::ffi::CStr;

fn name_buffer(name: *const u8, len: usize) -> [u8; MAX_NAME + 1] {
    let mut result = [0u8; MAX_NAME + 1];
    if len != 0 { unsafe { ptr::copy_nonoverlapping(name, result.as_mut_ptr(), len); } }
    result
}
unsafe extern "C" fn environment_get(name: *const u8, len: usize, out: *mut u8, capacity: usize, required: *mut usize) -> u32 {
    let name = name_buffer(name, len);
    let value = unsafe { libc::getenv(name.as_ptr().cast()) };
    if value.is_null() { return NOT_FOUND; }
    // Caller must prevent concurrent environment mutation for the full call.
    let bytes = unsafe { CStr::from_ptr(value) }.to_bytes_with_nul();
    unsafe { required.write(bytes.len()) };
    if capacity < bytes.len() { return BUFFER_TOO_SMALL; }
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) }; OK
}
unsafe extern "C" fn process_id(out: *mut u64) -> u32 {
    let value = unsafe { libc::getpid() };
    if value <= 0 { return OS_ERROR; }
    unsafe { out.write(value as u64) }; OK
}
unsafe extern "C" fn thread_id(out: *mut u64) -> u32 {
    let value = unsafe { libc::syscall(libc::SYS_gettid) };
    if value <= 0 { return OS_ERROR; }
    unsafe { out.write(value as u64) }; OK
}
unsafe extern "C" fn realtime_ns(out: *mut u64) -> u32 {
    let mut value = mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, value.as_mut_ptr()) } != 0 { return OS_ERROR; }
    let value = unsafe { value.assume_init() };
    if value.tv_sec < 0 || !(0..1_000_000_000).contains(&value.tv_nsec) { return OS_ERROR; }
    let Some(ns) = (value.tv_sec as u64).checked_mul(1_000_000_000).and_then(|x| x.checked_add(value.tv_nsec as u64)) else { return OS_ERROR; };
    unsafe { out.write(ns) }; OK
}
unsafe extern "C" fn random_bytes(out: *mut u8, size: usize) -> u32 {
    let mut done = 0;
    while done < size {
        let result = unsafe { libc::getrandom(out.add(done).cast(), (size - done).min(262144), 0) };
        if result < 0 {
            if unsafe { *libc::__errno_location() } == libc::EINTR { continue; }
            return OS_ERROR;
        }
        if result == 0 { return OS_ERROR; }
        done += result as usize;
    }
    OK
}
fn protection(value: u32) -> i32 {
    (if value & READ != 0 { libc::PROT_READ } else { 0 }) |
    (if value & WRITE != 0 { libc::PROT_WRITE } else { 0 }) |
    (if value & EXECUTE != 0 { libc::PROT_EXEC } else { 0 })
}
fn page_aligned(address: *mut c_void) -> bool {
    let page = crate::backend::page_size();
    page.is_power_of_two() && address as usize & (page - 1) == 0
}
unsafe extern "C" fn mapping_allocate(size: usize, permissions: u32, out: *mut *mut c_void) -> u32 {
    let value = unsafe { libc::mmap(ptr::null_mut(), size, protection(permissions), libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) };
    if value == libc::MAP_FAILED {
        return if unsafe { *libc::__errno_location() } == libc::ENOMEM { OUT_OF_MEMORY } else { OS_ERROR };
    }
    unsafe { out.write(value) }; OK
}
unsafe extern "C" fn mapping_release(address: *mut c_void, size: usize) -> u32 {
    if !page_aligned(address) { return INVALID_ARGUMENT; }
    if unsafe { libc::munmap(address, size) } == 0 { OK } else { OS_ERROR }
}
unsafe extern "C" fn mapping_protect(address: *mut c_void, size: usize, permissions: u32) -> u32 {
    if !page_aligned(address) { return INVALID_ARGUMENT; }
    if unsafe { libc::mprotect(address, size, protection(permissions)) } == 0 { OK } else { OS_ERROR }
}
unsafe extern "C" fn module_open(name: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    let buffer = name_buffer(name, len);
    let name = if name.is_null() { ptr::null() } else { buffer.as_ptr().cast() };
    let module = unsafe { libc::dlopen(name, libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if module.is_null() { return NOT_FOUND; }
    unsafe { out.write(module) }; OK
}
unsafe extern "C" fn module_symbol(module: *mut c_void, name: *const u8, len: usize, out: *mut *mut c_void) -> u32 {
    let name = name_buffer(name, len);
    unsafe { libc::dlerror() };
    let symbol = unsafe { libc::dlsym(module, name.as_ptr().cast()) };
    if !unsafe { libc::dlerror() }.is_null() { return NOT_FOUND; }
    unsafe { out.write(symbol) }; OK
}
unsafe extern "C" fn module_close(module: *mut c_void) -> u32 {
    if unsafe { libc::dlclose(module) } == 0 { OK } else { OS_ERROR }
}
unsafe extern "C" fn module_info(address: *mut c_void, out: *mut ModuleInfo) -> u32 {
    let mut value = mem::MaybeUninit::<libc::Dl_info>::uninit();
    if unsafe { libc::dladdr(address, value.as_mut_ptr()) } == 0 { return NOT_FOUND; }
    let value = unsafe { value.assume_init() };
    if value.dli_fname.is_null() { return OS_ERROR; }
    let name = unsafe { CStr::from_ptr(value.dli_fname) }.to_bytes();
    unsafe { out.write(ModuleInfo { base: value.dli_fbase, name: value.dli_fname.cast(), name_length: name.len() }) }; OK
}
static OPS: Ops = Ops {
    environment_get: Some(environment_get), process_id: Some(process_id), thread_id: Some(thread_id),
    realtime_ns: Some(realtime_ns), random_bytes: Some(random_bytes), mapping_allocate: Some(mapping_allocate),
    mapping_release: Some(mapping_release), mapping_protect: Some(mapping_protect), module_open: Some(module_open),
    module_symbol: Some(module_symbol), module_close: Some(module_close), module_info: Some(module_info), read_stats: None,
};
pub fn ops() -> Option<&'static Ops> { Some(&OPS) }
