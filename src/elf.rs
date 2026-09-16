//! ELF metadata services; ELF format is explicit, no SDK-native structure in ABI.
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::{ffi::{c_char, c_void}, mem, ptr};
pub const CAP: u64 = 524288;
pub const CAPABILITIES: u64 = if cfg!(any(all(feature="linux",target_pointer_width="64"),feature="host-elf")) { CAP } else { 0 };
#[repr(C)]
pub struct ProgramHeader {
    pub kind: u32, pub flags: u32, pub offset: u64, pub virtual_address: u64,
    pub physical_address: u64, pub file_size: u64, pub memory_size: u64, pub alignment: u64,
}
#[repr(C)]
pub struct Image {
    pub load_bias: usize, pub name: *const c_char, pub headers: *const ProgramHeader,
    pub header_count: u32, pub flags: u32, pub loads: u64, pub unloads: u64,
}
#[repr(C)]
#[derive(Clone,Copy)]
pub struct Symbol {
    pub module_base: *mut c_void, pub module_name: *const c_char,
    pub symbol_address: *mut c_void, pub symbol_name: *const c_char,
}
const EMPTY_SYMBOL: Symbol = Symbol { module_base:ptr::null_mut(), module_name:ptr::null(),
    symbol_address:ptr::null_mut(), symbol_name:ptr::null() };
pub type Visitor = unsafe extern "C" fn(*const Image,*mut c_void)->i32;
#[repr(C)]
#[derive(Clone,Copy)]
pub struct Ops {
    pub enumerate: Option<unsafe extern "C" fn(Option<Visitor>,*mut c_void,*mut i32)->u32>,
    pub lookup: Option<unsafe extern "C" fn(*const c_void,*mut Symbol)->u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats,usize)->u32>,
}
#[repr(C)]
pub struct Host { pub header:Header, pub ops:Ops }
#[repr(C)]
pub struct Stats { pub enumerate_ok:u64, pub lookup_ok:u64, pub rejected_or_failed:u64 }
static COUNTERS:[Counter;3] = [const {Counter::new()};3];
#[cfg(all(feature="linux",target_pointer_width="64"))]
#[path="elf_linux.rs"]
mod platform;
#[cfg(feature="host-elf")]
mod platform {
    use super::*;
    extern "C" { fn dotnet_pal_host_elf_v2()->*const Host; }
    pub fn ops()->Option<&'static Ops> {
        let p=unsafe{dotnet_pal_host_elf_v2()};
        if p.is_null() || p as usize % mem::align_of::<Host>() != 0 { return None; }
        let header=unsafe{ptr::addr_of!((*p).header).read()};
        if header.abi_version!=crate::ABI_VERSION || (header.struct_size as usize)<mem::size_of::<Host>() || header.capabilities & CAP == 0 { return None; }
        let ops=unsafe{&(*p).ops};
        if ops.enumerate.is_none() || ops.lookup.is_none() { None } else { Some(ops) }
    }
}
#[cfg(not(any(all(feature="linux",target_pointer_width="64"),feature="host-elf")))]
mod platform { use super::*; pub fn ops()->Option<&'static Ops>{None} }
pub fn available()->bool { CAPABILITIES==0 || platform::ops().is_some() }
fn record(status:u32,index:usize)->u32 {
    let s=match status {OK|INVALID_ARGUMENT|OS_ERROR|UNSUPPORTED|8=>status,_=>OS_ERROR};
    COUNTERS[if s==OK{index}else{2}].increment();s
}
struct Visit { visitor:Visitor, data:*mut c_void, invalid:bool, stopped:bool, result:i32 }
unsafe extern "C" fn visit(image:*const Image,data:*mut c_void)->i32 {
    let state=unsafe{&mut *data.cast::<Visit>()};
    if state.stopped { state.invalid=true; return 1; }
    if image.is_null() || image as usize % mem::align_of::<Image>() != 0 { state.invalid=true; return 1; }
    let i=unsafe{&*image};
    if i.name.is_null() || i.flags & !1 != 0 || i.header_count>u16::MAX as u32 ||
        (i.header_count!=0 && (i.headers.is_null() || i.headers as usize % mem::align_of::<ProgramHeader>() != 0 ||
        (i.headers as usize).checked_add(i.header_count as usize * mem::size_of::<ProgramHeader>()).is_none())) {
        state.invalid=true; return 1;
    }
    if state.invalid { return 1; }
    let result=unsafe{(state.visitor)(image,state.data)};
    if result!=0 { state.stopped=true; state.result=result; }
    result
}
unsafe extern "C" fn enumerate(visitor:Option<Visitor>,data:*mut c_void,out:*mut i32)->u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT,0); }
    unsafe{out.write(0)};
    let Some(visitor)=visitor else{return record(INVALID_ARGUMENT,0)};
    let Some(call)=platform::ops().and_then(|o|o.enumerate) else{return record(UNSUPPORTED,0)};
    let mut state=Visit {visitor,data,invalid:false,stopped:false,result:0}; let mut result=0;
    let mut status=unsafe{call(Some(visit),ptr::addr_of_mut!(state).cast(),&mut result)};
    if status==OK && (state.invalid || result!=state.result) { status=OS_ERROR; }
    if status==OK { unsafe{out.write(result)}; }
    record(status,0)
}
unsafe extern "C" fn lookup(address:*const c_void,out:*mut Symbol)->u32 {
    if !aligned_output(out) { return record(INVALID_ARGUMENT,1); }
    unsafe{out.write(EMPTY_SYMBOL)};
    if address.is_null() { return record(INVALID_ARGUMENT,1); }
    let Some(call)=platform::ops().and_then(|o|o.lookup) else{return record(UNSUPPORTED,1)};
    let mut value=EMPTY_SYMBOL; let mut status=unsafe{call(address,&mut value)};
    if status==OK && (value.module_base.is_null() || value.module_name.is_null() ||
        (value.symbol_name.is_null() != value.symbol_address.is_null())) { status=OS_ERROR; }
    if status==OK { unsafe{out.write(value)}; }
    record(status,1)
}
unsafe extern "C" fn read_stats(out:*mut Stats,size:usize)->u32 {
    if !aligned_output(out) || size<mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe{out.write(Stats{enumerate_ok:COUNTERS[0].load(),lookup_ok:COUNTERS[1].load(),rejected_or_failed:COUNTERS[2].load()})};OK
}
pub const OPS:Ops=Ops {
    enumerate:if CAPABILITIES!=0{Some(enumerate)}else{None},
    lookup:if CAPABILITIES!=0{Some(lookup)}else{None},read_stats:Some(read_stats),
};
