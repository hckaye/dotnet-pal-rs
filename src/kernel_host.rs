//! Required only by host-kernel. Legacy VM and host-services linkage is unchanged.
use super::*;
extern "C" { fn dotnet_pal_host_kernel_v2() -> *const HostKernel; }
pub fn table() -> Option<&'static HostKernel> {
    let p = unsafe { dotnet_pal_host_kernel_v2() };
    if p.is_null() || (p as usize) % mem::align_of::<HostKernel>() != 0 { return None; }
    // Host guarantees at least a readable header, then struct_size readable bytes.
    let h = unsafe { &*p.cast::<Header>() };
    if h.abi_version != crate::ABI_VERSION || (h.struct_size as usize) < mem::size_of::<HostKernel>() || h.capabilities & ALL != ALL { return None; }
    let table = unsafe { &*p }; let o = &table.ops;
    if o.event_create.is_none() || o.event_destroy.is_none() || o.event_set.is_none() || o.event_reset.is_none() || o.event_wait.is_none()
        || o.mutex_create.is_none() || o.mutex_destroy.is_none() || o.mutex_lock.is_none() || o.mutex_unlock.is_none()
        || o.thread_create.is_none() || o.thread_join.is_none() || o.thread_detach.is_none()
        || o.tls_create.is_none() || o.tls_destroy.is_none() || o.tls_get.is_none() || o.tls_set.is_none()
        || o.stack_bounds.is_none() || o.process_barrier.is_none() { return None; }
    Some(table)
}
macro_rules! forward {
    ($name:ident ($($arg:ident : $ty:ty),*)) => {
        pub unsafe fn $name($($arg:$ty),*) -> u32 {
            match table().and_then(|t| t.ops.$name) {
                Some(f) => unsafe { f($($arg),*) }, None => crate::UNSUPPORTED,
            }
        }
    };
}
forward!(event_create(manual:u32, initial:u32, out:*mut *mut c_void));
forward!(event_destroy(h:*mut c_void)); forward!(event_set(h:*mut c_void)); forward!(event_reset(h:*mut c_void));
forward!(event_wait(h:*mut c_void, ns:u64));
forward!(mutex_create(recursive:u32, out:*mut *mut c_void));
forward!(mutex_destroy(h:*mut c_void)); forward!(mutex_lock(h:*mut c_void)); forward!(mutex_unlock(h:*mut c_void));
forward!(thread_create(entry:Option<Entry>, arg:*mut c_void, stack:usize, out:*mut *mut c_void));
forward!(thread_join(h:*mut c_void)); forward!(thread_detach(h:*mut c_void));
forward!(tls_create(dtor:Option<Destructor>, out:*mut *mut c_void));
forward!(tls_destroy(h:*mut c_void)); forward!(tls_get(h:*mut c_void, out:*mut *mut c_void));
forward!(tls_set(h:*mut c_void, value:*mut c_void));
forward!(stack_bounds(low:*mut *mut c_void, high:*mut *mut c_void));
forward!(process_barrier());
