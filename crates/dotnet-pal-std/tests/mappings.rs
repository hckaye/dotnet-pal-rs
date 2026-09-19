//! Exercises the file mapping group of the std port through the negotiated C
//! table, with the handles of the files group of the same table, on whatever
//! desktop OS runs the test.
use dotnet_pal_rs::files::{self, CREATE, READ, WRITE};
use dotnet_pal_rs::mappings::{self, PRIVATE, SHARED};
#[cfg(unix)]
use dotnet_pal_rs::runtime::EXECUTE;
use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
use std::{ffi::c_void, mem::size_of, path::{Path, PathBuf}, ptr, sync::{Mutex, MutexGuard}};

/// One test at a time: the counters are the process's.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
fn api() -> &'static dotnet_pal_rs::Api {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & (mappings::CAP | files::CAP), mappings::CAP | files::CAP);
    api
}
fn ops() -> &'static mappings::Ops { &api().mappings }
/// Removes the scratch tree even when an assertion fails first.
struct Scratch(PathBuf);
impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn scratch(name: &str) -> Scratch {
    let root = std::env::temp_dir().join(format!("pal-std-mappings-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    Scratch(root)
}
fn open(path: &Path, flags: u32) -> *mut c_void {
    let (mut handle, path) = (ptr::null_mut(), path.to_str().unwrap().as_bytes());
    assert_eq!(unsafe { api().files.open.unwrap()(path.as_ptr(), path.len(), flags, 0o600, &mut handle) }, OK);
    assert!(!handle.is_null());
    handle
}
fn close(file: *mut c_void) { assert_eq!(unsafe { api().files.close.unwrap()(file) }, OK); }
#[cfg(unix)]
fn read_at(file: *mut c_void, offset: u64, size: usize) -> Vec<u8> {
    let (mut buffer, mut got) = (vec![0xAAu8; size], usize::MAX);
    assert_eq!(unsafe { api().files.read_at.unwrap()(file, offset, buffer.as_mut_ptr(), size, &mut got) }, OK);
    buffer.truncate(got);
    buffer
}
#[cfg(unix)]
fn write_at(file: *mut c_void, offset: u64, data: &[u8]) {
    let mut done = usize::MAX;
    assert_eq!(unsafe { api().files.write_at.unwrap()(file, offset, data.as_ptr(), data.len(), &mut done) }, OK);
    assert_eq!(done, data.len());
}
/// Status and address of one map call; the address starts as garbage the call has to replace.
fn map(file: *mut c_void, offset: u64, length: usize, access: u32, mode: u32) -> (u32, *mut u8) {
    let mut address = 7usize as *mut c_void;
    (unsafe { ops().map.unwrap()(file, offset, length, access, mode, &mut address) }, address.cast())
}
#[cfg(unix)]
fn mapped(file: *mut c_void, offset: u64, length: usize, access: u32, mode: u32) -> *mut u8 {
    let (status, address) = map(file, offset, length, access, mode);
    assert_eq!(status, OK);
    assert!(!address.is_null() && (address as usize).is_multiple_of(page()));
    address
}
fn unmap(address: *mut u8, length: usize) -> u32 { unsafe { ops().unmap.unwrap()(address.cast(), length) } }
fn sync(address: *mut u8, length: usize) -> u32 { unsafe { ops().sync.unwrap()(address.cast(), length) } }
fn page() -> usize { unsafe { api().vm.page_size.unwrap()() } }
#[cfg(unix)]
unsafe fn view<'a>(address: *mut u8, length: usize) -> &'a mut [u8] { unsafe { std::slice::from_raw_parts_mut(address, length) } }
fn stats() -> mappings::Stats {
    let mut out = mappings::Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<mappings::Stats>()) }, OK);
    out
}
fn counted(s: &mappings::Stats) -> (u64, u64, u64, u64) { (s.map_ok, s.unmap_ok, s.sync_ok, s.rejected_or_failed) }
/// Three pages and five bytes, every byte telling its position.
fn content() -> Vec<u8> { (0..3 * page() + 5).map(|i| (i % 251) as u8).collect() }

#[cfg(unix)]
#[test]
fn shared_mappings_are_the_file_and_private_ones_a_copy() {
    use dotnet_pal_rs::io::ACCESS_DENIED;
    let _serial = serial();
    let (root, page, before) = (scratch("shared"), page(), stats());
    let (path, mut expected) = (root.0.join("data"), content());
    std::fs::write(&path, &expected).unwrap();
    let size = expected.len();
    let file = open(&path, READ | WRITE);
    // Two shared mappings up to the end of the page that holds the last byte: the file's bytes, then zeros.
    let (first, second) = (mapped(file, 0, 4 * page, 1 | 2, SHARED), mapped(file, 0, 4 * page, 1, SHARED));
    assert_ne!(first, second);
    let (a, b) = unsafe { (view(first, 4 * page), view(second, 4 * page)) };
    assert!(a[..size] == expected[..] && b[..size] == expected[..]);
    assert!(a[size..].iter().all(|byte| *byte == 0) && b[size..].iter().all(|byte| *byte == 0));
    // A write through a shared mapping is in the file and in the other shared mapping.
    a[10..15].copy_from_slice(b"HELLO");
    expected[10..15].copy_from_slice(b"HELLO");
    assert_eq!(read_at(file, 0, size + 9), expected);
    assert_eq!(&b[10..15], b"HELLO");
    // A write through the handle is in both.
    write_at(file, page as u64 + 3, b"through the handle");
    expected[page + 3..page + 21].copy_from_slice(b"through the handle");
    assert!(a[..size] == expected[..] && b[..size] == expected[..]);
    // A private mapping starts as the file and keeps what is written to it to itself.
    let third = mapped(file, 0, size, 1 | 2, PRIVATE);
    let c = unsafe { view(third, size) };
    assert!(c[..] == expected[..]);
    c[20..27].copy_from_slice(b"PRIVATE");
    assert_eq!(read_at(file, 0, size + 9), expected);
    assert!(a[..size] == expected[..] && b[..size] == expected[..] && &c[20..27] == b"PRIVATE");
    assert_eq!((sync(first, 4 * page), sync(third, size)), (OK, OK));
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    // What is written behind the last byte does not become part of the file.
    a[size + 1] = b'x';
    assert_eq!(sync(first, 4 * page), OK);
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    // The mappings outlive the handle.
    close(file);
    a[0] = b'#';
    expected[0] = b'#';
    assert_eq!((b[0], sync(first, 4 * page)), (b'#', OK));
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    // A part in the middle of the file, through a handle that only reads.
    let reader = open(&path, READ);
    let middle = mapped(reader, page as u64, page + 7, 1, SHARED);
    assert!(unsafe { view(middle, page + 7) }[..] == expected[page..2 * page + 7]);
    a[page + 100] = b'!';
    assert_eq!(unsafe { view(middle, page) }[100], b'!');
    // Private and writable needs no more than read access; shared and writable does.
    let copy = mapped(reader, 2 * page as u64, page, 1 | 2, PRIVATE);
    unsafe { copy.write(b'?') };
    assert_ne!(a[2 * page], b'?');
    assert_eq!(map(reader, 0, page, 1 | 2, SHARED), (ACCESS_DENIED, ptr::null_mut()));
    assert_eq!(map(reader, 0, page, 2, SHARED), (ACCESS_DENIED, ptr::null_mut()));
    close(reader);
    for (address, length) in [(first, 4 * page), (second, 4 * page), (third, size), (middle, page + 7), (copy, page)] { assert_eq!(unmap(address, length), OK); }
    assert_eq!(counted(&stats()), (before.map_ok + 5, before.unmap_ok + 5, before.sync_ok + 4, before.rejected_or_failed + 2));
}

#[cfg(unix)]
#[test]
fn a_mapping_needs_read_access_a_page_offset_and_a_file_that_has_pages() {
    use dotnet_pal_rs::io::ACCESS_DENIED;
    use dotnet_pal_rs::UNSUPPORTED;
    let _serial = serial();
    let (root, page, before) = (scratch("access"), page(), stats());
    let path = root.0.join("data");
    std::fs::write(&path, content()).unwrap();
    // A handle that only writes maps nothing, whatever the mapping would be used for.
    let writer = open(&path, WRITE);
    for mode in [SHARED, PRIVATE] { for access in [1, 2, 1 | 2] { assert_eq!(map(writer, 0, page, access, mode), (ACCESS_DENIED, ptr::null_mut())); } }
    close(writer);
    let file = open(&path, READ | WRITE);
    for offset in [1, page as u64 - 1, page as u64 + 1, 2 * page as u64 + 512] { assert_eq!(map(file, offset, page, 1, SHARED), (INVALID_ARGUMENT, ptr::null_mut())); }
    // An executable mapping is the target's to grant: Linux does unless the volume forbids it, macOS refuses an unsigned file.
    let (status, address) = map(file, 0, page, 1 | EXECUTE, PRIVATE);
    assert!(matches!(status, OK | ACCESS_DENIED) && (status == OK) == !address.is_null(), "{status}");
    if cfg!(target_os = "macos") { assert_eq!(status, ACCESS_DENIED); }
    if status == OK { assert_eq!(unmap(address, page), OK); }
    close(file);
    // Nodes without pages: the null device, and a FIFO, of which macOS says EINVAL where Linux says ENODEV.
    let null = open(Path::new("/dev/null"), READ | WRITE);
    for mode in [SHARED, PRIVATE] { assert_eq!(map(null, 0, page, 1, mode), (UNSUPPORTED, ptr::null_mut())); }
    close(null);
    let fifo = root.0.join("fifo");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let pipe = open(&fifo, READ | WRITE);
    assert_eq!(map(pipe, 0, page, 1, SHARED), (UNSUPPORTED, ptr::null_mut()));
    assert_eq!(map(pipe, 1, page, 1, SHARED), (INVALID_ARGUMENT, ptr::null_mut()));
    close(pipe);
    let executable = u64::from(status == OK);
    assert_eq!(counted(&stats()), (before.map_ok + executable, before.unmap_ok + executable, before.sync_ok, before.rejected_or_failed + 15 - executable));
}

#[test]
fn malformed_arguments_are_refused_before_the_provider() {
    let _serial = serial();
    let (root, page, before) = (scratch("arguments"), page(), stats());
    let path = root.0.join("data");
    std::fs::write(&path, content()).unwrap();
    let file = open(&path, READ | WRITE | CREATE);
    let m = ops();
    let mut address = 7usize as *mut c_void;
    assert_eq!(unsafe { m.map.unwrap()(file, 0, page, 1, SHARED, ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { m.map.unwrap()(file, 0, page, 1, SHARED, (&raw mut address).cast::<u8>().wrapping_add(1).cast()) }, INVALID_ARGUMENT);
    assert_eq!(address as usize, 7);
    assert_eq!(map(ptr::null_mut(), 0, page, 1, SHARED), (INVALID_ARGUMENT, ptr::null_mut()));
    for length in [0, isize::MAX as usize + 1, usize::MAX] { assert_eq!(map(file, 0, length, 1, SHARED), (INVALID_ARGUMENT, ptr::null_mut())); }
    // The last byte of a mapping has an offset the file group can name.
    for offset in [i64::MAX as u64 - page as u64 + 1, i64::MAX as u64 + 1, u64::MAX - page as u64 + 1, u64::MAX] { assert_eq!(map(file, offset, page, 1, SHARED), (INVALID_ARGUMENT, ptr::null_mut())); }
    for access in [0, 8, 1 | 8, u32::MAX] { assert_eq!(map(file, 0, page, access, SHARED), (INVALID_ARGUMENT, ptr::null_mut())); }
    for mode in [0, SHARED | PRIVATE, 4, u32::MAX] { assert_eq!(map(file, 0, page, 1, mode), (INVALID_ARGUMENT, ptr::null_mut())); }
    let somewhere = (&raw mut address).cast::<u8>();
    for call in [unmap, sync] {
        assert_eq!(call(ptr::null_mut(), page), INVALID_ARGUMENT);
        assert_eq!(call(somewhere, 0), INVALID_ARGUMENT);
        assert_eq!(call(somewhere, isize::MAX as usize + 1), INVALID_ARGUMENT);
        assert_eq!(call((usize::MAX - page + 2) as *mut u8, page), INVALID_ARGUMENT);
    }
    assert_eq!(unsafe { m.read_stats.unwrap()(ptr::null_mut(), size_of::<mappings::Stats>()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { m.read_stats.unwrap()(&mut mappings::Stats::default(), size_of::<mappings::Stats>() - 1) }, INVALID_ARGUMENT);
    // Windows has no provider behind the table: every well-formed call is UNSUPPORTED and leaves no address.
    #[cfg(not(unix))]
    {
        assert_eq!(map(file, 0, page, 1, SHARED), (dotnet_pal_rs::UNSUPPORTED, ptr::null_mut()));
        assert_eq!((unmap(somewhere, page), sync(somewhere, page)), (dotnet_pal_rs::UNSUPPORTED, dotnet_pal_rs::UNSUPPORTED));
    }
    close(file);
    let unsupported = if cfg!(unix) { 0 } else { 3 };
    assert_eq!(counted(&stats()), (before.map_ok, before.unmap_ok, before.sync_ok, before.rejected_or_failed + 26 + unsupported));
}
