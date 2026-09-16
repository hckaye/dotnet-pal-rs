//! Architecture-bound signal/activation substrate. This capability does NOT make
//! CPU register contexts portable: the native adapter must agree on context ABI.
//! Installation is once per signal and previous-handler storage is caller-owned.
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::{ffi::c_void, mem};
#[cfg(any(feature="linux",feature="host-context"))]
use core::ptr;
pub const CAP: u64 = 131072;
pub const CAPABILITIES: u64 = if cfg!(any(feature="linux",feature="host-context")) { CAP } else { 0 };
pub const ACTIVATION:u32=0;
pub const SEGMENTATION:u32=1;
pub const BUS:u32=2;
pub const FLOATING_POINT:u32=3;
pub const ILLEGAL_INSTRUCTION:u32=4;
pub type Callback=unsafe extern "C" fn(i32,*mut c_void,*mut c_void,*mut c_void);
#[repr(C)]
#[derive(Clone,Copy)]
pub struct Ops {
    pub abi_tag:Option<extern "C" fn()->u64>,
    pub action_size:Option<extern "C" fn()->usize>,
    pub action_alignment:Option<extern "C" fn()->usize>,
    pub install:Option<unsafe extern "C" fn(u32,Option<Callback>,*mut c_void,*mut c_void,usize)->u32>,
    pub restore:Option<unsafe extern "C" fn(u32,*const c_void,usize)->u32>,
    pub unblock_activation:Option<unsafe extern "C" fn()->u32>,
    pub request_activation:Option<unsafe extern "C" fn(usize)->u32>,
    pub current_thread:Option<unsafe extern "C" fn(*mut usize)->u32>,
    pub process_id_async:Option<unsafe extern "C" fn(*mut u64)->u32>,
    pub ignore_broken_pipe:Option<unsafe extern "C" fn()->u32>,
    pub read_stats:Option<unsafe extern "C" fn(*mut Stats,usize)->u32>,
    pub signal_number:Option<extern "C" fn(u32)->i32>,
}
#[repr(C)]
pub struct Host { pub header:Header,pub ops:Ops }
#[repr(C)]
pub struct Stats { pub installs:u64,pub restores:u64,pub requests:u64,pub unblocks:u64,pub thread_queries:u64,pub rejected:u64 }
static COUNTERS:[Counter;6]=[const {Counter::new()};6];
#[cfg(feature="linux")]
#[path="context_linux.rs"]
mod platform;
#[cfg(feature="host-context")]
#[path="context_host.rs"]
mod platform;
#[cfg(not(any(feature="linux",feature="host-context")))]
mod platform {use super::*;pub fn ops()->Option<&'static Ops>{None}}
pub fn available()->bool {
    #[cfg(feature="host-context")]
    return platform::prepare();
    #[cfg(not(feature="host-context"))]
    {CAPABILITIES==0 || platform::ops().is_some()}
}
extern "C" fn signal_number(kind:u32)->i32 {
    if kind>4{return -1}
    platform::ops().and_then(|o|o.signal_number).map(|f|f(kind)).unwrap_or(-1)
}
fn record(status:u32,index:usize)->u32 {
    let status=match status {OK|UNSUPPORTED|INVALID_ARGUMENT|OS_ERROR|6|8=>status,_=>OS_ERROR};
    COUNTERS[if status==OK {index} else {5}].increment();status
}
extern "C" fn abi_tag()->u64 {platform::ops().and_then(|o|o.abi_tag).map(|f|f()).unwrap_or(0)}
extern "C" fn action_size()->usize {platform::ops().and_then(|o|o.action_size).map(|f|f()).unwrap_or(0)}
extern "C" fn action_alignment()->usize {platform::ops().and_then(|o|o.action_alignment).map(|f|f()).unwrap_or(0)}
fn valid_action(value:*const c_void,size:usize)->bool {
    let alignment=action_alignment();
    !value.is_null() && size==action_size() && size!=0 && alignment.is_power_of_two()
        && value as usize % alignment==0 && (value as usize).checked_add(size).is_some()
}
unsafe extern "C" fn install(kind:u32,callback:Option<Callback>,data:*mut c_void,previous:*mut c_void,size:usize)->u32 {
    if kind>4 || callback.is_none() || !valid_action(previous,size) {return record(INVALID_ARGUMENT,0)}
    let Some(call)=platform::ops().and_then(|o|o.install) else{return record(UNSUPPORTED,0)};
    record(unsafe{call(kind,callback,data,previous,size)},0)
}
unsafe extern "C" fn restore(kind:u32,previous:*const c_void,size:usize)->u32 {
    if kind>4 || !valid_action(previous,size) {return record(INVALID_ARGUMENT,1)}
    let Some(call)=platform::ops().and_then(|o|o.restore) else{return record(UNSUPPORTED,1)};
    record(unsafe{call(kind,previous,size)},1)
}
unsafe extern "C" fn unblock_activation()->u32 {
    let Some(call)=platform::ops().and_then(|o|o.unblock_activation) else{return record(UNSUPPORTED,3)};
    record(unsafe{call()},3)
}
unsafe extern "C" fn request_activation(token:usize)->u32 {
    if token==0{return record(INVALID_ARGUMENT,2)}
    let Some(call)=platform::ops().and_then(|o|o.request_activation) else{return record(UNSUPPORTED,2)};
    record(unsafe{call(token)},2)
}
unsafe extern "C" fn current_thread(out:*mut usize)->u32 {
    if !aligned_output(out){return record(INVALID_ARGUMENT,4)}
    unsafe{out.write(0)};
    let Some(call)=platform::ops().and_then(|o|o.current_thread) else{return record(UNSUPPORTED,4)};
    let mut value=0;let mut status=unsafe{call(&mut value)};
    if status==OK {if value==0 {status=OS_ERROR}else{unsafe{out.write(value)}}}
    record(status,4)
}
unsafe extern "C" fn process_id_async(out:*mut u64)->u32 {
    // No counter, mutex or table negotiation in the actual signal dispatch path.
    if !aligned_output(out){return INVALID_ARGUMENT}
    unsafe{out.write(0)};
    let Some(call)=platform::ops().and_then(|o|o.process_id_async) else{return UNSUPPORTED};
    let mut value=0;let status=unsafe{call(&mut value)};
    if status!=OK || value==0 {return OS_ERROR}
    unsafe{out.write(value)};OK
}
unsafe extern "C" fn ignore_broken_pipe()->u32 {
    let Some(call)=platform::ops().and_then(|o|o.ignore_broken_pipe) else{return UNSUPPORTED};
    let status=unsafe{call()};if status==OK{OK}else{OS_ERROR}
}
unsafe extern "C" fn read_stats(out:*mut Stats,size:usize)->u32 {
    if !aligned_output(out)||size<mem::size_of::<Stats>(){return INVALID_ARGUMENT}
    unsafe{out.write(Stats{installs:COUNTERS[0].load(),restores:COUNTERS[1].load(),requests:COUNTERS[2].load(),
        unblocks:COUNTERS[3].load(),thread_queries:COUNTERS[4].load(),rejected:COUNTERS[5].load()})};OK
}
pub const EMPTY:Ops=Ops {abi_tag:None,action_size:None,action_alignment:None,install:None,restore:None,unblock_activation:None,
    request_activation:None,current_thread:None,process_id_async:None,ignore_broken_pipe:None,read_stats:Some(read_stats),signal_number:None};
pub const OPS:Ops=if CAPABILITIES!=0 {Ops {abi_tag:Some(abi_tag),action_size:Some(action_size),action_alignment:Some(action_alignment),
    install:Some(install),restore:Some(restore),unblock_activation:Some(unblock_activation),request_activation:Some(request_activation),
    current_thread:Some(current_thread),process_id_async:Some(process_id_async),ignore_broken_pipe:Some(ignore_broken_pipe),read_stats:Some(read_stats),signal_number:Some(signal_number)}} else {EMPTY};
