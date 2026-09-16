use super::*;
extern "C" {fn dotnet_pal_host_context_v2()->*const Host;}
use core::sync::atomic::{AtomicPtr,Ordering};
static CACHED:AtomicPtr<Ops>=AtomicPtr::new(ptr::null_mut());
pub fn prepare()->bool {
    if !CACHED.load(Ordering::Acquire).is_null(){return true}
    let Some(ops)=validate() else{return false};
    let _=CACHED.compare_exchange(ptr::null_mut(),ops as *const Ops as *mut Ops,Ordering::Release,Ordering::Acquire);
    true
}
pub fn ops()->Option<&'static Ops>{
    // Signal callbacks read the already-validated table; never call a host getter.
    unsafe{CACHED.load(Ordering::Acquire).as_ref()}
}
fn validate()->Option<&'static Ops>{
    let p=unsafe{dotnet_pal_host_context_v2()};
    if p.is_null() || p as usize%mem::align_of::<Host>()!=0{return None}
    let header=unsafe{ptr::addr_of!((*p).header).read()};
    if header.abi_version!=crate::ABI_VERSION || (header.struct_size as usize)<mem::size_of::<Host>() || header.capabilities&CAP==0{return None}
    let ops=unsafe{&(*p).ops};
    if ops.abi_tag.is_none() || ops.action_size.is_none() || ops.action_alignment.is_none() || ops.install.is_none() || ops.restore.is_none()
        || ops.unblock_activation.is_none() || ops.request_activation.is_none() || ops.current_thread.is_none()
        || ops.process_id_async.is_none() || ops.ignore_broken_pipe.is_none() || ops.signal_number.is_none(){return None}
    Some(ops)
}
