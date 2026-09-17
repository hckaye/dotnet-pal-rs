//! Linux provider. This heap is separate from GC virtual memory.
use super::*;
unsafe extern "C" fn allocate(size: usize, zero: u32, out: *mut *mut c_void) -> u32 {
    let p = unsafe { if zero != 0 { libc::calloc(1,size) } else { libc::malloc(size) } };
    if p.is_null() { OUT_OF_MEMORY } else { unsafe { out.write(p) }; OK }
}
unsafe extern "C" fn resize(p: *mut c_void, size: usize, out: *mut *mut c_void) -> u32 {
    let result = unsafe { libc::realloc(p,size) };
    if result.is_null() { OUT_OF_MEMORY } else { unsafe { out.write(result) }; OK }
}
unsafe extern "C" fn release(p: *mut c_void) -> u32 { unsafe { libc::free(p) }; OK }
fn status(rc: i32) -> u32 {
    match rc { 0 => OK, libc::ENOMEM | libc::EAGAIN => OUT_OF_MEMORY,
        libc::EINVAL => INVALID_ARGUMENT, libc::EBUSY => crate::kernel::BUSY, _ => OS_ERROR }
}
unsafe extern "C" fn rw_create(out: *mut *mut c_void) -> u32 {
    let p = unsafe { libc::malloc(mem::size_of::<libc::pthread_rwlock_t>()) }.cast::<libc::pthread_rwlock_t>();
    if p.is_null() { return OUT_OF_MEMORY; }
    let rc = unsafe { libc::pthread_rwlock_init(p,ptr::null()) };
    if rc != 0 { unsafe { libc::free(p.cast()) }; return status(rc); }
    unsafe { out.write(p.cast()); } OK
}
macro_rules! rw {
    ($name:ident,$native:ident) => {
        unsafe extern "C" fn $name(p: *mut c_void) -> u32 { status(unsafe { libc::$native(p.cast()) }) }
    };
}
rw!(rw_read,pthread_rwlock_rdlock);
rw!(rw_write,pthread_rwlock_wrlock);
rw!(rw_unlock,pthread_rwlock_unlock);
unsafe extern "C" fn rw_destroy(p: *mut c_void) -> u32 {
    let rc = unsafe { libc::pthread_rwlock_destroy(p.cast()) };
    if rc == 0 { unsafe { libc::free(p) }; } status(rc)
}
unsafe extern "C" fn write_stderr(data: *const u8, size: usize, out: *mut usize) -> u32 {
    let mut done = 0;
    while done < size {
        let n = unsafe { libc::write(2, data.add(done).cast(), size-done) };
        if n > 0 { done += n as usize; }
        else if n < 0 && unsafe { *libc::__errno_location() } == libc::EINTR { continue; }
        else { unsafe { out.write(done); } return OS_ERROR; }
    }
    unsafe { out.write(done); } OK
}
unsafe extern "C" fn thread_name(name: *const u8,len: usize) -> u32 {
    if len > 15 { return INVALID_ARGUMENT; }
    let mut buffer = [0u8;16];
    unsafe { ptr::copy_nonoverlapping(name,buffer.as_mut_ptr(),len); }
    status(unsafe { libc::pthread_setname_np(libc::pthread_self(),buffer.as_ptr().cast()) })
}
static OPS: Ops = Ops { allocate: Some(allocate), resize: Some(resize), release: Some(release),
    rw_create: Some(rw_create), rw_read: Some(rw_read), rw_write: Some(rw_write), rw_unlock: Some(rw_unlock),
    rw_destroy: Some(rw_destroy), write_stderr: Some(write_stderr), thread_name: Some(thread_name), read_stats: None };
pub fn ops() -> Option<&'static Ops> { Some(&OPS) }
