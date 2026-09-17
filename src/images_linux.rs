use super::*;
struct Context {visitor: Visitor,data: *mut c_void,failed: bool}
unsafe extern "C" fn visit(info: *mut libc::dl_phdr_info,size: usize,data: *mut c_void) -> i32 {
    let ctx=unsafe {&mut *data.cast::<Context>()};
    if size<mem::offset_of!(libc::dl_phdr_info,dlpi_subs)+mem::size_of::<u64>() {ctx.failed=true;return 1;}
    let name=unsafe {(*info).dlpi_name};
    let view=View {format:ELF64_LE,reserved:0,load_bias:unsafe {(*info).dlpi_addr as usize},
        name:name.cast(),name_length:if name.is_null(){0}else{unsafe {libc::strlen(name)}},
        headers:unsafe {(*info).dlpi_phdr.cast()},header_count:unsafe {(*info).dlpi_phnum as usize},
        added:unsafe {(*info).dlpi_adds},removed:unsafe {(*info).dlpi_subs}};
    unsafe {(ctx.visitor)(&view,ctx.data)}
}
unsafe extern "C" fn iterate(visitor: Option<Visitor>,data: *mut c_void,out: *mut i32) -> u32 {
    let Some(visitor)=visitor else {return INVALID_ARGUMENT;};
    let mut context=Context{visitor,data,failed:false};
    let result=unsafe {libc::dl_iterate_phdr(Some(visit),ptr::addr_of_mut!(context).cast())};
    if context.failed {return OS_ERROR;}
    unsafe {out.write(result);}OK
}
unsafe extern "C" fn address_info(address: *mut c_void,out: *mut SymbolInfo) -> u32 {
    let mut info=mem::MaybeUninit::<libc::Dl_info>::zeroed();
    if unsafe {libc::dladdr(address,info.as_mut_ptr())}==0 {return crate::runtime::NOT_FOUND;}
    let info=unsafe {info.assume_init()};
    let name_length=if info.dli_fname.is_null(){0}else{unsafe {libc::strlen(info.dli_fname)}};
    let symbol_name_length=if info.dli_sname.is_null(){0}else{unsafe {libc::strlen(info.dli_sname)}};
    unsafe {out.write(SymbolInfo{base:info.dli_fbase,name:info.dli_fname.cast(),name_length,
        symbol_address:info.dli_saddr,symbol_name:info.dli_sname.cast(),symbol_name_length});}OK
}
static OPS: Ops=Ops{iterate:Some(iterate),address_info:Some(address_info),read_stats:None};
pub fn ops()->Option<&'static Ops>{Some(&OPS)}
