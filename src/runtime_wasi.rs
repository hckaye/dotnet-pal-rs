//! WASIp1 services. Missing native identity/mapping/loader services stay absent.
//! Environment snapshots are explicitly bounded to 256 entries and 16 KiB; an
//! oversized host snapshot fails instead of silently omitting variables.
use super::*;
#[link(wasm_import_module="wasi_snapshot_preview1")]
extern "C" {
    fn environ_sizes_get(count: *mut usize, size: *mut usize) -> u16;
    fn environ_get(entries: *mut *mut u8, buffer: *mut u8) -> u16;
    fn clock_time_get(id: u32, precision: u64, out: *mut u64) -> u16;
    fn random_get(out: *mut u8, size: usize) -> u16;
}
unsafe extern "C" fn realtime_ns(out: *mut u64) -> u32 {
    if unsafe { clock_time_get(0, 1, out) } == 0 { OK } else { OS_ERROR }
}
unsafe extern "C" fn random_bytes(out: *mut u8, size: usize) -> u32 {
    if unsafe { random_get(out, size) } == 0 { OK } else { OS_ERROR }
}
unsafe extern "C" fn environment_get(name: *const u8, len: usize, out: *mut u8, capacity: usize, required: *mut usize) -> u32 {
    let (mut count, mut size) = (0, 0);
    if unsafe { environ_sizes_get(&mut count, &mut size) } != 0 { return OS_ERROR; }
    if count > 256 || size > 16384 { return OUT_OF_MEMORY; }
    let mut entries = [ptr::null_mut::<u8>(); 256];
    let mut bytes = [0u8; 16384];
    if unsafe { environ_get(entries.as_mut_ptr(), bytes.as_mut_ptr()) } != 0 { return OS_ERROR; }
    let input = unsafe { core::slice::from_raw_parts(name, len) };
    let base = bytes.as_ptr() as usize;
    for entry in entries.iter().take(count) {
        let Some(offset) = (*entry as usize).checked_sub(base) else { return OS_ERROR; };
        if offset >= size { return OS_ERROR; }
        let remaining = &bytes[offset..size];
        let Some(end) = remaining.iter().position(|&b| b == 0) else { return OS_ERROR; };
        let value = &remaining[..end];
        if value.len() > len && &value[..len] == input && value[len] == b'=' {
            let needed = value.len() - len; // includes the trailing NUL
            unsafe { required.write(needed) };
            if capacity < needed { return BUFFER_TOO_SMALL; }
            unsafe { ptr::copy_nonoverlapping(value.as_ptr().add(len+1), out, needed) }; return OK;
        }
    }
    NOT_FOUND
}
static OPS: Ops = Ops {
    environment_get: Some(environment_get), realtime_ns: Some(realtime_ns), random_bytes: Some(random_bytes), ..EMPTY
};
pub fn ops() -> Option<&'static Ops> { Some(&OPS) }
