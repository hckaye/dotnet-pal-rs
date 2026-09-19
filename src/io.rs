//! Portable I/O statuses of the files and sockets groups. A provider reports the
//! condition; the consumer owns the translation into its own error vocabulary
//! (the boundary's System.Native maps them to errno values). Groups that predate
//! these statuses sanitize them to `OS_ERROR`.
pub const ALREADY_EXISTS: u32 = 9;
pub const ACCESS_DENIED: u32 = 10;
pub const IS_DIRECTORY: u32 = 11;
pub const NOT_DIRECTORY: u32 = 12;
pub const NOT_EMPTY: u32 = 13;
pub const NO_SPACE: u32 = 14;
pub const WOULD_BLOCK: u32 = 15;
pub const BROKEN_PIPE: u32 = 16;
pub const CONNECTION_REFUSED: u32 = 17;
pub const CONNECTION_RESET: u32 = 18;
pub const CONNECTION_ABORTED: u32 = 19;
pub const NOT_CONNECTED: u32 = 20;
pub const ALREADY_CONNECTED: u32 = 21;
pub const ADDRESS_IN_USE: u32 = 22;
pub const ADDRESS_NOT_AVAILABLE: u32 = 23;
pub const NETWORK_UNREACHABLE: u32 = 24;
pub const HOST_UNREACHABLE: u32 = 25;
pub const IN_PROGRESS: u32 = 26;
pub const TOO_MANY_HANDLES: u32 = 27;
pub const NAME_TOO_LONG: u32 = 28;
pub const READ_ONLY: u32 = 29;
pub const CROSS_DEVICE: u32 = 30;
pub const MESSAGE_TOO_LARGE: u32 = 31;

/// Borrows a path argument: 1..=`MAX_NAME` bytes, readable, no NUL.
pub(crate) unsafe fn path<'a>(data: *const u8, length: usize) -> Option<&'a [u8]> {
    if data.is_null() || length == 0 || length > crate::runtime::MAX_NAME || (data as usize).checked_add(length).is_none() { return None; }
    // SAFETY: a readable borrow of `length` bytes is the caller's contract.
    let bytes = unsafe { core::slice::from_raw_parts(data, length) };
    (!bytes.contains(&0)).then_some(bytes)
}
/// The NUL-terminated text contract shared by `current_directory`, `host_name` and
/// the other text answers: the provider copies the text and its NUL when they fit
/// and returns the length needed including the NUL. A short buffer is cleared and
/// reported as too small. `limit` is the longest text, with its NUL, the call allows.
pub(crate) unsafe fn text(out: *mut u8, capacity: usize, needed: *mut usize, limit: usize, provider: impl FnOnce() -> crate::port::Result<usize>) -> u32 {
    if capacity != 0 { unsafe { core::ptr::write_bytes(out, 0, capacity) }; }
    let status = match provider() {
        Ok(length) if !(2..=limit).contains(&length) => crate::OS_ERROR,
        Ok(length) if length > capacity => { unsafe { needed.write(length) }; crate::runtime::BUFFER_TOO_SMALL }
        Ok(length) => {
            // SAFETY: the provider wrote `length` bytes into a buffer of at least that capacity.
            let bytes = unsafe { core::slice::from_raw_parts(out, length) };
            if bytes[length - 1] != 0 || bytes[..length - 1].contains(&0) { crate::OS_ERROR } else { unsafe { needed.write(length) }; crate::OK }
        }
        Err(e) => e.status(),
    };
    if status != crate::OK && capacity != 0 { unsafe { core::ptr::write_bytes(out, 0, capacity) }; }
    status
}
