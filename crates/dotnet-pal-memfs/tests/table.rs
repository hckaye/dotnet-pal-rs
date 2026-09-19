//! Drives the provider through the negotiated C table, the way the runtime's
//! adapter does: the front end validates, the provider answers, statuses come back.
use dotnet_pal_rs::files::{Stats, Status, CAP, CREATE, EXCLUSIVE, LOCK_EXCLUSIVE, LOCK_SHARED, LOCK_UNLOCK, MAX_ENTRY_NAME, NODE_DIRECTORY, NODE_FILE, NODE_SYMLINK, READ, TIME_KEEP, TRUNCATE, WRITE};
use dotnet_pal_rs::io::{ACCESS_DENIED, ALREADY_EXISTS, IS_DIRECTORY, NAME_TOO_LONG, NOT_DIRECTORY, NOT_EMPTY, WOULD_BLOCK};
use dotnet_pal_rs::kernel::BUSY;
use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
use std::{ffi::c_void, mem, ptr};

dotnet_pal_rs::declare_port! { struct P; Files = dotnet_pal_memfs::MemFs }
static SLOT: dotnet_pal_rs::Slot = dotnet_pal_rs::Slot::new();
fn files() -> &'static dotnet_pal_rs::files::Ops {
    let api = dotnet_pal_rs::negotiate::<P>(&SLOT, dotnet_pal_rs::ABI_VERSION);
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities, CAP, "the port provides files and nothing else");
    &api.files
}
fn open(path: &str, flags: u32) -> (u32, *mut c_void) {
    let mut file = ptr::null_mut();
    (unsafe { files().open.unwrap()(path.as_ptr(), path.len(), flags, 0o640, &mut file) }, file)
}
fn stat(path: &str) -> (u32, Status) {
    let mut status = Status::default();
    (unsafe { files().path_status.unwrap()(path.as_ptr(), path.len(), 1, &mut status, mem::size_of::<Status>()) }, status)
}
fn lstat(path: &str) -> (u32, Status) {
    let mut status = Status::default();
    (unsafe { files().path_status.unwrap()(path.as_ptr(), path.len(), 0, &mut status, mem::size_of::<Status>()) }, status)
}
fn mkdir(path: &str) -> u32 { unsafe { files().directory_create.unwrap()(path.as_ptr(), path.len(), 0o750) } }
fn stats() -> Stats {
    let mut stats = Stats::default();
    assert_eq!(unsafe { files().read_stats.unwrap()(&mut stats, mem::size_of::<Stats>()) }, OK);
    stats
}
type TextCall = unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> u32;
/// A text call into `capacity` bytes that held 0xee: the status, the length needed and the buffer afterwards.
fn text(call: Option<TextCall>, path: &str, capacity: usize) -> (u32, usize, Vec<u8>) {
    let (mut out, mut needed) = (vec![0xeeu8; capacity], 99usize);
    let status = unsafe { call.unwrap()(path.as_ptr(), path.len(), out.as_mut_ptr(), capacity, &mut needed) };
    (status, needed, out)
}

#[test]
fn files_are_written_read_described_renamed_and_removed_through_the_table() {
    let f = files();
    assert_eq!(mkdir("/table"), OK);
    assert_eq!(mkdir("/table"), ALREADY_EXISTS);
    assert_eq!(mkdir("/table/missing/d"), NOT_FOUND);
    assert_eq!(open("/table/f", READ).0, NOT_FOUND);
    let (status, file) = open("/table/f", READ | WRITE | CREATE | EXCLUSIVE);
    assert_eq!(status, OK);
    assert!(!file.is_null());
    let (status, none) = open("/table/f", WRITE | CREATE | EXCLUSIVE);
    assert_eq!(status, ALREADY_EXISTS);
    assert!(none.is_null());
    assert_eq!(open("/table", READ).0, IS_DIRECTORY);
    assert_eq!(open("/table/f/g", READ).0, NOT_DIRECTORY);
    assert_eq!(open(&format!("/table/{}", "n".repeat(MAX_ENTRY_NAME + 1)), WRITE | CREATE).0, NAME_TOO_LONG);
    // Refused by the front end before the provider sees them.
    assert_eq!(open("/table/f", CREATE).0, INVALID_ARGUMENT);
    assert_eq!(open("/table/f", READ | TRUNCATE).0, INVALID_ARGUMENT);
    assert_eq!(open("/table/\0f", READ).0, INVALID_ARGUMENT);

    let mut done = 99usize;
    assert_eq!(unsafe { f.write_at.unwrap()(file, 4, b"sparse".as_ptr(), 6, &mut done) }, OK);
    assert_eq!(done, 6);
    let mut data = [0xeeu8; 32];
    assert_eq!(unsafe { f.read_at.unwrap()(file, 0, data.as_mut_ptr(), data.len(), &mut done) }, OK);
    assert_eq!(&data[..done], b"\0\0\0\0sparse");
    assert_eq!(unsafe { f.read_at.unwrap()(file, 10, data.as_mut_ptr(), data.len(), &mut done) }, OK);
    assert_eq!(done, 0, "end of file is zero bytes with OK");
    assert_eq!(unsafe { f.set_size.unwrap()(file, 6) }, OK);
    assert_eq!(unsafe { f.flush.unwrap()(file) }, OK);
    let mut by_handle = Status::default();
    assert_eq!(unsafe { f.status.unwrap()(file, &mut by_handle, mem::size_of::<Status>()) }, OK);
    assert_eq!((by_handle.kind, by_handle.mode, by_handle.size, by_handle.device), (NODE_FILE, 0o640, 6, 1));
    assert_eq!(stat("/table/f"), (OK, by_handle));
    let (status, directory) = stat("/table/");
    assert_eq!((status, directory.kind, directory.mode, directory.size), (OK, NODE_DIRECTORY, 0o750, 0));
    assert_ne!(directory.identity, by_handle.identity);

    let (status, reader) = open("/table/f", READ);
    assert_eq!(status, OK);
    assert_eq!(unsafe { f.write_at.unwrap()(reader, 0, b"x".as_ptr(), 1, &mut done) }, ACCESS_DENIED);
    assert_eq!(unsafe { f.set_size.unwrap()(reader, 0) }, ACCESS_DENIED);
    let rename = |from: &str, to: &str| unsafe { f.rename.unwrap()(from.as_ptr(), from.len(), to.as_ptr(), to.len()) };
    let remove = |path: &str| unsafe { f.remove.unwrap()(path.as_ptr(), path.len()) };
    assert_eq!(rename("/table/f", "/table/g"), OK);
    assert_eq!(rename("/table/f", "/table/g"), NOT_FOUND);
    assert_eq!(rename("/", "/table/root"), BUSY);
    assert_eq!(remove("/table"), IS_DIRECTORY);
    assert_eq!(remove("/table/g"), OK);
    assert_eq!(remove("/table/g"), NOT_FOUND);
    assert_eq!(stat("/table/g").0, NOT_FOUND);
    // The removed file is still there for the handles that hold it.
    assert_eq!(unsafe { f.read_at.unwrap()(reader, 4, data.as_mut_ptr(), data.len(), &mut done) }, OK);
    assert_eq!(&data[..done], b"sp");
    assert_eq!(unsafe { f.close.unwrap()(reader) }, OK);
    assert_eq!(unsafe { f.close.unwrap()(file) }, OK);
    assert_eq!(unsafe { f.close.unwrap()(ptr::null_mut()) }, INVALID_ARGUMENT);

    let mut stats = Stats::default();
    assert_eq!(unsafe { f.read_stats.unwrap()(&mut stats, mem::size_of::<Stats>()) }, OK);
    assert!(stats.open_ok >= 2 && stats.close_ok >= 2 && stats.write_ok >= 1 && stats.read_ok >= 3 && stats.rename_ok >= 1 && stats.remove_ok >= 1);
    assert!(stats.rejected_or_failed >= 10);
}

#[test]
fn directories_are_enumerated_and_removed_through_the_table() {
    let f = files();
    assert_eq!(mkdir("/entries"), OK);
    assert_eq!(mkdir("/entries/sub"), OK);
    for name in ["/entries/b", "/entries/a"] {
        let (status, file) = open(name, WRITE | CREATE);
        assert_eq!(status, OK);
        assert_eq!(unsafe { f.close.unwrap()(file) }, OK);
    }
    let remove_directory = |path: &str| unsafe { f.directory_remove.unwrap()(path.as_ptr(), path.len()) };
    assert_eq!(remove_directory("/entries"), NOT_EMPTY);
    assert_eq!(remove_directory("/entries/a"), NOT_DIRECTORY);
    assert_eq!(remove_directory("/entries/missing"), NOT_FOUND);
    assert_eq!(remove_directory("/"), BUSY);

    let mut directory = ptr::null_mut();
    assert_eq!(unsafe { f.directory_open.unwrap()(b"/entries/a".as_ptr(), 10, &mut directory) }, NOT_DIRECTORY);
    assert!(directory.is_null());
    assert_eq!(unsafe { f.directory_open.unwrap()(b"/entries".as_ptr(), 8, &mut directory) }, OK);
    let mut name = [0xeeu8; MAX_ENTRY_NAME];
    let (mut length, mut kind) = (0usize, 0u32);
    assert_eq!(unsafe { f.directory_read.unwrap()(directory, name.as_mut_ptr(), MAX_ENTRY_NAME - 1, &mut length, &mut kind) }, INVALID_ARGUMENT);
    let mut seen = Vec::new();
    loop {
        match unsafe { f.directory_read.unwrap()(directory, name.as_mut_ptr(), name.len(), &mut length, &mut kind) } {
            OK => {
                assert!(name[length..].iter().all(|byte| *byte == 0), "the front end clears the rest of the buffer");
                seen.push((String::from_utf8(name[..length].to_vec()).unwrap(), kind));
                // Deleting what was just listed does not disturb the enumeration.
                let path = format!("/entries/{}", seen.last().unwrap().0);
                let status = if kind == NODE_DIRECTORY { remove_directory(&path) } else { unsafe { f.remove.unwrap()(path.as_ptr(), path.len()) } };
                assert_eq!(status, OK);
            }
            end => { assert_eq!(end, NOT_FOUND); break; }
        }
    }
    assert_eq!((length, kind), (0, 0));
    assert_eq!(seen, [("a".to_string(), NODE_FILE), ("b".to_string(), NODE_FILE), ("sub".to_string(), NODE_DIRECTORY)]);
    // A file handle is not a directory handle, and the refusal leaves it open.
    let (status, file) = open("/entries/late", WRITE | CREATE);
    assert_eq!(status, OK);
    assert_eq!(unsafe { f.directory_close.unwrap()(file) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { f.close.unwrap()(directory) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { f.close.unwrap()(file) }, OK);
    assert_eq!(unsafe { f.directory_close.unwrap()(directory) }, OK);
    assert_eq!(unsafe { f.remove.unwrap()(b"/entries/late".as_ptr(), 13) }, OK);
    assert_eq!(remove_directory("/entries"), OK);
    assert_eq!(stat("/entries").0, NOT_FOUND);
}

#[test]
fn the_current_directory_follows_the_text_contract() {
    let f = files();
    let mut needed = 0usize;
    let mut out = [0xeeu8; 8];
    assert_eq!(unsafe { f.current_directory.unwrap()(out.as_mut_ptr(), out.len(), &mut needed) }, OK);
    assert_eq!((needed, &out[..2]), (2, &b"/\0"[..]));
    assert_eq!(unsafe { f.current_directory.unwrap()(out.as_mut_ptr(), 1, &mut needed) }, BUFFER_TOO_SMALL);
    assert_eq!(needed, 2);
    assert_eq!(unsafe { f.current_directory.unwrap()(ptr::null_mut(), 0, &mut needed) }, BUFFER_TOO_SMALL);
    assert_eq!(needed, 2);
    // Relative paths resolve against it.
    assert_eq!(mkdir("relative"), OK);
    assert_eq!(stat("/relative").0, OK);
    assert_eq!(stat("relative/../relative/.").1, stat("/relative").1);
    // This is the one test that moves it or asks a relative question: the others run beside it.
    let enter = |path: &str| unsafe { f.set_current_directory.unwrap()(path.as_ptr(), path.len()) };
    let directory_ok = stats().directory_ok;
    assert_eq!(mkdir("relative/inner"), OK);
    assert_eq!(enter("/relative/missing"), NOT_FOUND);
    assert_eq!(enter("/relative/\0"), INVALID_ARGUMENT);
    assert_eq!(enter("relative/inner/"), OK);
    assert_eq!(unsafe { f.current_directory.unwrap()(out.as_mut_ptr(), out.len(), &mut needed) }, BUFFER_TOO_SMALL);
    assert_eq!((needed, out), (16, [0; 8]));
    let mut out = [0xeeu8; 32];
    assert_eq!(unsafe { f.current_directory.unwrap()(out.as_mut_ptr(), out.len(), &mut needed) }, OK);
    assert_eq!((needed, &out[..16]), (16, &b"/relative/inner\0"[..]));
    assert_eq!(stat("..").1, stat("/relative").1);
    assert_eq!(enter("/"), OK);
    // The directory calls of the other tests count here too.
    assert!(stats().directory_ok - directory_ok >= 4);
    // The longest path the boundary passes on resolves; one byte more never arrives.
    let longest = format!("{}relative", "./".repeat((dotnet_pal_rs::runtime::MAX_NAME - 8) / 2 + 1));
    let longest = &longest[longest.len() - dotnet_pal_rs::runtime::MAX_NAME..];
    assert_eq!(stat(longest), stat("/relative"));
    assert_eq!(stat(&format!("/{longest}")).0, INVALID_ARGUMENT);
}

#[test]
fn attributes_links_and_locks_go_through_the_table() {
    let f = files();
    let before = stats();
    assert_eq!(mkdir("/extra"), OK);
    let (status, file) = open("/extra/f", READ | WRITE | CREATE);
    assert_eq!(status, OK);

    let set_mode = |path: &str, mode: u32| unsafe { f.set_mode.unwrap()(path.as_ptr(), path.len(), mode) };
    let set_times = |path: &str, follow: u32, accessed: u64, modified: u64| unsafe { f.set_times.unwrap()(path.as_ptr(), path.len(), follow, accessed, modified) };
    assert_eq!(set_mode("/extra/f", 0o4711), OK);
    assert_eq!(set_mode("/extra/f", 0o10000), INVALID_ARGUMENT);
    assert_eq!(set_mode("/extra/missing", 0o600), NOT_FOUND);
    assert_eq!(stat("/extra/f").1.mode, 0o4711);
    assert_eq!(unsafe { f.set_file_mode.unwrap()(file, 0o600) }, OK);
    assert_eq!(unsafe { f.set_file_mode.unwrap()(ptr::null_mut(), 0o600) }, INVALID_ARGUMENT);
    assert_eq!(set_times("/extra/f", 1, 111, 222), OK);
    assert_eq!(set_times("/extra/f", 1, TIME_KEEP, 333), OK);
    assert_eq!(set_times("/extra/f", 2, 1, 1), INVALID_ARGUMENT);
    assert_eq!(set_times("/extra/f", 1, i64::MAX as u64 + 1, 1), INVALID_ARGUMENT);
    assert_eq!(unsafe { f.set_file_times.unwrap()(file, 444, TIME_KEEP) }, OK);
    let described = stat("/extra/f").1;
    assert_eq!((described.mode, described.accessed_ns, described.modified_ns), (0o600, 444, 333));

    let link = |existing: &str, created: &str| unsafe { f.link.unwrap()(existing.as_ptr(), existing.len(), created.as_ptr(), created.len()) };
    let symlink = |target: &str, created: &str| unsafe { f.symlink.unwrap()(target.as_ptr(), target.len(), created.as_ptr(), created.len()) };
    assert_eq!(link("/extra/f", "/extra/g"), OK);
    assert_eq!(link("/extra/f", "/extra/g"), ALREADY_EXISTS);
    assert_eq!(link("/extra", "/extra/d"), ACCESS_DENIED);
    assert_eq!(link("/extra/f", ""), INVALID_ARGUMENT);
    assert_eq!(stat("/extra/g"), (OK, described));
    assert_eq!(symlink("../extra/f", "/extra/link"), OK);
    assert_eq!(symlink("x", "/extra/link"), ALREADY_EXISTS);
    assert_eq!(symlink("", "/extra/empty"), INVALID_ARGUMENT);
    assert_eq!(symlink("nowhere", "/extra/dangling"), OK);
    // Both times were set just above: compare what another test's clock cannot move.
    assert_eq!((stat("/extra/link").1.identity, stat("/extra/dangling").0), (described.identity, NOT_FOUND));
    assert_eq!((lstat("/extra/link").1.kind, lstat("/extra/link").1.size, lstat("/extra/dangling").1.kind), (NODE_SYMLINK, 10, NODE_SYMLINK));
    assert_eq!(set_times("/extra/dangling", 1, 5, 5), NOT_FOUND);
    assert_eq!(set_times("/extra/dangling", 0, 5, 6), OK);
    assert_eq!((lstat("/extra/dangling").1.accessed_ns, lstat("/extra/dangling").1.modified_ns), (5, 6));

    assert_eq!(text(f.read_link, "/extra/link", 16), (OK, 11, b"../extra/f\0\0\0\0\0\0".to_vec()));
    assert_eq!(text(f.read_link, "/extra/link", 11).0, OK);
    assert_eq!(text(f.read_link, "/extra/link", 10), (BUFFER_TOO_SMALL, 11, vec![0; 10]));
    assert_eq!(text(f.read_link, "/extra/link", 0), (BUFFER_TOO_SMALL, 11, vec![]));
    assert_eq!(text(f.read_link, "/extra/f", 16), (INVALID_ARGUMENT, 0, vec![0; 16]));
    assert_eq!(text(f.read_link, "/extra/missing", 16), (NOT_FOUND, 0, vec![0; 16]));
    assert_eq!(text(f.real_path, "/extra/../extra//link", 16), (OK, 9, b"/extra/f\0\0\0\0\0\0\0\0".to_vec()));
    assert_eq!(text(f.real_path, "/extra/link", 8), (BUFFER_TOO_SMALL, 9, vec![0; 8]));
    assert_eq!(text(f.real_path, "/extra/dangling", 16), (NOT_FOUND, 0, vec![0; 16]));
    let mut directory = ptr::null_mut();
    assert_eq!(unsafe { f.directory_open.unwrap()(b"/extra".as_ptr(), 6, &mut directory) }, OK);
    let (mut name, mut length, mut kind, mut kinds) = ([0u8; MAX_ENTRY_NAME], 0usize, 0u32, Vec::new());
    while unsafe { f.directory_read.unwrap()(directory, name.as_mut_ptr(), name.len(), &mut length, &mut kind) } == OK { kinds.push(kind); }
    assert_eq!(unsafe { f.directory_close.unwrap()(directory) }, OK);
    assert_eq!(kinds, [NODE_SYMLINK, NODE_FILE, NODE_FILE, NODE_SYMLINK]);

    let (status, second) = open("/extra/g", READ | WRITE);
    assert_eq!(status, OK);
    let lock = |file: *mut c_void, mode: u32, wait: u32| unsafe { f.lock.unwrap()(file, mode, wait) };
    let lock_range = |file: *mut c_void, offset: u64, length: u64, mode: u32| unsafe { f.lock_range.unwrap()(file, offset, length, mode) };
    assert_eq!(lock(file, LOCK_SHARED, 0), OK);
    assert_eq!(lock(second, LOCK_SHARED, 1), OK);
    assert_eq!(lock(second, LOCK_EXCLUSIVE, 0), WOULD_BLOCK);
    assert_eq!(dotnet_pal_rs::io::WOULD_BLOCK, 15);
    assert_eq!(lock(file, LOCK_UNLOCK, 0), OK);
    assert_eq!(lock(second, LOCK_EXCLUSIVE, 0), OK);
    assert_eq!(lock(file, LOCK_SHARED, 0), WOULD_BLOCK);
    for (mode, wait) in [(0, 0), (LOCK_UNLOCK + 1, 0), (LOCK_SHARED, 2)] { assert_eq!(lock(file, mode, wait), INVALID_ARGUMENT); }
    assert_eq!(lock(ptr::null_mut(), LOCK_SHARED, 0), INVALID_ARGUMENT);
    assert_eq!(lock_range(file, 0, 8, LOCK_EXCLUSIVE), OK);
    assert_eq!(lock_range(second, 7, 1, LOCK_SHARED), WOULD_BLOCK);
    assert_eq!(lock_range(second, 8, 1, LOCK_SHARED), OK);
    assert_eq!(lock_range(file, 2, 4, LOCK_UNLOCK), OK);
    assert_eq!(lock_range(second, 2, 4, LOCK_EXCLUSIVE), OK);
    assert_eq!(lock_range(second, 0, i64::MAX as u64, LOCK_SHARED), WOULD_BLOCK);
    for (offset, length, mode) in [(0, 0, LOCK_SHARED), (i64::MAX as u64, 1, LOCK_SHARED), (1 << 63, 1, LOCK_SHARED), (0, 1, 0)] {
        assert_eq!(lock_range(file, offset, length, mode), INVALID_ARGUMENT);
    }
    // A range lock asks for the access its mode stands for; the refusal counts as a failed call.
    let (status, reader) = open("/extra/g", READ);
    assert_eq!(status, OK);
    assert_eq!(lock_range(reader, 100, 1, LOCK_EXCLUSIVE), dotnet_pal_rs::io::ACCESS_DENIED);
    assert_eq!(unsafe { f.close.unwrap()(reader) }, OK);
    // Closing the holder is what frees the file for the other handle.
    assert_eq!(unsafe { f.close.unwrap()(second) }, OK);
    assert_eq!(lock(file, LOCK_EXCLUSIVE, 1), OK);
    assert_eq!(lock_range(file, 0, i64::MAX as u64, LOCK_EXCLUSIVE), OK);
    assert_eq!(unsafe { f.close.unwrap()(file) }, OK);

    // Other tests run beside this one, but none of them makes a call that counts here.
    let after = stats();
    assert_eq!((after.attribute_ok - before.attribute_ok, after.link_ok - before.link_ok, after.lock_ok - before.lock_ok), (6, 6, 10));
    assert!(after.rejected_or_failed - before.rejected_or_failed >= 30);
}
