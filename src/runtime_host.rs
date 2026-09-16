use super::*;
extern "C" { fn dotnet_pal_host_runtime_v2() -> *const Host; }
pub fn ops() -> Option<&'static Ops> {
    let value = unsafe { dotnet_pal_host_runtime_v2() };
    if value.is_null() || value as usize % mem::align_of::<Host>() != 0 { return None; }
    // A rejected host table still promises a readable header, not a readable body.
    let header = unsafe { ptr::addr_of!((*value).header).read() };
    if header.abi_version != crate::ABI_VERSION || (header.struct_size as usize) < mem::size_of::<Host>()
        || header.capabilities & ALL != ALL { return None; }
    let ops = unsafe { &(*value).ops };
    if ops.environment_get.is_none() || ops.process_id.is_none() || ops.thread_id.is_none()
        || ops.realtime_ns.is_none() || ops.random_bytes.is_none() || ops.mapping_allocate.is_none()
        || ops.mapping_release.is_none() || ops.mapping_protect.is_none() || ops.module_open.is_none()
        || ops.module_symbol.is_none() || ops.module_close.is_none() || ops.module_info.is_none() { return None; }
    Some(ops)
}
