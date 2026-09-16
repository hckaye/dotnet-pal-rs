use super::*;
#[cfg(not(target_pointer_width="64"))]
compile_error!("ELF metadata implementation currently requires a 64-bit Linux target");
struct Context { visitor:Visitor, data:*mut c_void, invalid:bool }
unsafe extern "C" fn callback(info:*mut libc::dl_phdr_info,size:usize,data:*mut c_void)->i32 {
    let context=unsafe{&mut *data.cast::<Context>()};
    if size<mem::offset_of!(libc::dl_phdr_info,dlpi_phnum)+mem::size_of::<u16>() { context.invalid=true; return 1; }
    let counters=size>=mem::offset_of!(libc::dl_phdr_info,dlpi_subs)+mem::size_of::<u64>();
    let flat=Image {load_bias:unsafe{ptr::addr_of!((*info).dlpi_addr).read()} as usize,name:unsafe{ptr::addr_of!((*info).dlpi_name).read()},
        headers:unsafe{ptr::addr_of!((*info).dlpi_phdr).read()}.cast(),header_count:unsafe{ptr::addr_of!((*info).dlpi_phnum).read()} as u32,flags:counters as u32,
        loads:if counters{unsafe{ptr::addr_of!((*info).dlpi_adds).read()}}else{0},unloads:if counters{unsafe{ptr::addr_of!((*info).dlpi_subs).read()}}else{0}};
    unsafe{(context.visitor)(&flat,context.data)}
}
unsafe extern "C" fn enumerate(visitor:Option<Visitor>,data:*mut c_void,out:*mut i32)->u32 {
    let Some(visitor)=visitor else{return INVALID_ARGUMENT};
    let mut context=Context{visitor,data,invalid:false};
    let result=unsafe{libc::dl_iterate_phdr(Some(callback),ptr::addr_of_mut!(context).cast())};
    if context.invalid {return OS_ERROR;}
    unsafe{out.write(result)};OK
}
unsafe extern "C" fn lookup(address:*const c_void,out:*mut Symbol)->u32 {
    let mut info:libc::Dl_info=unsafe{mem::zeroed()};
    if unsafe{libc::dladdr(address,&mut info)}==0 {return 8;}
    unsafe{out.write(Symbol{module_base:info.dli_fbase,module_name:info.dli_fname,
        symbol_address:info.dli_saddr,symbol_name:info.dli_sname})};OK
}
static OPS:Ops=Ops{enumerate:Some(enumerate),lookup:Some(lookup),read_stats:None};
pub fn ops()->Option<&'static Ops>{Some(&OPS)}
