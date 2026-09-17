//! Borrowed loader metadata. Object format is explicit and never guessed.
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr};
pub const CAP_ELF: u64 = 8388608;
pub const CAP_SYMBOL: u64 = 16777216;
pub const ALL: u64 = CAP_ELF | CAP_SYMBOL;
pub const ELF64_LE: u32 = 1;
pub const CAPABILITIES: u64 = if cfg!(any(all(feature="linux", target_pointer_width="64", target_endian="little"), feature="host-images")) { ALL } else { 0 };
#[repr(C)]
#[derive(Clone, Copy)]
pub struct View {
    pub format: u32, pub reserved: u32, pub load_bias: usize,
    pub name: *const u8, pub name_length: usize, pub headers: *const u8,
    pub header_count: usize, pub added: u64, pub removed: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SymbolInfo {
    pub base: *mut c_void, pub name: *const u8, pub name_length: usize,
    pub symbol_address: *mut c_void, pub symbol_name: *const u8, pub symbol_name_length: usize,
}
const EMPTY_INFO: SymbolInfo = SymbolInfo {base: ptr::null_mut(), name: ptr::null(), name_length: 0,
    symbol_address: ptr::null_mut(), symbol_name: ptr::null(), symbol_name_length: 0};
pub type Visitor = unsafe extern "C" fn(*const View, *mut c_void) -> i32;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub iterate: Option<unsafe extern "C" fn(Option<Visitor>, *mut c_void, *mut i32) -> u32>,
    pub address_info: Option<unsafe extern "C" fn(*mut c_void, *mut SymbolInfo) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
pub struct Stats {pub iterate_ok: u64, pub images_seen: u64, pub address_ok: u64, pub rejected: u64}
static COUNTERS: [Counter; 4] = [const {Counter::new()};4];
#[cfg(all(feature="linux", target_pointer_width="64", target_endian="little"))]
#[path="images_linux.rs"]
mod platform;
#[cfg(feature="host-images")]
mod platform {
    use super::*;
    use core::sync::atomic::{AtomicPtr, Ordering};
    static CACHED: AtomicPtr<Ops> = AtomicPtr::new(ptr::null_mut());
    extern "C" { fn dotnet_pal_host_images_v2() -> *const Host; }
    pub fn ops() -> Option<&'static Ops> {
        let cached = CACHED.load(Ordering::Acquire);
        if !cached.is_null() {return Some(unsafe {&*cached});}
        let p = unsafe {dotnet_pal_host_images_v2()};
        if p.is_null() || p as usize % mem::align_of::<Host>() != 0 {return None;}
        let h = unsafe {ptr::read(p.cast::<Header>())};
        if h.abi_version != crate::ABI_VERSION || (h.struct_size as usize) < mem::size_of::<Host>() || h.capabilities & ALL != ALL {return None;}
        let ops = unsafe {&(*p).ops};
        if ops.iterate.is_none() || ops.address_info.is_none() {return None;}
        CACHED.store(ops as *const Ops as *mut Ops,Ordering::Release);Some(ops)
    }
}
#[cfg(not(any(all(feature="linux", target_pointer_width="64", target_endian="little"),feature="host-images")))]
mod platform {use super::*;pub fn ops() -> Option<&'static Ops> {None}}
pub fn available() -> bool {CAPABILITIES==0 || platform::ops().is_some()}
fn record(status: u32, index: usize) -> u32 {
    let status=match status {OK | INVALID_ARGUMENT | OS_ERROR | UNSUPPORTED | crate::OUT_OF_MEMORY | crate::runtime::NOT_FOUND => status, _=>OS_ERROR};
    COUNTERS[if status==OK {index} else {3}].increment();status
}
unsafe fn valid_string(p: *const u8,len: usize,optional: bool) -> bool {
    if p.is_null() {return optional && len==0;}
    if len >= isize::MAX as usize || (p as usize).checked_add(len+1).is_none() {return false;}
    // Provider owns the string: terminator and readable extent are its contract.
    unsafe {*p.add(len)==0}
}
struct VisitContext {visitor: Visitor, data: *mut c_void, invalid: bool, stopped: bool, result: i32}
unsafe extern "C" fn visit(view: *const View,data: *mut c_void) -> i32 {
    let ctx=unsafe {&mut *data.cast::<VisitContext>()};
    if ctx.stopped {ctx.invalid=true;return 1;}
    if view.is_null() || view as usize % mem::align_of::<View>() != 0 {ctx.invalid=true;ctx.stopped=true;return 1;}
    let v=unsafe {ptr::read(view)};
    if v.format!=ELF64_LE || v.reserved!=0 || v.header_count==0 || v.header_count>u16::MAX as usize
        || v.headers.is_null() || v.headers as usize % 8!=0
        || (v.headers as usize).checked_add(v.header_count*56).is_none()
        || !unsafe {valid_string(v.name,v.name_length,false)} {
        ctx.invalid=true;ctx.stopped=true;return 1;
    }
    COUNTERS[1].increment();
    let result=unsafe {(ctx.visitor)(view,ctx.data)};
    if result!=0 {ctx.stopped=true;ctx.result=result;}
    result
}
unsafe extern "C" fn iterate(visitor: Option<Visitor>,data: *mut c_void,out: *mut i32) -> u32 {
    if !aligned_output(out) {return record(INVALID_ARGUMENT,0);}
    unsafe {out.write(0);}
    let Some(visitor)=visitor else {return record(INVALID_ARGUMENT,0);};
    let Some(call)=platform::ops().and_then(|o|o.iterate) else {return record(UNSUPPORTED,0);};
    let mut ctx=VisitContext {visitor,data,invalid:false,stopped:false,result:0};
    let mut result=0;
    let mut status=unsafe {call(Some(visit),ptr::addr_of_mut!(ctx).cast(),&mut result)};
    if ctx.invalid || (status==OK && result!=ctx.result) {status=OS_ERROR;}
    if status==OK {unsafe {out.write(result);}}
    record(status,0)
}
unsafe extern "C" fn address_info(address: *mut c_void,out: *mut SymbolInfo) -> u32 {
    if !aligned_output(out) {return record(INVALID_ARGUMENT,2);}
    unsafe {out.write(EMPTY_INFO);}
    if address.is_null() {return record(INVALID_ARGUMENT,2);}
    let Some(call)=platform::ops().and_then(|o|o.address_info) else {return record(UNSUPPORTED,2);};
    let mut result=EMPTY_INFO;
    let mut status=unsafe {call(address,&mut result)};
    if status==OK {
        if result.base.is_null() || !unsafe {valid_string(result.name,result.name_length,false)}
            || !unsafe {valid_string(result.symbol_name,result.symbol_name_length,true)} {status=OS_ERROR;}
        else {unsafe {out.write(result);}}
    }
    record(status,2)
}
unsafe extern "C" fn stats(out: *mut Stats,size: usize) -> u32 {
    if !aligned_output(out) || size<mem::size_of::<Stats>() {return INVALID_ARGUMENT;}
    unsafe {out.write(Stats{iterate_ok:COUNTERS[0].load(),images_seen:COUNTERS[1].load(),address_ok:COUNTERS[2].load(),rejected:COUNTERS[3].load()});}OK
}
pub const EMPTY: Ops = Ops{iterate:None,address_info:None,read_stats:Some(stats)};
pub const OPS: Ops = if CAPABILITIES==0 {EMPTY} else {Ops{iterate:Some(iterate),address_info:Some(address_info),read_stats:Some(stats)}};
