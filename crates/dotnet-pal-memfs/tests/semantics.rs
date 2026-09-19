//! Every rule of the provider, called through the traits the way a front end calls
//! them. The file system is one per process, so each test works under its own
//! top-level directory. Tests that set or assert process-wide state (capacity, bytes
//! in use, the clock, the yield, the working directory) run `alone()` and put it back;
//! the others run `together()`. The wait function of a reading watcher cannot be taken
//! back once it is set, so every test that waits sets the same one, which keeps its
//! books per thread; what a read does without one is in `tests/unhooked.rs`.
use dotnet_pal_memfs::{set_capacity, set_clock, set_wait, set_yield, used_bytes, MemFs};
use dotnet_pal_rs::files::{Status, CREATE, EXCLUSIVE, LOCK_EXCLUSIVE, LOCK_SHARED, LOCK_UNLOCK, NODE_DIRECTORY, NODE_FILE, NODE_SYMLINK, READ, TRUNCATE, WRITE};
use dotnet_pal_rs::port::{self, Error, Files, Mappings, Result};
use dotnet_pal_rs::runtime::EXECUTE;
use dotnet_pal_rs::watches::{self, Event, ACCESS, ATTRIBUTES, DELETE, DIRECTORY, FOREVER, MODIFY, MOVED_FROM, MOVED_TO, NO_FOLLOW, ONLY_DIRECTORY, OVERFLOW, REMOVED};
use std::{cell::RefCell, collections::HashSet, ffi::c_void, ptr, sync::{atomic::{AtomicU64, Ordering}, mpsc, Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard}, thread};

static WORLD: RwLock<()> = RwLock::new(());
fn together() -> RwLockReadGuard<'static, ()> { WORLD.read().unwrap_or_else(PoisonError::into_inner) }
fn alone() -> RwLockWriteGuard<'static, ()> { WORLD.write().unwrap_or_else(PoisonError::into_inner) }

fn open_mode(path: &str, flags: u32, mode: u32) -> Result<*mut c_void> { unsafe { MemFs::open(path.as_bytes(), flags, mode) } }
fn open(path: &str, flags: u32) -> Result<*mut c_void> { open_mode(path, flags, 0o644) }
fn close(file: *mut c_void) { assert_eq!(unsafe { MemFs::close(file) }, Ok(())); }
fn write(file: *mut c_void, offset: u64, data: &[u8]) -> Result<usize> { unsafe { MemFs::write_at(file, offset, data.as_ptr(), data.len()) } }
fn read(file: *mut c_void, offset: u64, capacity: usize) -> Result<Vec<u8>> {
    let mut out = vec![0xee; capacity];
    let got = unsafe { MemFs::read_at(file, offset, out.as_mut_ptr(), capacity) }?;
    out.truncate(got);
    Ok(out)
}
fn set_size(file: *mut c_void, size: u64) -> Result<()> { unsafe { MemFs::set_size(file, size) } }
fn status(file: *mut c_void) -> Status { unsafe { MemFs::status(file) }.unwrap() }
fn stat(path: &str) -> Result<Status> { unsafe { MemFs::path_status(path.as_bytes(), true) } }
/// What the path names, a link in its final name included.
fn lstat(path: &str) -> Result<Status> { unsafe { MemFs::path_status(path.as_bytes(), false) } }
fn chmod(path: &str, mode: u32) -> Result<()> { unsafe { MemFs::set_mode(path.as_bytes(), mode) } }
fn utimes(path: &str, follow: bool, accessed: Option<u64>, modified: Option<u64>) -> Result<()> { unsafe { MemFs::set_times(path.as_bytes(), follow, accessed, modified) } }
fn link(existing: &str, created: &str) -> Result<()> { unsafe { MemFs::link(existing.as_bytes(), created.as_bytes()) } }
fn symlink(target: &str, created: &str) -> Result<()> { unsafe { MemFs::symlink(target.as_bytes(), created.as_bytes()) } }
/// Asks a text call three times, so that every answer is held to the contract: the
/// length needed whatever the capacity, nothing written into a buffer one byte short.
fn text(call: impl Fn(*mut u8, usize) -> Result<usize>) -> Result<String> {
    let needed = call(ptr::null_mut(), 0)?;
    let mut out = vec![0xee; needed];
    assert_eq!(call(out.as_mut_ptr(), needed - 1), Ok(needed));
    assert!(out.iter().all(|byte| *byte == 0xee));
    assert_eq!(call(out.as_mut_ptr(), needed), Ok(needed));
    assert_eq!(out.pop(), Some(0));
    Ok(String::from_utf8(out).unwrap())
}
fn read_link(path: &str) -> Result<String> { text(|out, capacity| unsafe { MemFs::read_link(path.as_bytes(), out, capacity) }) }
fn real_path(path: &str) -> Result<String> { text(|out, capacity| unsafe { MemFs::real_path(path.as_bytes(), out, capacity) }) }
fn cwd() -> Result<String> { text(|out, capacity| unsafe { MemFs::current_directory(out, capacity) }) }
fn chdir(path: &str) -> Result<()> { unsafe { MemFs::set_current_directory(path.as_bytes()) } }
fn lock(file: *mut c_void, mode: u32) -> Result<()> { unsafe { MemFs::lock(file, mode, false) } }
fn lock_range(file: *mut c_void, offset: u64, length: u64, mode: u32) -> Result<()> { unsafe { MemFs::lock_range(file, offset, length, mode) } }
fn remove(path: &str) -> Result<()> { unsafe { MemFs::remove(path.as_bytes()) } }
fn rename(from: &str, to: &str) -> Result<()> { unsafe { MemFs::rename(from.as_bytes(), to.as_bytes()) } }
fn mkdir(path: &str) -> Result<()> { unsafe { MemFs::create_directory(path.as_bytes(), 0o755) } }
fn rmdir(path: &str) -> Result<()> { unsafe { MemFs::remove_directory(path.as_bytes()) } }
fn opendir(path: &str) -> Result<*mut c_void> { unsafe { MemFs::open_directory(path.as_bytes()) } }
fn next(directory: *mut c_void) -> Result<(String, u32)> {
    let mut name = [0u8; 255];
    let (length, kind) = unsafe { MemFs::read_directory(directory, name.as_mut_ptr(), name.len()) }?;
    Ok((String::from_utf8(name[..length].to_vec()).unwrap(), kind))
}
fn closedir(directory: *mut c_void) { assert_eq!(unsafe { MemFs::close_directory(directory) }, Ok(())); }
fn list(path: &str) -> Vec<(String, u32)> {
    let directory = opendir(path).unwrap();
    let mut entries = Vec::new();
    loop {
        match next(directory) {
            Ok(entry) => entries.push(entry),
            Err(end) => { assert_eq!(end, Error::NotFound); break; }
        }
    }
    closedir(directory);
    entries
}
fn names(path: &str) -> Vec<String> { list(path).into_iter().map(|(name, _)| name).collect() }
fn put(path: &str, data: &[u8]) {
    let file = open(path, WRITE | CREATE | TRUNCATE).unwrap();
    if !data.is_empty() { assert_eq!(write(file, 0, data), Ok(data.len())); }
    close(file);
}
fn get(path: &str) -> Vec<u8> {
    let file = open(path, READ).unwrap();
    let data = read(file, 0, 1 << 20).unwrap();
    close(file);
    data
}

#[test]
fn the_root_exists_and_paths_normalize() {
    let _world = together();
    let root = stat("/").unwrap();
    assert_eq!((root.kind, root.size, root.device), (NODE_DIRECTORY, 0, 1));
    assert_ne!(root.identity, 0);
    mkdir("/norm").unwrap();
    mkdir("/norm/a").unwrap();
    put("/norm/a/f", b"x");
    let file = stat("/norm/a/f").unwrap();
    assert_eq!((file.kind, file.size), (NODE_FILE, 1));
    for same in ["//norm///a//f", "/norm/./a/./f", "/norm/a/../a/f", "/../../norm/a/f", "norm/a/f", "./norm/a/f", "../norm/a/f", "/norm/a/../../norm/a/f"] {
        assert_eq!(stat(same), Ok(file), "{same}");
    }
    let directory = stat("/norm/a").unwrap();
    for same in ["/norm/a/", "/norm/a//", "/norm/a/.", "/norm/a/./", "norm/a", "/norm/a/../a", "/norm/a/../a/"] {
        assert_eq!(stat(same), Ok(directory), "{same}");
    }
    // Other tests change the root's times while this one runs: compare what is stable.
    for same in ["//", "/.", "/..", "/../..", ".", "..", "./", "/norm/..", "/norm/a/../..", "/norm/a/../../.."] {
        let status = stat(same).unwrap();
        assert_eq!((status.kind, status.identity), (NODE_DIRECTORY, root.identity), "{same}");
    }
    assert_eq!(get("norm/a/../a/./f"), b"x");
}

#[test]
fn dot_components_and_trailing_slashes_need_a_directory() {
    let _world = together();
    mkdir("/dots").unwrap();
    put("/dots/file", b"x");
    assert_eq!(stat("/dots/missing/../file"), Err(Error::NotFound));
    assert_eq!(stat("/dots/file/../file"), Err(Error::NotDirectory));
    assert_eq!(stat("/dots/file/."), Err(Error::NotDirectory));
    assert_eq!(stat("/dots/file/"), Err(Error::NotDirectory));
    assert_eq!(open("/dots/file/", READ), Err(Error::NotDirectory));
    assert_eq!(remove("/dots/file/"), Err(Error::NotDirectory));
    assert_eq!(rename("/dots/file/", "/dots/other"), Err(Error::NotDirectory));
    // A path with a trailing slash never becomes a file.
    assert_eq!(open("/dots/new/", WRITE | CREATE), Err(Error::IsDirectory));
    assert_eq!(rename("/dots/file", "/dots/new/"), Err(Error::NotDirectory));
    assert_eq!(stat("/dots/new"), Err(Error::NotFound));
    assert_eq!(mkdir("/dots/made/"), Ok(()));
    assert_eq!(stat("/dots/made").unwrap().kind, NODE_DIRECTORY);
    assert_eq!(rename("/dots/made/", "/dots/moved/"), Ok(()));
    assert_eq!(names("/dots"), ["file", "moved"]);
}

#[test]
fn long_names_and_broken_intermediate_components() {
    let _world = together();
    mkdir("/names").unwrap();
    let long = format!("/names/{}", "n".repeat(256));
    assert_eq!(open(&long, WRITE | CREATE), Err(Error::NameTooLong));
    assert_eq!(mkdir(&long), Err(Error::NameTooLong));
    assert_eq!(stat(&long), Err(Error::NameTooLong));
    assert_eq!(stat(&format!("{long}/x")), Err(Error::NameTooLong));
    assert_eq!(rename("/names", &long), Err(Error::NameTooLong));
    let fits = "n".repeat(255);
    put(&format!("/names/{fits}"), b"ok");
    assert_eq!(list("/names"), [(fits, NODE_FILE)]);

    assert_eq!(stat("/names/missing/x"), Err(Error::NotFound));
    assert_eq!(open("/names/missing/x", WRITE | CREATE), Err(Error::NotFound));
    assert_eq!(mkdir("/names/missing/x"), Err(Error::NotFound));
    assert_eq!(opendir("/names/missing/x"), Err(Error::NotFound));
    put("/names/file", b"");
    assert_eq!(stat("/names/file/x"), Err(Error::NotDirectory));
    assert_eq!(open("/names/file/x", WRITE | CREATE), Err(Error::NotDirectory));
    assert_eq!(mkdir("/names/file/x"), Err(Error::NotDirectory));
    assert_eq!(remove("/names/file/x"), Err(Error::NotDirectory));
    assert_eq!(rmdir("/names/file/x"), Err(Error::NotDirectory));
    assert_eq!(rename("/names/file", "/names/file/x"), Err(Error::NotDirectory));
}

#[test]
fn open_honors_every_flag() {
    let _world = together();
    mkdir("/open").unwrap();
    assert_eq!(open("/open/f", READ), Err(Error::NotFound));
    assert_eq!(open("/open/f", READ | WRITE), Err(Error::NotFound));
    assert_eq!(open("/open/f", WRITE | TRUNCATE), Err(Error::NotFound));
    put("/open/f", b"content");
    // CREATE alone opens an existing file as it is.
    let file = open("/open/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(read(file, 0, 16).unwrap(), b"content");
    close(file);
    assert_eq!(open("/open/f", WRITE | CREATE | EXCLUSIVE), Err(Error::AlreadyExists));
    assert_eq!(open("/open", READ | CREATE | EXCLUSIVE), Err(Error::AlreadyExists));
    assert_eq!(get("/open/f"), b"content");
    let file = open("/open/g", READ | WRITE | CREATE | EXCLUSIVE).unwrap();
    assert_eq!((status(file).kind, status(file).size), (NODE_FILE, 0));
    close(file);

    assert_eq!(open("/open", READ), Err(Error::IsDirectory));
    assert_eq!(open("/open", WRITE | CREATE), Err(Error::IsDirectory));
    assert_eq!(open("/", READ), Err(Error::IsDirectory));
    assert_eq!(open("/open/missing/f", WRITE | CREATE), Err(Error::NotFound));
    assert_eq!(open("/open/f/g", WRITE | CREATE), Err(Error::NotDirectory));

    let file = open("/open/f", READ | WRITE | TRUNCATE).unwrap();
    assert_eq!(status(file).size, 0);
    assert_eq!(read(file, 0, 16).unwrap(), b"");
    close(file);
    put("/open/h", b"fresh");
    assert_eq!(get("/open/h"), b"fresh");
    put("/open/h", b"");
    assert_eq!(get("/open/h"), b"");
    assert_eq!(names("/open"), ["f", "g", "h"]);
}

#[test]
fn handles_enforce_their_access_and_modes_enforce_nothing() {
    let _world = together();
    mkdir("/access").unwrap();
    let writer = open_mode("/access/f", WRITE | CREATE, 0).unwrap();
    assert_eq!(write(writer, 0, b"data"), Ok(4));
    assert_eq!(read(writer, 0, 4), Err(Error::AccessDenied));
    assert_eq!(status(writer).mode, 0);
    assert_eq!(unsafe { MemFs::flush(writer) }, Ok(()));
    close(writer);
    // Mode 0 refuses nobody.
    let reader = open("/access/f", READ).unwrap();
    assert_eq!(read(reader, 0, 4).unwrap(), b"data");
    assert_eq!(write(reader, 0, b"x"), Err(Error::AccessDenied));
    assert_eq!(set_size(reader, 0), Err(Error::AccessDenied));
    assert_eq!(unsafe { MemFs::flush(reader) }, Ok(()));
    assert_eq!(status(reader).size, 4);
    close(reader);
    let both = open("/access/f", READ | WRITE).unwrap();
    assert_eq!(write(both, 4, b"!"), Ok(1));
    assert_eq!(read(both, 0, 8).unwrap(), b"data!");
    close(both);

    close(open_mode("/access/special", WRITE | CREATE, 0o6751).unwrap());
    assert_eq!(stat("/access/special").unwrap().mode, 0o6751);
    // The mode belongs to the call that creates the file.
    close(open_mode("/access/special", WRITE | CREATE, 0o600).unwrap());
    assert_eq!(stat("/access/special").unwrap().mode, 0o6751);
    assert_eq!(unsafe { MemFs::create_directory(b"/access/private", 0o700) }, Ok(()));
    assert_eq!(stat("/access/private").unwrap().mode, 0o700);
}

#[test]
fn reads_stop_at_the_end_and_writes_past_it_fill_with_zeros() {
    let _world = together();
    mkdir("/sparse").unwrap();
    let file = open("/sparse/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(read(file, 0, 8).unwrap(), b"");
    assert_eq!(write(file, 0, b"hello"), Ok(5));
    assert_eq!(read(file, 5, 8).unwrap(), b"");
    assert_eq!(read(file, 6, 8).unwrap(), b"");
    assert_eq!(read(file, u64::MAX, 8).unwrap(), b"");
    assert_eq!(read(file, 3, 8).unwrap(), b"lo");
    assert_eq!(read(file, 1, 2).unwrap(), b"el");
    assert_eq!(write(file, 8, b"world"), Ok(5));
    assert_eq!(status(file).size, 13);
    assert_eq!(read(file, 0, 32).unwrap(), b"hello\0\0\0world");
    assert_eq!(write(file, 3, b"LO.."), Ok(4));
    assert_eq!(read(file, 0, 32).unwrap(), b"helLO..\0world");
    // A write across the end overwrites the tail and extends the file.
    assert_eq!(write(file, 11, b"LD!!"), Ok(4));
    assert_eq!(read(file, 0, 32).unwrap(), b"helLO..\0worLD!!");
    assert_eq!(status(file).size, 15);
    close(file);
    let far = open("/sparse/far", READ | WRITE | CREATE).unwrap();
    assert_eq!(write(far, 100_000, b"end"), Ok(3));
    let data = read(far, 0, 200_000).unwrap();
    assert_eq!(data.len(), 100_003);
    assert!(data[..100_000].iter().all(|byte| *byte == 0));
    assert_eq!(&data[100_000..], b"end");
    close(far);
}

#[test]
fn set_size_grows_with_zeros_and_shrinks() {
    let _world = together();
    mkdir("/size").unwrap();
    let file = open("/size/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(write(file, 0, b"abcdef"), Ok(6));
    assert_eq!(set_size(file, 3), Ok(()));
    assert_eq!(read(file, 0, 16).unwrap(), b"abc");
    assert_eq!(set_size(file, 6), Ok(()));
    assert_eq!(read(file, 0, 16).unwrap(), b"abc\0\0\0");
    assert_eq!(set_size(file, 6), Ok(()));
    assert_eq!(status(file).size, 6);
    assert_eq!(set_size(file, 0), Ok(()));
    assert_eq!(read(file, 0, 16).unwrap(), b"");
    // Shrinking far below the allocation moves the content to a smaller one.
    assert_eq!(set_size(file, 1 << 20), Ok(()));
    assert_eq!(write(file, 0, b"kept"), Ok(4));
    assert_eq!(set_size(file, 4), Ok(()));
    assert_eq!(read(file, 0, 16).unwrap(), b"kept");
    assert_eq!(write(file, 4, b" and extended"), Ok(13));
    assert_eq!(stat("/size/f").unwrap().size, 17);
    close(file);
    assert_eq!(get("/size/f"), b"kept and extended");
}

#[test]
fn status_describes_files_and_directories() {
    let _world = together();
    mkdir("/status").unwrap();
    put("/status/f", b"12345");
    let by_path = stat("/status/f").unwrap();
    assert_eq!((by_path.kind, by_path.mode, by_path.size, by_path.device), (NODE_FILE, 0o644, 5, 1));
    let file = open("/status/f", READ).unwrap();
    assert_eq!(status(file), by_path);
    close(file);
    // Neither is a link, so there is nothing not to follow.
    assert_eq!(lstat("/status/f"), Ok(by_path));
    let directory = stat("/status").unwrap();
    assert_eq!((directory.kind, directory.mode, directory.size, directory.device), (NODE_DIRECTORY, 0o755, 0, 1));
    assert_eq!(lstat("/status"), Ok(directory));
    assert_eq!(stat("/status/missing"), Err(Error::NotFound));
}

#[test]
fn identities_are_nonzero_and_never_reused() {
    let _world = together();
    mkdir("/identity").unwrap();
    let mut seen = HashSet::from([stat("/").unwrap().identity, stat("/identity").unwrap().identity]);
    for round in 0..50 {
        put("/identity/f", b"x");
        mkdir("/identity/d").unwrap();
        for path in ["/identity/f", "/identity/d"] {
            let identity = stat(path).unwrap().identity;
            assert_ne!(identity, 0);
            assert!(seen.insert(identity), "identity {identity} came back in round {round}");
        }
        remove("/identity/f").unwrap();
        rmdir("/identity/d").unwrap();
    }
    // Moving a node keeps it the same node.
    put("/identity/before", b"x");
    let identity = stat("/identity/before").unwrap().identity;
    rename("/identity/before", "/identity/after").unwrap();
    assert_eq!(stat("/identity/after").unwrap().identity, identity);
}

#[test]
fn remove_deletes_files_only() {
    let _world = together();
    mkdir("/remove").unwrap();
    mkdir("/remove/d").unwrap();
    put("/remove/f", b"x");
    assert_eq!(remove("/remove/f"), Ok(()));
    assert_eq!(stat("/remove/f"), Err(Error::NotFound));
    assert_eq!(remove("/remove/f"), Err(Error::NotFound));
    assert_eq!(remove("/remove/d"), Err(Error::IsDirectory));
    assert_eq!(remove("/remove/d/."), Err(Error::IsDirectory));
    assert_eq!(remove("/"), Err(Error::IsDirectory));
    assert_eq!(remove("/remove/missing/f"), Err(Error::NotFound));
    assert_eq!(names("/remove"), ["d"]);
}

#[test]
fn an_open_handle_outlives_removal_and_replacement() {
    let _world = together();
    mkdir("/alive").unwrap();
    put("/alive/f", b"first");
    let file = open("/alive/f", READ | WRITE).unwrap();
    let identity = status(file).identity;
    assert_eq!(remove("/alive/f"), Ok(()));
    assert_eq!(stat("/alive/f"), Err(Error::NotFound));
    assert!(list("/alive").is_empty());
    assert_eq!(read(file, 0, 16).unwrap(), b"first");
    assert_eq!(write(file, 5, b" more"), Ok(5));
    assert_eq!(set_size(file, 9), Ok(()));
    assert_eq!((status(file).size, status(file).identity), (9, identity));
    // The name is free again, and the new file is another node.
    put("/alive/f", b"second");
    assert_ne!(stat("/alive/f").unwrap().identity, identity);
    assert_eq!(read(file, 0, 16).unwrap(), b"first mor");
    assert_eq!(get("/alive/f"), b"second");
    close(file);

    put("/alive/old", b"old");
    put("/alive/new", b"new");
    let old = open("/alive/old", READ).unwrap();
    assert_eq!(rename("/alive/new", "/alive/old"), Ok(()));
    assert_eq!(get("/alive/old"), b"new");
    assert_eq!(read(old, 0, 16).unwrap(), b"old");
    close(old);
    assert_eq!(names("/alive"), ["f", "old"]);
}

#[test]
fn rename_moves_replaces_and_refuses() {
    let _world = together();
    mkdir("/rename").unwrap();
    mkdir("/rename/a").unwrap();
    mkdir("/rename/b").unwrap();
    put("/rename/a/f", b"moved");
    assert_eq!(rename("/rename/a/f", "/rename/b/g"), Ok(()));
    assert_eq!(stat("/rename/a/f"), Err(Error::NotFound));
    assert_eq!(get("/rename/b/g"), b"moved");
    put("/rename/b/target", b"replaced");
    assert_eq!(rename("/rename/b/g", "/rename/b/target"), Ok(()));
    assert_eq!(get("/rename/b/target"), b"moved");
    assert_eq!(names("/rename/b"), ["target"]);

    assert_eq!(rename("/rename/b/target", "/rename/b/target"), Ok(()));
    assert_eq!(rename("/rename/b/target", "/rename/b/./../b/target"), Ok(()));
    assert_eq!(rename("/rename/b", "/rename/b/"), Ok(()));
    assert_eq!(get("/rename/b/target"), b"moved");
    assert_eq!(rename("/rename/missing", "/rename/x"), Err(Error::NotFound));
    assert_eq!(rename("/rename/missing", "/rename/missing"), Err(Error::NotFound));
    assert_eq!(rename("/rename/b/target", "/rename/missing/x"), Err(Error::NotFound));
    assert_eq!(rename("/rename/b/target", "/rename/a"), Err(Error::IsDirectory));
    assert_eq!(rename("/rename/a", "/rename/b/target"), Err(Error::NotDirectory));

    // A directory replaces an empty directory only.
    assert_eq!(rename("/rename/a", "/rename/b"), Err(Error::NotEmpty));
    mkdir("/rename/empty").unwrap();
    let moving = stat("/rename/b").unwrap().identity;
    assert_eq!(rename("/rename/b", "/rename/empty"), Ok(()));
    assert_eq!(stat("/rename/b"), Err(Error::NotFound));
    assert_eq!(stat("/rename/empty").unwrap().identity, moving);
    assert_eq!(get("/rename/empty/target"), b"moved");

    // Not into itself, however the destination is spelled.
    mkdir("/rename/a/inner").unwrap();
    assert_eq!(rename("/rename/a", "/rename/a/x"), Err(Error::InvalidArgument));
    assert_eq!(rename("/rename/a", "/rename/a/inner/x"), Err(Error::InvalidArgument));
    assert_eq!(rename("/rename/a", "/rename/empty/../a/inner/x"), Err(Error::InvalidArgument));
    assert_eq!(rename("/rename/a/inner", "/rename/a"), Err(Error::NotEmpty));
    // `..` of a moved directory is its new parent.
    assert_eq!(rename("/rename/a/inner", "/rename/empty/inner"), Ok(()));
    assert_eq!(stat("/rename/empty/inner/..").unwrap().identity, moving);
    assert_eq!(get("/rename/empty/inner/../target"), b"moved");
    assert_eq!(names("/rename"), ["a", "empty"]);

    // The root and paths ending in `.` or `..` name no entry to move or replace.
    assert_eq!(rename("/", "/rename/root"), Err(Error::Busy));
    assert_eq!(rename("/rename/a", "/"), Err(Error::Busy));
    assert_eq!(rename("/rename/empty/target", "/"), Err(Error::Busy));
    assert_eq!(rename("/rename/..", "/rename/root"), Err(Error::Busy));
    assert_eq!(rename("/rename/a/.", "/rename/x"), Err(Error::Busy));
    assert_eq!(rename("/rename/a", "/rename/empty/inner/.."), Err(Error::Busy));
    assert_eq!(names("/rename"), ["a", "empty"]);
}

#[test]
fn directories_are_created_once_and_removed_empty() {
    let _world = together();
    assert_eq!(mkdir("/dirs"), Ok(()));
    assert_eq!(mkdir("/dirs"), Err(Error::AlreadyExists));
    assert_eq!(mkdir("/"), Err(Error::AlreadyExists));
    assert_eq!(mkdir("/dirs/."), Err(Error::AlreadyExists));
    put("/dirs/f", b"x");
    assert_eq!(mkdir("/dirs/f"), Err(Error::AlreadyExists));
    assert_eq!(mkdir("/dirs/f/"), Err(Error::AlreadyExists));
    assert_eq!(mkdir("/dirs/missing/d"), Err(Error::NotFound));
    assert_eq!(mkdir("/dirs/f/d"), Err(Error::NotDirectory));
    assert_eq!(mkdir("/dirs/d"), Ok(()));
    assert_eq!(mkdir("/dirs/d/e"), Ok(()));

    assert_eq!(rmdir("/dirs/d"), Err(Error::NotEmpty));
    assert_eq!(rmdir("/dirs/f"), Err(Error::NotDirectory));
    assert_eq!(rmdir("/dirs/missing"), Err(Error::NotFound));
    assert_eq!(rmdir("/"), Err(Error::Busy));
    assert_eq!(rmdir("/dirs/.."), Err(Error::Busy));
    assert_eq!(rmdir("/dirs/d/e/."), Err(Error::InvalidArgument));
    assert_eq!(rmdir("/dirs/d/e/.."), Err(Error::NotEmpty));
    assert_eq!(rmdir("/dirs/d/e/"), Ok(()));
    assert_eq!(rmdir("/dirs/d"), Ok(()));
    assert_eq!(stat("/dirs/d"), Err(Error::NotFound));
    assert_eq!(list("/dirs"), [("f".to_string(), NODE_FILE)]);
}

#[test]
fn enumeration_is_a_sorted_snapshot() {
    let _world = together();
    mkdir("/list").unwrap();
    for name in ["b", "a", "c", "B", "10", "9"] { put(&format!("/list/{name}"), b"x"); }
    mkdir("/list/dir").unwrap();
    let directory = opendir("/list/.").unwrap();
    assert_eq!(next(directory), Ok(("10".to_string(), NODE_FILE)));
    // What a recursive delete does: empty the directory while enumerating it.
    for name in ["b", "a", "c", "B", "10", "9"] { remove(&format!("/list/{name}")).unwrap(); }
    rmdir("/list/dir").unwrap();
    put("/list/zz", b"late");
    for expected in ["9", "B", "a", "b", "c"] { assert_eq!(next(directory), Ok((expected.to_string(), NODE_FILE))); }
    assert_eq!(next(directory), Ok(("dir".to_string(), NODE_DIRECTORY)));
    assert_eq!(next(directory), Err(Error::NotFound));
    assert_eq!(next(directory), Err(Error::NotFound));
    closedir(directory);
    assert_eq!(list("/list"), [("zz".to_string(), NODE_FILE)]);

    mkdir("/list/empty").unwrap();
    assert!(list("/list/empty").is_empty());
    assert_eq!(opendir("/list/zz"), Err(Error::NotDirectory));
    assert_eq!(opendir("/list/missing"), Err(Error::NotFound));
    let root = list("/");
    assert!(root.contains(&("list".to_string(), NODE_DIRECTORY)));
    assert!(root.iter().all(|(name, _)| name != "." && name != ".."));
    assert!(root.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[test]
fn a_handle_of_the_wrong_kind_is_refused_and_stays_open() {
    let _world = together();
    mkdir("/kinds").unwrap();
    put("/kinds/f", b"x");
    let file = open("/kinds/f", READ | WRITE).unwrap();
    let directory = opendir("/kinds").unwrap();
    let mut byte = 0u8;
    assert_eq!(unsafe { MemFs::read_at(directory, 0, &mut byte, 1) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::write_at(directory, 0, &byte, 1) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::set_size(directory, 0) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::flush(directory) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::status(directory) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::close(directory) }, Err(Error::InvalidArgument));
    assert_eq!(next(file), Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::close_directory(file) }, Err(Error::InvalidArgument));
    assert_eq!(next(directory), Ok(("f".to_string(), NODE_FILE)));
    assert_eq!(read(file, 0, 1).unwrap(), b"x");
    closedir(directory);
    close(file);
}

#[test]
fn the_current_directory_starts_at_the_root() {
    let _world = together();
    let mut out = [0xeeu8; 4];
    assert_eq!(unsafe { MemFs::current_directory(out.as_mut_ptr(), 4) }, Ok(2));
    assert_eq!(out, [b'/', 0, 0xee, 0xee]);
    let mut exact = [0xeeu8; 2];
    assert_eq!(unsafe { MemFs::current_directory(exact.as_mut_ptr(), 2) }, Ok(2));
    assert_eq!(&exact, b"/\0");
    let mut short = [0xeeu8; 1];
    assert_eq!(unsafe { MemFs::current_directory(short.as_mut_ptr(), 1) }, Ok(2));
    assert_eq!(short, [0xee]);
    assert_eq!(unsafe { MemFs::current_directory(std::ptr::null_mut(), 0) }, Ok(2));
}

#[test]
fn modes_change_by_path_and_by_handle_and_still_enforce_nothing() {
    let _world = together();
    mkdir("/mode").unwrap();
    put("/mode/f", b"x");
    assert_eq!(chmod("/mode/f", 0o4751), Ok(()));
    assert_eq!(stat("/mode/f").unwrap().mode, 0o4751);
    assert_eq!(chmod("/mode", 0o500), Ok(()));
    assert_eq!(stat("/mode").unwrap().mode, 0o500);
    // A directory without write permission takes entries, a file without any is read.
    put("/mode/g", b"y");
    assert_eq!(chmod("/mode/f", 0), Ok(()));
    assert_eq!(get("/mode/f"), b"x");
    // The path is followed to the file; the link keeps the mode every link has.
    symlink("f", "/mode/link").unwrap();
    assert_eq!(chmod("/mode/link", 0o640), Ok(()));
    assert_eq!((stat("/mode/f").unwrap().mode, lstat("/mode/link").unwrap().mode), (0o640, 0o777));
    symlink("nowhere", "/mode/dangling").unwrap();
    assert_eq!(chmod("/mode/dangling", 0o600), Err(Error::NotFound));
    assert_eq!(chmod("/mode/missing", 0o600), Err(Error::NotFound));
    assert_eq!(chmod("/mode/f/x", 0o600), Err(Error::NotDirectory));

    // A handle needs no particular access, and the file no name.
    let reader = open("/mode/f", READ).unwrap();
    assert_eq!(unsafe { MemFs::set_file_mode(reader, 0o604) }, Ok(()));
    assert_eq!((status(reader).mode, stat("/mode/f").unwrap().mode), (0o604, 0o604));
    remove("/mode/f").unwrap();
    assert_eq!(unsafe { MemFs::set_file_mode(reader, 0o7777) }, Ok(()));
    assert_eq!(status(reader).mode, 0o7777);
    close(reader);
    let directory = opendir("/mode").unwrap();
    assert_eq!(unsafe { MemFs::set_file_mode(directory, 0o600) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::set_file_times(directory, Some(1), Some(1)) }, Err(Error::InvalidArgument));
    closedir(directory);
}

#[test]
fn a_hard_link_is_another_name_of_the_same_file() {
    let _world = together();
    mkdir("/hard").unwrap();
    mkdir("/hard/d").unwrap();
    put("/hard/f", b"shared");
    assert_eq!(link("/hard/f", "/hard/d/g"), Ok(()));
    assert_eq!(stat("/hard/d/g"), stat("/hard/f"));
    let file = open("/hard/d/g", WRITE).unwrap();
    assert_eq!(write(file, 6, b" bytes"), Ok(6));
    close(file);
    assert_eq!(get("/hard/f"), b"shared bytes");
    assert_eq!(list("/hard/d"), [("g".to_string(), NODE_FILE)]);

    // Refused in the order of Linux: the source, then the new name, then the kind of the source.
    assert_eq!(link("/hard/f", "/hard/d/g"), Err(Error::AlreadyExists));
    assert_eq!(link("/hard/f", "/hard/d"), Err(Error::AlreadyExists));
    assert_eq!(link("/hard/f", "/hard/d/"), Err(Error::AlreadyExists));
    assert_eq!(link("/hard/f", "/hard/d/g/"), Err(Error::AlreadyExists));
    assert_eq!(link("/hard/missing", "/hard/new"), Err(Error::NotFound));
    assert_eq!(link("/hard/missing", "/hard/f"), Err(Error::NotFound));
    assert_eq!(link("/hard/f", "/hard/missing/new"), Err(Error::NotFound));
    assert_eq!(link("/hard/f", "/hard/f/new"), Err(Error::NotDirectory));
    assert_eq!(link("/hard/f", "/hard/new/"), Err(Error::NotFound));
    assert_eq!(link("/hard/f/", "/hard/new"), Err(Error::NotDirectory));
    assert_eq!(link("/hard/d", "/hard/new"), Err(Error::AccessDenied));
    assert_eq!(link("/", "/hard/new"), Err(Error::AccessDenied));
    assert_eq!(link("/hard/d", "/hard/f"), Err(Error::AlreadyExists));
    assert_eq!(link("/hard/f", &format!("/hard/{}", "n".repeat(256))), Err(Error::NameTooLong));
    assert_eq!(stat("/hard/new"), Err(Error::NotFound));

    // Removing one name keeps the other, and the freed name can be given again.
    let identity = stat("/hard/f").unwrap().identity;
    assert_eq!(remove("/hard/f"), Ok(()));
    assert_eq!(stat("/hard/f"), Err(Error::NotFound));
    assert_eq!(get("/hard/d/g"), b"shared bytes");
    assert_eq!(link("/hard/d/g", "/hard/f"), Ok(()));
    assert_eq!(stat("/hard/f").unwrap().identity, identity);

    // Links before the final name of the source are followed; one in it gets the new name itself.
    symlink("d", "/hard/tod").unwrap();
    assert_eq!(link("/hard/tod/g", "/hard/via"), Ok(()));
    assert_eq!(stat("/hard/via").unwrap().identity, identity);
    assert_eq!(link("/hard/tod", "/hard/tod2"), Ok(()));
    assert_eq!(lstat("/hard/tod2"), lstat("/hard/tod"));
    assert_eq!((lstat("/hard/tod2").unwrap().kind, read_link("/hard/tod2").unwrap().as_str()), (NODE_SYMLINK, "d"));
    assert_eq!(link("/hard/tod/", "/hard/new"), Err(Error::AccessDenied));
    symlink("nowhere", "/hard/dangling").unwrap();
    assert_eq!(link("/hard/f", "/hard/dangling"), Err(Error::AlreadyExists));
    assert_eq!(link("/hard/dangling", "/hard/dangling2"), Ok(()));
    assert_eq!(read_link("/hard/dangling2").unwrap(), "nowhere");
    assert_eq!(stat("/hard/nowhere"), Err(Error::NotFound));
}

#[test]
fn rename_between_two_names_of_one_file_does_nothing() {
    let _world = together();
    mkdir("/relink").unwrap();
    put("/relink/a", b"A");
    link("/relink/a", "/relink/b").unwrap();
    let identity = stat("/relink/a").unwrap().identity;
    assert_eq!(rename("/relink/a", "/relink/b"), Ok(()));
    assert_eq!(rename("/relink/b", "/relink/a"), Ok(()));
    assert_eq!(names("/relink"), ["a", "b"]);
    assert_eq!((stat("/relink/a").unwrap().identity, stat("/relink/b").unwrap().identity), (identity, identity));

    // Replacing one name leaves the file to the other.
    put("/relink/c", b"C");
    assert_eq!(rename("/relink/c", "/relink/a"), Ok(()));
    assert_eq!((get("/relink/a"), get("/relink/b")), (b"C".to_vec(), b"A".to_vec()));
    assert_eq!(stat("/relink/b").unwrap().identity, identity);
    assert_eq!(names("/relink"), ["a", "b"]);
    // Moving one name moves nothing but the name.
    link("/relink/b", "/relink/b2").unwrap();
    mkdir("/relink/d").unwrap();
    assert_eq!(rename("/relink/b", "/relink/d/moved"), Ok(()));
    assert_eq!((stat("/relink/d/moved").unwrap().identity, stat("/relink/b2").unwrap().identity), (identity, identity));
    assert_eq!(remove("/relink/d/moved"), Ok(()));
    assert_eq!(get("/relink/b2"), b"A");
}

#[test]
fn a_symbolic_link_stores_its_target_as_given() {
    let _world = together();
    mkdir("/sym").unwrap();
    mkdir("/sym/d").unwrap();
    put("/sym/f", b"file");
    for (target, name) in [("f", "rel"), ("/sym/f", "abs"), ("nowhere/at/all", "dangling"), ("d//./", "odd"), ("..", "up")] {
        assert_eq!(symlink(target, &format!("/sym/{name}")), Ok(()), "{name}");
        assert_eq!(read_link(&format!("/sym/{name}")).unwrap(), target);
    }
    let described = lstat("/sym/dangling").unwrap();
    assert_eq!((described.kind, described.mode, described.size, described.device), (NODE_SYMLINK, 0o777, 14, 1));
    assert_ne!(described.identity, 0);
    assert_eq!(list("/sym"), [("abs", NODE_SYMLINK), ("d", NODE_DIRECTORY), ("dangling", NODE_SYMLINK), ("f", NODE_FILE), ("odd", NODE_SYMLINK),
        ("rel", NODE_SYMLINK), ("up", NODE_SYMLINK)].map(|(name, kind)| (name.to_string(), kind)));
    // What a handle describes is never the link it was opened through.
    let file = open("/sym/rel", READ).unwrap();
    assert_eq!(Ok(status(file)), stat("/sym/f"));
    close(file);

    // Whatever exists is in the way, a link that leads nowhere included.
    for taken in ["/sym/f", "/sym/f/", "/sym/dangling", "/sym/rel", "/sym/d", "/sym/d/", "/sym/odd/", "/", "/sym/d/."] {
        assert_eq!(symlink("x", taken), Err(Error::AlreadyExists), "{taken}");
    }
    assert_eq!(read_link("/sym/dangling").unwrap(), "nowhere/at/all");
    assert_eq!(symlink("x", "/sym/missing/l"), Err(Error::NotFound));
    assert_eq!(symlink("x", "/sym/new/"), Err(Error::NotFound));
    assert_eq!(symlink("x", "/sym/f/l"), Err(Error::NotDirectory));
    assert_eq!(symlink("x", &format!("/sym/{}", "n".repeat(256))), Err(Error::NameTooLong));
    assert_eq!(symlink("", "/sym/empty"), Err(Error::NotFound));
    // The longest text the boundary can hand back is the longest one kept.
    let longest = "t".repeat(dotnet_pal_rs::runtime::MAX_NAME);
    assert_eq!(symlink(&format!("{longest}t"), "/sym/long"), Err(Error::NameTooLong));
    assert_eq!(symlink(&longest, "/sym/long"), Ok(()));
    assert_eq!(read_link("/sym/long").unwrap(), longest);

    assert_eq!(read_link("/sym/f"), Err(Error::InvalidArgument));
    assert_eq!(read_link("/sym/d"), Err(Error::InvalidArgument));
    assert_eq!(read_link("/"), Err(Error::InvalidArgument));
    assert_eq!(read_link("/sym/missing"), Err(Error::NotFound));
    assert_eq!(read_link("/sym/f/x"), Err(Error::NotDirectory));
    // A slash after the link asks for what is behind it, and that is no link.
    assert_eq!(read_link("/sym/odd/"), Err(Error::InvalidArgument));
    assert_eq!(read_link("/sym/rel/"), Err(Error::NotDirectory));
    assert_eq!(read_link("/sym/dangling/"), Err(Error::NotFound));
}

#[test]
fn links_are_followed_where_linux_follows_them() {
    let _world = together();
    for directory in ["/follow", "/follow/d", "/follow/d/sub"] { mkdir(directory).unwrap(); }
    put("/follow/f", b"top");
    put("/follow/d/f", b"inner");
    for (target, name) in [("d", "tod"), ("f", "tof"), ("../f", "d/sub/up"), ("/follow/d", "abs"), ("nowhere", "dangling"), ("gone", "dangling2")] {
        symlink(target, &format!("/follow/{name}")).unwrap();
    }
    // Before the final name always, also for a call that leaves a final link alone.
    assert_eq!(get("/follow/tod/f"), b"inner");
    assert_eq!(lstat("/follow/tod/f"), stat("/follow/d/f"));
    // A relative target starts at the directory holding the link, however that was reached.
    for path in ["/follow/d/sub/up", "/follow/tod/sub/up", "/follow/abs/sub/up", "follow/tod/../tod/sub/up"] { assert_eq!(get(path), b"inner", "{path}"); }
    // `..` leaves the directory the link led to, not the one holding the link.
    assert_eq!(stat("/follow/abs/sub/..").unwrap().identity, stat("/follow/d").unwrap().identity);
    assert_eq!(get("/follow/tod/../f"), b"top");

    assert_eq!(stat("/follow/tof"), stat("/follow/f"));
    assert_eq!(lstat("/follow/tof").unwrap().kind, NODE_SYMLINK);
    assert_eq!(stat("/follow/dangling"), Err(Error::NotFound));
    assert_eq!(lstat("/follow/dangling").unwrap().kind, NODE_SYMLINK);
    assert_eq!(lstat("/follow/tod/"), stat("/follow/d"));
    assert_eq!(lstat("/follow/tof/"), Err(Error::NotDirectory));
    assert_eq!(stat("/follow/tof/"), Err(Error::NotDirectory));
    assert_eq!(lstat("/follow/dangling/"), Err(Error::NotFound));

    // `open` writes through a link and creates what a dangling one names, unless it is to create the name itself.
    put("/follow/tof", b"through");
    assert_eq!(get("/follow/f"), b"through");
    assert_eq!(lstat("/follow/tof").unwrap().kind, NODE_SYMLINK);
    assert_eq!(open("/follow/dangling", READ), Err(Error::NotFound));
    assert_eq!(open("/follow/dangling", WRITE | CREATE | EXCLUSIVE), Err(Error::AlreadyExists));
    assert_eq!(open("/follow/tof", WRITE | CREATE | EXCLUSIVE), Err(Error::AlreadyExists));
    assert_eq!(open("/follow/dangling/", WRITE | CREATE), Err(Error::IsDirectory));
    assert_eq!(stat("/follow/nowhere"), Err(Error::NotFound));
    put("/follow/dangling", b"made");
    assert_eq!(get("/follow/nowhere"), b"made");
    assert_eq!(lstat("/follow/dangling").unwrap().kind, NODE_SYMLINK);
    assert_eq!(open("/follow/tod", READ), Err(Error::IsDirectory));

    assert_eq!(names("/follow/tod"), ["f", "sub"]);
    assert_eq!(names("/follow/tod/"), ["f", "sub"]);
    assert_eq!(opendir("/follow/tof"), Err(Error::NotDirectory));
    assert_eq!(opendir("/follow/dangling2"), Err(Error::NotFound));

    // A directory is made behind a link on the way, never behind one in the way.
    for taken in ["/follow/tof", "/follow/tod", "/follow/tod/", "/follow/dangling2", "/follow/dangling2/"] { assert_eq!(mkdir(taken), Err(Error::AlreadyExists), "{taken}"); }
    assert_eq!(stat("/follow/gone"), Err(Error::NotFound));
    assert_eq!(mkdir("/follow/tod/made"), Ok(()));
    assert_eq!(stat("/follow/d/made").unwrap().kind, NODE_DIRECTORY);

    // Removing and renaming work on the link, and a slash after one is refused.
    for path in ["/follow/tod", "/follow/tod/", "/follow/dangling2"] { assert_eq!(rmdir(path), Err(Error::NotDirectory), "{path}"); }
    for path in ["/follow/tod/", "/follow/tof/"] { assert_eq!(remove(path), Err(Error::NotDirectory), "{path}"); }
    assert_eq!(rename("/follow/tod/", "/follow/x"), Err(Error::NotDirectory));
    assert_eq!(rename("/follow/f", "/follow/tod/"), Err(Error::NotDirectory));
    assert_eq!(rename("/follow/f", "/follow/dangling2/"), Err(Error::NotDirectory));
    assert_eq!(remove("/follow/tod"), Ok(()));
    assert_eq!((lstat("/follow/tod"), stat("/follow/d").unwrap().kind), (Err(Error::NotFound), NODE_DIRECTORY));
    assert_eq!(remove("/follow/dangling2"), Ok(()));
    assert_eq!(rename("/follow/tof", "/follow/moved"), Ok(()));
    assert_eq!((read_link("/follow/moved").unwrap().as_str(), get("/follow/f")), ("f", b"through".to_vec()));
    assert_eq!(rename("/follow/d", "/follow/abs"), Err(Error::NotDirectory));
    assert_eq!(rename("/follow/abs", "/follow/d"), Err(Error::IsDirectory));
    put("/follow/g", b"g");
    assert_eq!(rename("/follow/g", "/follow/abs"), Ok(()));
    assert_eq!((lstat("/follow/abs").unwrap().kind, get("/follow/abs"), get("/follow/d/f")), (NODE_FILE, b"g".to_vec(), b"inner".to_vec()));
}

#[test]
fn a_walk_gives_up_after_forty_links() {
    let _world = together();
    mkdir("/loop").unwrap();
    symlink("b", "/loop/a").unwrap();
    symlink("a", "/loop/b").unwrap();
    assert_eq!(stat("/loop/a"), Err(Error::Os));
    assert_eq!(stat("/loop/a/x"), Err(Error::Os));
    assert_eq!(open("/loop/a", WRITE | CREATE), Err(Error::Os));
    assert_eq!(real_path("/loop/b"), Err(Error::Os));
    assert_eq!(lstat("/loop/a").unwrap().kind, NODE_SYMLINK);

    put("/loop/end", b"end");
    symlink("end", "/loop/1").unwrap();
    for link in 2..=41 { symlink(&(link - 1).to_string(), &format!("/loop/{link}")).unwrap(); }
    assert_eq!(get("/loop/40"), b"end");
    assert_eq!(stat("/loop/41"), Err(Error::Os));
    assert_eq!(read_link("/loop/41").unwrap(), "40");
    // The bound is on the links of one walk, one after the other as much as one inside the other.
    symlink(".", "/loop/self").unwrap();
    assert_eq!(get(&format!("/loop/{}end", "self/".repeat(40))), b"end");
    assert_eq!(stat(&format!("/loop/{}end", "self/".repeat(41))), Err(Error::Os));
    assert_eq!(remove("/loop/a"), Ok(()));
    assert_eq!(stat("/loop/b"), Err(Error::NotFound));
}

#[test]
fn real_path_is_absolute_and_free_of_links() {
    let _world = together();
    for directory in ["/real", "/real/d", "/real/d/sub"] { mkdir(directory).unwrap(); }
    put("/real/f", b"x");
    for (target, name) in [("../f", "d/up"), ("/real/d", "abs"), ("nowhere", "dangling"), ("/", "root")] { symlink(target, &format!("/real/{name}")).unwrap(); }
    for (path, real) in [("/real/f", "/real/f"), ("/real/d/up", "/real/f"), ("/real/abs/sub/..//./up", "/real/f"), ("/real/abs/", "/real/d"),
        ("/real/abs/sub", "/real/d/sub"), ("real/d/../f", "/real/f"), ("/real/d/sub/../..", "/real"), ("/real/root/real/abs", "/real/d"),
        ("/", "/"), ("/..", "/"), (".", "/"), ("/real/root", "/"), ("//real//", "/real")] {
        assert_eq!(real_path(path).as_deref(), Ok(real), "{path}");
    }
    assert_eq!(real_path("/real/dangling"), Err(Error::NotFound));
    assert_eq!(real_path("/real/d/missing"), Err(Error::NotFound));
    assert_eq!(real_path("/real/missing/.."), Err(Error::NotFound));
    assert_eq!(real_path("/real/f/"), Err(Error::NotDirectory));
    assert_eq!(real_path("/real/f/.."), Err(Error::NotDirectory));
}

#[test]
fn whole_file_locks_are_shared_or_exclusive_between_handles() {
    let _world = together();
    mkdir("/flock").unwrap();
    put("/flock/f", b"x");
    put("/flock/other", b"y");
    let (a, b) = (open("/flock/f", READ).unwrap(), open("/flock/f", READ).unwrap());
    assert_eq!(lock(a, LOCK_SHARED), Ok(()));
    assert_eq!(lock(b, LOCK_SHARED), Ok(()));
    // As with flock(2), a conversion gives the held lock up first: refused, `a` is left
    // with none, which is why `b` can convert.
    assert_eq!(lock(a, LOCK_EXCLUSIVE), Err(Error::WouldBlock));
    assert_eq!(lock(b, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock(a, LOCK_SHARED), Err(Error::WouldBlock));
    assert_eq!(lock(b, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock(b, LOCK_SHARED), Ok(()));
    assert_eq!(lock(a, LOCK_SHARED), Ok(()));
    let other = open("/flock/other", READ).unwrap();
    assert_eq!(lock(other, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock(a, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock(a, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock(b, LOCK_EXCLUSIVE), Ok(()));

    // The lock is on the file, whichever name a handle came through, and it stops no transfer.
    link("/flock/f", "/flock/g").unwrap();
    let c = open("/flock/g", READ | WRITE).unwrap();
    assert_eq!(lock(c, LOCK_SHARED), Err(Error::WouldBlock));
    assert_eq!(write(c, 0, b"z"), Ok(1));
    assert_eq!(read(a, 0, 1).unwrap(), b"z");
    // Closing releases it, and a file without a name is locked like any other.
    close(b);
    assert_eq!(lock(c, LOCK_EXCLUSIVE), Ok(()));
    remove("/flock/f").unwrap();
    remove("/flock/g").unwrap();
    assert_eq!(lock(a, LOCK_SHARED), Err(Error::WouldBlock));
    close(c);
    assert_eq!(lock(a, LOCK_EXCLUSIVE), Ok(()));
    close(a);
    close(other);
    let directory = opendir("/flock").unwrap();
    assert_eq!(lock(directory, LOCK_SHARED), Err(Error::InvalidArgument));
    assert_eq!(lock_range(directory, 0, 1, LOCK_SHARED), Err(Error::InvalidArgument));
    closedir(directory);
}

#[test]
fn range_locks_conflict_where_they_overlap() {
    let _world = together();
    mkdir("/range").unwrap();
    put("/range/f", b"0123456789");
    let (a, b) = (open("/range/f", READ | WRITE).unwrap(), open("/range/f", READ | WRITE).unwrap());
    // A shared lock needs a handle that reads and an exclusive one a handle that writes; unlocking needs neither.
    let (reader, writer) = (open("/range/f", READ).unwrap(), open("/range/f", WRITE).unwrap());
    assert_eq!(lock_range(reader, 200, 1, LOCK_EXCLUSIVE), Err(Error::AccessDenied));
    assert_eq!(lock_range(writer, 200, 1, LOCK_SHARED), Err(Error::AccessDenied));
    assert_eq!(lock_range(reader, 200, 1, LOCK_SHARED), Ok(()));
    assert_eq!(lock_range(writer, 201, 1, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock_range(reader, 200, 1, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock_range(writer, 201, 1, LOCK_UNLOCK), Ok(()));
    // Whole-file locks ask for no access, as flock(2) does not.
    assert_eq!(lock(reader, LOCK_EXCLUSIVE), Ok(()));
    close(reader);
    close(writer);
    assert_eq!(lock_range(a, 0, 10, LOCK_EXCLUSIVE), Ok(()));
    // Neighbours do not overlap, and a locked range may lie past the end.
    assert_eq!(lock_range(b, 10, 10, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock_range(b, 5, 10, LOCK_SHARED), Err(Error::WouldBlock));
    // Unlocking the middle of a lock leaves its two ends.
    assert_eq!(lock_range(a, 3, 4, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock_range(b, 3, 4, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock_range(b, 2, 2, LOCK_EXCLUSIVE), Err(Error::WouldBlock));
    assert_eq!(lock_range(b, 6, 2, LOCK_SHARED), Err(Error::WouldBlock));
    // A handle changes the mode of its own lock; shared locks overlap.
    assert_eq!(lock_range(a, 0, 3, LOCK_SHARED), Ok(()));
    assert_eq!(lock_range(b, 0, 3, LOCK_SHARED), Ok(()));
    // A refused call changes nothing: `a` still holds its shared lock.
    assert_eq!(lock_range(a, 0, 3, LOCK_EXCLUSIVE), Err(Error::WouldBlock));
    assert_eq!(lock_range(b, 0, 1, LOCK_EXCLUSIVE), Err(Error::WouldBlock));
    // Unlocking what is not held is fine and releases nothing of another handle.
    assert_eq!(lock_range(b, 100, 5, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock_range(b, 7, 3, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock_range(b, 7, 3, LOCK_SHARED), Err(Error::WouldBlock));
    assert_eq!(lock_range(a, i64::MAX as u64 - 1, 1, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock_range(b, i64::MAX as u64 - 1, 1, LOCK_SHARED), Err(Error::WouldBlock));
    assert_eq!(lock_range(b, i64::MAX as u64 - 2, 1, LOCK_SHARED), Ok(()));

    // Range locks and whole-file locks do not see each other.
    assert_eq!(lock(a, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock_range(b, 50, 1, LOCK_EXCLUSIVE), Ok(()));
    assert_eq!(lock(b, LOCK_SHARED), Err(Error::WouldBlock));
    assert_eq!(lock(a, LOCK_UNLOCK), Ok(()));
    assert_eq!(lock_range(b, 0, 3, LOCK_SHARED), Ok(()));

    // Closing releases every range of the handle.
    close(b);
    assert_eq!(lock_range(a, 0, 1000, LOCK_EXCLUSIVE), Ok(()));
    let c = open("/range/f", READ | WRITE).unwrap();
    assert_eq!(lock_range(c, 999, 1, LOCK_SHARED), Err(Error::WouldBlock));
    assert_eq!(lock_range(c, 1000, 1, LOCK_SHARED), Ok(()));
    close(a);
    assert_eq!(lock_range(c, 0, 1000, LOCK_EXCLUSIVE), Ok(()));
    // The front end never passes these on.
    assert_eq!(lock_range(c, 0, 0, LOCK_SHARED), Err(Error::InvalidArgument));
    assert_eq!(lock_range(c, u64::MAX, 1, LOCK_SHARED), Err(Error::InvalidArgument));
    close(c);
}

#[test]
fn the_tree_is_one_volume_with_the_capacity_it_was_given() {
    use dotnet_pal_rs::port::Volumes;
    let _world = alone();
    mkdir("/volume").unwrap();
    let base = used_bytes();
    set_capacity(base + 1000);
    put("/volume/f", &[1; 300]);
    let status = <MemFs as Volumes>::status(b"/volume/f").unwrap();
    assert_eq!((status.total_bytes, status.free_bytes, status.available_bytes), (base + 1000, 700, 700));
    assert_eq!(&status.format[..6], b"memfs\0");
    assert_eq!(<MemFs as Volumes>::status(b"/volume/missing").err(), Some(Error::NotFound));
    // What is stored stays stored when the capacity drops below it; nothing is free then.
    set_capacity(base + 100);
    let status = <MemFs as Volumes>::status(b"/").unwrap();
    assert_eq!((status.total_bytes, status.free_bytes), (base + 300, 0));
    let mut name = [0xffu8; 8];
    assert_eq!(unsafe { <MemFs as Volumes>::entry(0, name.as_mut_ptr(), name.len()) }, Ok(2));
    assert_eq!(&name[..3], b"/\0\xff");
    assert_eq!(unsafe { <MemFs as Volumes>::entry(0, name.as_mut_ptr(), 1) }, Ok(2));
    assert_eq!(unsafe { <MemFs as Volumes>::entry(1, name.as_mut_ptr(), name.len()) }, Err(Error::NotFound));
    remove("/volume/f").unwrap();
    set_capacity(64 * 1024 * 1024);
}

#[test]
fn capacity_bounds_content_and_is_returned_by_the_last_handle() {
    let _world = alone();
    mkdir("/capacity").unwrap();
    let base = used_bytes();
    set_capacity(base + 100);
    let file = open("/capacity/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(write(file, 0, &[7; 100]), Ok(100));
    assert_eq!(used_bytes(), base + 100);
    assert_eq!(write(file, 100, b"x"), Err(Error::NoSpace));
    // A write that does not fit changes nothing, not even the part that would.
    assert_eq!(write(file, 50, &[1; 51]), Err(Error::NoSpace));
    assert_eq!(write(file, 1000, b"x"), Err(Error::NoSpace));
    assert_eq!(set_size(file, 101), Err(Error::NoSpace));
    assert_eq!(read(file, 0, 200).unwrap(), [7; 100]);
    assert_eq!(write(file, 0, &[8; 100]), Ok(100));
    assert_eq!(used_bytes(), base + 100);
    assert_eq!(set_size(file, 40), Ok(()));
    assert_eq!(used_bytes(), base + 40);
    assert_eq!(write(file, 90, &[9; 10]), Ok(10));
    assert_eq!(used_bytes(), base + 100);

    // A removed file keeps its bytes until its last handle closes.
    let second = open("/capacity/f", READ).unwrap();
    assert_eq!(remove("/capacity/f"), Ok(()));
    assert_eq!(used_bytes(), base + 100);
    let other = open("/capacity/other", WRITE | CREATE).unwrap();
    assert_eq!(write(other, 0, b"x"), Err(Error::NoSpace));
    close(file);
    assert_eq!(used_bytes(), base + 100);
    close(second);
    assert_eq!(used_bytes(), base);
    assert_eq!(write(other, 0, b"x"), Ok(1));
    close(other);
    assert_eq!(remove("/capacity/other"), Ok(()));
    assert_eq!(used_bytes(), base);

    // So does a file that a rename replaced.
    put("/capacity/a", &[1; 60]);
    put("/capacity/b", &[2; 40]);
    let replaced = open("/capacity/a", READ).unwrap();
    assert_eq!(rename("/capacity/b", "/capacity/a"), Ok(()));
    assert_eq!(used_bytes(), base + 100);
    close(replaced);
    assert_eq!(used_bytes(), base + 40);
    close(open("/capacity/a", WRITE | TRUNCATE).unwrap());
    assert_eq!(used_bytes(), base);

    // A bound below the bytes in use refuses growth and allows everything else.
    put("/capacity/a", &[3; 10]);
    set_capacity(0);
    let file = open("/capacity/a", READ | WRITE).unwrap();
    assert_eq!(write(file, 10, b"x"), Err(Error::NoSpace));
    assert_eq!(write(file, 0, b"y"), Ok(1));
    assert_eq!(set_size(file, 5), Ok(()));
    close(file);
    assert_eq!(remove("/capacity/a"), Ok(()));
    assert_eq!(used_bytes(), base);
    set_capacity(64 * 1024 * 1024);
}

#[test]
#[cfg_attr(miri, ignore = "Miri ends the run on an allocation it cannot satisfy instead of failing it")]
fn an_allocation_the_allocator_refuses_is_no_space() {
    let _world = alone();
    mkdir("/exhausted").unwrap();
    let base = used_bytes();
    set_capacity(u64::MAX);
    let file = open("/exhausted/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(write(file, 0, b"kept"), Ok(4));
    assert_eq!(set_size(file, 1 << 60), Err(Error::NoSpace));
    assert_eq!(write(file, 1 << 60, b"x"), Err(Error::NoSpace));
    assert_eq!(write(file, i64::MAX as u64, b"x"), Err(Error::NoSpace));
    assert_eq!(read(file, 0, 16).unwrap(), b"kept");
    assert_eq!(used_bytes(), base + 4);
    close(file);
    set_capacity(64 * 1024 * 1024);
}

static NOW: AtomicU64 = AtomicU64::new(0);
fn at(now: u64) { NOW.store(now, Ordering::SeqCst); }
fn times(status: Status) -> [u64; 4] { [status.created_ns, status.modified_ns, status.changed_ns, status.accessed_ns] }

#[test]
fn timestamps_follow_the_injected_clock() {
    let _world = alone();
    set_clock(|| NOW.load(Ordering::SeqCst));
    at(1000);
    mkdir("/time").unwrap();
    assert_eq!(times(stat("/time").unwrap()), [1000; 4]);
    at(2000);
    let file = open("/time/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(times(status(file)), [2000; 4]);
    assert_eq!(times(stat("/time").unwrap()), [1000, 2000, 2000, 1000]);
    at(3000);
    assert_eq!(write(file, 0, b"data"), Ok(4));
    // Reads are not tracked and a write is not a read: the access time stays where it was.
    assert_eq!(times(status(file)), [2000, 3000, 3000, 2000]);
    assert_eq!(times(stat("/time").unwrap()), [1000, 2000, 2000, 1000]);
    // Reading, describing and reopening leave every time alone.
    at(3500);
    assert_eq!(read(file, 0, 4).unwrap(), b"data");
    close(open("/time/f", READ | WRITE | CREATE).unwrap());
    assert_eq!(times(status(file)), [2000, 3000, 3000, 2000]);
    at(4000);
    assert_eq!(set_size(file, 2), Ok(()));
    assert_eq!(times(status(file)), [2000, 4000, 4000, 2000]);
    // A rename changes the node's metadata, not its content.
    at(5000);
    assert_eq!(rename("/time/f", "/time/g"), Ok(()));
    assert_eq!(times(status(file)), [2000, 4000, 5000, 2000]);
    assert_eq!(times(stat("/time").unwrap()), [1000, 5000, 5000, 1000]);
    at(6000);
    close(open("/time/g", WRITE | TRUNCATE).unwrap());
    assert_eq!(times(status(file)), [2000, 6000, 6000, 2000]);
    at(7000);
    assert_eq!(remove("/time/g"), Ok(()));
    assert_eq!(times(status(file)), [2000, 6000, 7000, 2000]);
    assert_eq!(times(stat("/time").unwrap()), [1000, 7000, 7000, 1000]);
    close(file);
    at(8000);
    mkdir("/time/sub").unwrap();
    assert_eq!(times(stat("/time/sub").unwrap()), [8000; 4]);
    assert_eq!(times(stat("/time").unwrap()), [1000, 8000, 8000, 1000]);
    at(9000);
    mkdir("/time/other").unwrap();
    at(10_000);
    assert_eq!(rename("/time/sub", "/time/other/sub"), Ok(()));
    assert_eq!(times(stat("/time/other/sub").unwrap()), [8000, 8000, 10_000, 8000]);
    assert_eq!(times(stat("/time/other").unwrap()), [9000, 10_000, 10_000, 9000]);
    assert_eq!(times(stat("/time").unwrap()), [1000, 10_000, 10_000, 1000]);
    at(11_000);
    assert_eq!(rmdir("/time/other/sub"), Ok(()));
    assert_eq!(times(stat("/time/other").unwrap()), [9000, 11_000, 11_000, 9000]);
    set_clock(|| 0);
}

#[test]
fn explicit_times_are_kept_apart_and_mark_the_change() {
    let _world = alone();
    set_clock(|| NOW.load(Ordering::SeqCst));
    at(1000);
    mkdir("/stamp").unwrap();
    put("/stamp/f", b"x");
    symlink("f", "/stamp/link").unwrap();
    symlink("nowhere", "/stamp/dangling").unwrap();
    at(2000);
    assert_eq!(utimes("/stamp/f", true, Some(11), Some(22)), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 22, 2000, 11]);
    at(3000);
    assert_eq!(utimes("/stamp/f", true, None, Some(33)), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 33, 3000, 11]);
    at(4000);
    assert_eq!(utimes("/stamp/f", true, Some(44), None), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 33, 4000, 44]);
    // Keeping both times is not a change.
    at(5000);
    assert_eq!(utimes("/stamp/f", true, None, None), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 33, 4000, 44]);
    // A write moves the modification time and leaves the access time that was set.
    at(6000);
    let file = open("/stamp/f", READ).unwrap();
    put("/stamp/f", b"y");
    assert_eq!(times(status(file)), [1000, 6000, 6000, 44]);
    // A handle needs no write access to set them.
    at(7000);
    assert_eq!(unsafe { MemFs::set_file_times(file, Some(77), None) }, Ok(()));
    assert_eq!(unsafe { MemFs::set_file_times(file, None, Some(78)) }, Ok(()));
    assert_eq!(times(status(file)), [1000, 78, 7000, 77]);
    at(7500);
    assert_eq!(unsafe { MemFs::set_file_times(file, None, None) }, Ok(()));
    assert_eq!(times(status(file)), [1000, 78, 7000, 77]);
    close(file);

    // Followed, a link passes the times on; not followed, it takes them.
    at(8000);
    assert_eq!(utimes("/stamp/link", true, Some(81), Some(82)), Ok(()));
    assert_eq!((times(stat("/stamp/f").unwrap()), times(lstat("/stamp/link").unwrap())), ([1000, 82, 8000, 81], [1000; 4]));
    at(9000);
    assert_eq!(utimes("/stamp/link", false, Some(91), None), Ok(()));
    assert_eq!((times(stat("/stamp/f").unwrap()), times(lstat("/stamp/link").unwrap())), ([1000, 82, 8000, 81], [1000, 1000, 9000, 91]));
    assert_eq!(utimes("/stamp/dangling", true, Some(1), Some(2)), Err(Error::NotFound));
    assert_eq!(utimes("/stamp/dangling", false, Some(1), Some(2)), Ok(()));
    assert_eq!(times(lstat("/stamp/dangling").unwrap()), [1000, 2, 9000, 1]);
    assert_eq!(utimes("/stamp/missing", false, Some(1), Some(2)), Err(Error::NotFound));
    assert_eq!(utimes("/stamp", true, Some(5), Some(6)), Ok(()));
    assert_eq!(times(stat("/stamp").unwrap()), [1000, 6, 9000, 5]);

    // A new mode and a new name change the node, not its content; the name changes its directory.
    at(10_000);
    assert_eq!(chmod("/stamp/f", 0o600), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 82, 10_000, 81]);
    at(11_000);
    assert_eq!(link("/stamp/f", "/stamp/g"), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 82, 11_000, 81]);
    assert_eq!(times(stat("/stamp").unwrap()), [1000, 11_000, 11_000, 5]);
    at(12_000);
    assert_eq!(remove("/stamp/g"), Ok(()));
    assert_eq!(times(stat("/stamp/f").unwrap()), [1000, 82, 12_000, 81]);
    at(13_000);
    assert_eq!(symlink("f", "/stamp/late"), Ok(()));
    assert_eq!(times(lstat("/stamp/late").unwrap()), [13_000; 4]);
    assert_eq!(times(stat("/stamp").unwrap()), [1000, 13_000, 13_000, 5]);
    set_clock(|| 0);
}

#[test]
fn content_is_freed_with_the_last_name_and_the_last_handle() {
    let _world = alone();
    mkdir("/counted").unwrap();
    let base = used_bytes();
    put("/counted/a", &[1; 50]);
    link("/counted/a", "/counted/b").unwrap();
    link("/counted/b", "/counted/c").unwrap();
    // Three names, one content; link targets are not content.
    symlink(&"t".repeat(1000), "/counted/link").unwrap();
    assert_eq!(used_bytes(), base + 50);
    assert_eq!(remove("/counted/a"), Ok(()));
    assert_eq!(remove("/counted/b"), Ok(()));
    assert_eq!(used_bytes(), base + 50);
    // The last name goes to another file while a handle is open.
    let file = open("/counted/c", READ).unwrap();
    put("/counted/new", &[2; 7]);
    assert_eq!(rename("/counted/new", "/counted/c"), Ok(()));
    assert_eq!(used_bytes(), base + 57);
    assert_eq!(read(file, 0, 64).unwrap(), [1; 50]);
    close(file);
    assert_eq!(used_bytes(), base + 7);
    // A name given while the file is open keeps it past the handle.
    let file = open("/counted/c", READ).unwrap();
    link("/counted/c", "/counted/d").unwrap();
    assert_eq!(remove("/counted/c"), Ok(()));
    close(file);
    assert_eq!((used_bytes(), get("/counted/d")), (base + 7, vec![2; 7]));
    assert_eq!(remove("/counted/d"), Ok(()));
    assert_eq!(used_bytes(), base);
}

#[test]
fn the_current_directory_can_be_set_moved_and_removed() {
    let _world = alone();
    for directory in ["/cwd", "/cwd/a", "/cwd/a/in"] { mkdir(directory).unwrap(); }
    put("/cwd/f", b"x");
    symlink("a/in", "/cwd/link").unwrap();
    symlink("nowhere", "/cwd/dangling").unwrap();
    assert_eq!(chdir("/cwd/f"), Err(Error::NotDirectory));
    assert_eq!(chdir("/cwd/missing"), Err(Error::NotFound));
    assert_eq!(chdir("/cwd/dangling"), Err(Error::NotFound));
    assert_eq!(cwd().unwrap(), "/");
    // Set through a link, it is reported without one.
    assert_eq!(chdir("/cwd/link"), Ok(()));
    assert_eq!(cwd().unwrap(), "/cwd/a/in");
    put("rel", b"r");
    assert_eq!(get("/cwd/a/in/rel"), b"r");
    assert_eq!(stat("../../f"), stat("/cwd/f"));
    assert_eq!(stat("/cwd/f").unwrap().kind, NODE_FILE);
    assert_eq!((real_path("rel").unwrap().as_str(), real_path("..").unwrap().as_str(), real_path(".").unwrap().as_str()), ("/cwd/a/in/rel", "/cwd/a", "/cwd/a/in"));
    // A relative link target starts where the link is, not where the caller is.
    symlink("../f", "/cwd/a/up").unwrap();
    assert_eq!(get("../up"), b"x");
    assert_eq!(chdir(".."), Ok(()));
    assert_eq!(cwd().unwrap(), "/cwd/a");
    assert_eq!(chdir("in/"), Ok(()));
    assert_eq!(mkdir("sub"), Ok(()));
    assert_eq!(list("."), [("rel".to_string(), NODE_FILE), ("sub".to_string(), NODE_DIRECTORY)]);
    assert_eq!(rmdir("sub"), Ok(()));

    // Renamed, here through a directory above it, it is reported from where it is now.
    assert_eq!(rename("/cwd/a", "/cwd/b"), Ok(()));
    assert_eq!(cwd().unwrap(), "/cwd/b/in");
    assert_eq!(get("rel"), b"r");
    // Removed, it leaves relative paths nowhere to start until another one is set.
    assert_eq!(remove("rel"), Ok(()));
    assert_eq!(rmdir("/cwd/b/in"), Ok(()));
    assert_eq!(cwd(), Err(Error::NotFound));
    for path in ["rel", ".", "..", "../in"] { assert_eq!(stat(path), Err(Error::NotFound), "{path}"); }
    assert_eq!(open("x", WRITE | CREATE), Err(Error::NotFound));
    assert_eq!(mkdir("y"), Err(Error::NotFound));
    assert_eq!(real_path("."), Err(Error::NotFound));
    assert_eq!(chdir(".."), Err(Error::NotFound));
    assert_eq!(names("/cwd/b"), ["up"]);
    assert_eq!(get("/cwd/b/up"), b"x");
    assert_eq!(chdir("/cwd/b/../b"), Ok(()));
    assert_eq!(cwd().unwrap(), "/cwd/b");
    assert_eq!(chdir("/"), Ok(()));
    assert_eq!(cwd().unwrap(), "/");
}

static YIELDS: AtomicU64 = AtomicU64::new(0);

#[test]
fn a_waiting_lock_is_granted_after_the_unlock() {
    let _world = alone();
    set_yield(|| { YIELDS.fetch_add(1, Ordering::SeqCst); thread::yield_now(); });
    mkdir("/wait").unwrap();
    put("/wait/f", b"x");
    let holder = open("/wait/f", READ).unwrap();
    assert_eq!(lock(holder, LOCK_EXCLUSIVE), Ok(()));
    let (events, order) = mpsc::channel();
    let waiter = thread::spawn({
        let events = events.clone();
        move || {
            let file = open("/wait/f", READ).unwrap();
            assert_eq!(unsafe { MemFs::lock(file, LOCK_SHARED, true) }, Ok(()));
            events.send("granted").unwrap();
            close(file);
        }
    });
    // The waiter yields only after a refusal: by then it is waiting for this unlock.
    while YIELDS.load(Ordering::SeqCst) == 0 { thread::yield_now(); }
    events.send("unlock").unwrap();
    assert_eq!(lock(holder, LOCK_UNLOCK), Ok(()));
    waiter.join().unwrap();
    assert_eq!(order.try_iter().collect::<Vec<_>>(), ["unlock", "granted"]);

    // Two shared holders that both wait for the exclusive lock do not wait for each
    // other: each gave its shared lock up before it started to wait.
    assert_eq!(lock(holder, LOCK_SHARED), Ok(()));
    let (ready, shared) = mpsc::channel();
    let rival = thread::spawn(move || {
        let file = open("/wait/f", READ).unwrap();
        assert_eq!(lock(file, LOCK_SHARED), Ok(()));
        ready.send(()).unwrap();
        assert_eq!(unsafe { MemFs::lock(file, LOCK_EXCLUSIVE, true) }, Ok(()));
        close(file);
    });
    shared.recv().unwrap();
    let before = YIELDS.load(Ordering::SeqCst);
    while YIELDS.load(Ordering::SeqCst) == before { thread::yield_now(); }
    assert_eq!(unsafe { MemFs::lock(holder, LOCK_EXCLUSIVE, true) }, Ok(()));
    close(holder);
    rival.join().unwrap();
    set_yield(std::hint::spin_loop);
}

#[test]
fn threads_work_on_their_own_files_in_one_directory() {
    let _world = together();
    mkdir("/threads").unwrap();
    let rounds = if cfg!(miri) { 4 } else { 200usize };
    let workers: Vec<_> = (0..8u8).map(|worker| std::thread::spawn(move || {
        for round in 0..rounds {
            let (path, moved) = (format!("/threads/{worker}-{round}"), format!("/threads/{worker}-{round}.moved"));
            let data = vec![worker; 1 + round % 97];
            let file = open(&path, READ | WRITE | CREATE | EXCLUSIVE).unwrap();
            assert_eq!(write(file, round as u64, &data), Ok(data.len()));
            assert_eq!(read(file, round as u64, 128).unwrap(), data);
            assert_eq!(rename(&path, &moved), Ok(()));
            assert_eq!(status(file).size, (round + data.len()) as u64);
            close(file);
            assert_eq!(&get(&moved)[round..], data);
            let listed = names("/threads");
            assert!(listed.contains(&format!("{worker}-{round}.moved")) && !listed.contains(&format!("{worker}-{round}")));
            assert_eq!(remove(&moved), Ok(()));
            assert_eq!(stat(&moved), Err(Error::NotFound));
        }
    })).collect();
    for worker in workers { worker.join().unwrap(); }
    assert!(list("/threads").is_empty());
    assert_eq!(rmdir("/threads"), Ok(()));
}

// `Files` and `Watches` both have `open`, `close` and `remove`, so the calls of a watcher are spelled out here.
const ALL: u32 = ACCESS | MODIFY | ATTRIBUTES | MOVED_FROM | MOVED_TO | watches::CREATE | DELETE;
/// One event as the tests compare it: the watch, the kinds, the cookie and the name.
type Seen = (u32, u32, u32, String);
fn seen(watch: u32, events: u32, name: &str) -> Seen { (watch, events, 0, name.to_string()) }
fn watcher() -> *mut c_void { <MemFs as port::Watches>::open().unwrap() }
fn close_watcher(watcher: *mut c_void) { assert_eq!(unsafe { <MemFs as port::Watches>::close(watcher) }, Ok(())); }
fn watch(watcher: *mut c_void, path: &str, events: u32) -> Result<u32> { unsafe { <MemFs as port::Watches>::add(watcher, path.as_bytes(), events) } }
fn unwatch(watcher: *mut c_void, id: u32) -> Result<()> { unsafe { <MemFs as port::Watches>::remove(watcher, id) } }
fn await_event(watcher: *mut c_void, timeout_ns: u64) -> Result<Seen> {
    let mut event = Event::EMPTY;
    unsafe { <MemFs as port::Watches>::read(watcher, timeout_ns, &mut event) }?;
    assert!(event.name[event.name_length as usize..].iter().all(|byte| *byte == 0), "the name ends in zeros");
    Ok((event.watch, event.events, event.cookie, String::from_utf8(event.name().to_vec()).unwrap()))
}
/// Everything that is queued now. Nothing is waited for: an event is queued by the call that causes it.
fn queued(watcher: *mut c_void) -> Vec<Seen> {
    let mut all = Vec::new();
    loop {
        match await_event(watcher, 0) {
            Ok(event) => all.push(event),
            Err(end) => { assert_eq!(end, Error::Timeout); return all; }
        }
    }
}
/// The two halves of a rename, which share a cookie that is not zero.
fn moved(from: (u32, &str), to: (u32, &str), mark: u32, halves: &[Seen]) {
    let cookie = halves[0].2;
    assert_ne!(cookie, 0);
    assert_eq!(halves, [(from.0, MOVED_FROM | mark, cookie, from.1.to_string()), (to.0, MOVED_TO | mark, cookie, to.1.to_string())]);
}

#[test]
fn a_watched_directory_tells_what_happens_to_its_entries_in_order() {
    let _world = together();
    mkdir("/seen").unwrap();
    mkdir("/unseen").unwrap();
    let w = watcher();
    let id = watch(w, "/seen", ALL).unwrap();
    assert_ne!(id, 0);
    assert_eq!(queued(w), []);
    let file = open("/seen/f", READ | WRITE | CREATE).unwrap();
    assert_eq!(write(file, 0, b"hello"), Ok(5));
    assert_eq!(read(file, 0, 4).unwrap(), b"hell");
    assert_eq!(chmod("/seen/f", 0o600), Ok(()));
    assert_eq!(rename("/seen/f", "/seen/g"), Ok(()));
    assert_eq!(remove("/seen/g"), Ok(()));
    let all = queued(w);
    assert_eq!(all[..4], [seen(id, watches::CREATE, "f"), seen(id, MODIFY, "f"), seen(id, ACCESS, "f"), seen(id, ATTRIBUTES, "f")]);
    moved((id, "f"), (id, "g"), 0, &all[4..6]);
    assert_eq!(all[6..], [seen(id, DELETE, "g")]);
    // The file has no name left, so nothing tells of it any more.
    assert_eq!(write(file, 0, b"unseen"), Ok(6));
    close(file);
    assert_eq!(queued(w), []);

    // What changes nothing is no event: a write of nothing, a read at the end, the size and the times the file has.
    let file = open("/seen/h", READ | WRITE | CREATE).unwrap();
    assert_eq!(write(file, 0, b"abc"), Ok(3));
    assert_eq!(write(file, 1, b""), Ok(0));
    assert_eq!(read(file, 3, 8).unwrap(), b"");
    assert_eq!(set_size(file, 3), Ok(()));
    assert_eq!(utimes("/seen/h", true, None, None), Ok(()));
    assert_eq!(unsafe { MemFs::set_file_times(file, None, None) }, Ok(()));
    assert_eq!(queued(w), [seen(id, watches::CREATE, "h"), seen(id, MODIFY, "h")]);
    assert_eq!(set_size(file, 1), Ok(()));
    assert_eq!(set_size(file, 9), Ok(()));
    assert_eq!(queued(w), [seen(id, MODIFY, "h"), seen(id, MODIFY, "h")]);
    assert_eq!(unsafe { MemFs::set_file_mode(file, 0o640) }, Ok(()));
    assert_eq!(utimes("/seen/h", true, Some(1), None), Ok(()));
    assert_eq!(unsafe { MemFs::set_file_times(file, None, Some(2)) }, Ok(()));
    assert_eq!(queued(w), [seen(id, ATTRIBUTES, "h"), seen(id, ATTRIBUTES, "h"), seen(id, ATTRIBUTES, "h")]);
    close(open("/seen/h", WRITE | TRUNCATE).unwrap());
    close(open("/seen/h", WRITE | TRUNCATE).unwrap());
    assert_eq!(queued(w), [seen(id, MODIFY, "h")], "truncating an empty file changes nothing");
    close(file);

    // Directories carry their mark, and what happens inside them is theirs to tell.
    assert_eq!(mkdir("/seen/d"), Ok(()));
    assert_eq!(chmod("/seen/d", 0o700), Ok(()));
    put("/seen/d/inner", b"x");
    assert_eq!(remove("/seen/d/inner"), Ok(()));
    assert_eq!(rename("/seen/d", "/seen/e"), Ok(()));
    assert_eq!(rmdir("/seen/e"), Ok(()));
    let all = queued(w);
    assert_eq!(all[..2], [seen(id, watches::CREATE | DIRECTORY, "d"), seen(id, ATTRIBUTES | DIRECTORY, "d")]);
    moved((id, "d"), (id, "e"), DIRECTORY, &all[2..4]);
    assert_eq!(all[4..], [seen(id, DELETE | DIRECTORY, "e")]);

    // A link is an entry like any other; a call that follows it changes what it leads to.
    assert_eq!(symlink("h", "/seen/l"), Ok(()));
    assert_eq!(utimes("/seen/l", false, Some(1), Some(1)), Ok(()));
    assert_eq!(chmod("/seen/l", 0o600), Ok(()));
    assert_eq!(remove("/seen/l"), Ok(()));
    assert_eq!(queued(w), [seen(id, watches::CREATE, "l"), seen(id, ATTRIBUTES, "l"), seen(id, ATTRIBUTES, "h"), seen(id, DELETE, "l")]);

    // A rename into or out of the directory is the half that happens there.
    put("/unseen/x", b"x");
    assert_eq!(rename("/unseen/x", "/seen/x"), Ok(()));
    assert_eq!(rename("/seen/x", "/unseen/y"), Ok(()));
    let all = queued(w);
    assert_eq!(all.iter().map(|(watch, events, _, name)| (*watch, *events, name.as_str())).collect::<Vec<_>>(), [(id, MOVED_TO, "x"), (id, MOVED_FROM, "x")]);
    assert!(all[0].2 != 0 && all[1].2 != 0 && all[0].2 != all[1].2);
    close_watcher(w);
}

#[test]
fn a_watch_hears_only_the_kinds_it_asked_for() {
    let _world = together();
    mkdir("/asked").unwrap();
    let w = watcher();
    let id = watch(w, "/asked", watches::CREATE | DELETE).unwrap();
    put("/asked/g", b"data");
    assert_eq!(chmod("/asked/g", 0o600), Ok(()));
    assert_eq!(rename("/asked/g", "/asked/h"), Ok(()));
    assert_eq!(get("/asked/h"), b"data");
    assert_eq!(remove("/asked/h"), Ok(()));
    assert_eq!(queued(w), [seen(id, watches::CREATE, "g"), seen(id, DELETE, "h")]);
    // Watching the node again, by whatever path, keeps the id and replaces the kinds.
    assert_eq!(symlink("/asked", "/asked-link"), Ok(()));
    assert_eq!(watch(w, "/asked-link/../asked/", MODIFY | MOVED_TO | ONLY_DIRECTORY), Ok(id));
    put("/asked/i", b"data");
    assert_eq!(rename("/asked/i", "/asked/j"), Ok(()));
    assert_eq!(remove("/asked/j"), Ok(()));
    let all = queued(w);
    assert_eq!(all.iter().map(|(watch, events, _, name)| (*watch, *events, name.as_str())).collect::<Vec<_>>(), [(id, MODIFY, "i"), (id, MOVED_TO, "j")]);
    assert_ne!(all[1].2, 0);
    close_watcher(w);
}

#[test]
fn add_names_a_node_and_refuses_what_it_cannot_watch() {
    let _world = together();
    mkdir("/adds").unwrap();
    put("/adds/f", b"x");
    symlink("f", "/adds/to-file").unwrap();
    symlink("/adds", "/adds/to-dir").unwrap();
    symlink("nowhere", "/adds/dangling").unwrap();
    let w = watcher();
    assert_eq!(watch(w, "/adds/missing", ALL), Err(Error::NotFound));
    assert_eq!(watch(w, "/adds/dangling", ALL), Err(Error::NotFound));
    assert_eq!(watch(w, "/adds/f/x", ALL), Err(Error::NotDirectory));
    assert_eq!(watch(w, &format!("/adds/{}", "n".repeat(256)), ALL), Err(Error::NameTooLong));
    assert_eq!(watch(w, "/adds/f", ALL | ONLY_DIRECTORY), Err(Error::NotDirectory));
    assert_eq!(watch(w, "/adds/to-file", ALL | ONLY_DIRECTORY), Err(Error::NotDirectory));
    assert_eq!(watch(w, "/adds/to-dir", ALL | ONLY_DIRECTORY | NO_FOLLOW), Err(Error::NotDirectory));
    for nothing in [0, ONLY_DIRECTORY, NO_FOLLOW, OVERFLOW | REMOVED | DIRECTORY] { assert_eq!(watch(w, "/adds", nothing), Err(Error::InvalidArgument)); }
    // A refusal uses no id. Ids count from 1, a node keeps the one it has, and a link that is followed is its target.
    let directory = watch(w, "/adds", ALL | ONLY_DIRECTORY).unwrap();
    assert_eq!(watch(w, "/adds/to-dir", ALL | ONLY_DIRECTORY), Ok(directory));
    let file = watch(w, "/adds/f", ALL).unwrap();
    assert_eq!(watch(w, "/adds/to-file", ALL), Ok(file));
    let link = watch(w, "/adds/to-file", ALL | NO_FOLLOW).unwrap();
    let dangling = watch(w, "/adds/dangling", ALL | NO_FOLLOW).unwrap();
    assert_eq!([directory, file, link, dangling], [1, 2, 3, 4]);
    let other = watcher();
    assert_eq!(watch(other, "/adds/f", MODIFY), Ok(1), "ids are the watcher's own");
    // The link and its target are two nodes.
    assert_eq!(utimes("/adds/to-file", false, Some(5), Some(6)), Ok(()));
    assert_eq!(chmod("/adds/to-file", 0o600), Ok(()));
    assert_eq!(queued(w), [seen(directory, ATTRIBUTES, "to-file"), seen(link, ATTRIBUTES, ""), seen(directory, ATTRIBUTES, "f"), seen(file, ATTRIBUTES, "")]);
    assert_eq!(queued(other), []);

    // A handle of another kind is refused by every call and stays what it is.
    let handle = open("/adds/f", READ).unwrap();
    let listing = opendir("/adds").unwrap();
    for wrong in [handle, listing, ptr::null_mut()] {
        assert_eq!(watch(wrong, "/adds", ALL), Err(Error::InvalidArgument));
        assert_eq!(unwatch(wrong, 1), Err(Error::InvalidArgument));
        assert_eq!(await_event(wrong, 0), Err(Error::InvalidArgument));
        assert_eq!(unsafe { <MemFs as port::Watches>::close(wrong) }, Err(Error::InvalidArgument));
    }
    assert_eq!(unsafe { <MemFs as Files>::close(w) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::close_directory(w) }, Err(Error::InvalidArgument));
    assert_eq!(unsafe { MemFs::status(w) }.err(), Some(Error::InvalidArgument));
    assert_eq!(map(w, 0, PAGE, MAP_READ, true), Err(Error::InvalidArgument));
    assert_eq!(read(handle, 0, 8).unwrap(), b"x");
    assert_eq!(queued(w), [seen(directory, ACCESS, "f"), seen(file, ACCESS, "")]);
    close(handle);
    closedir(listing);
    close_watcher(other);
    close_watcher(w);
}

#[test]
fn a_rename_is_two_events_with_one_cookie() {
    let _world = together();
    mkdir("/ra").unwrap();
    mkdir("/rb").unwrap();
    put("/ra/f", b"x");
    let (w, only_b) = (watcher(), watcher());
    let (a, b) = (watch(w, "/ra", ALL).unwrap(), watch(w, "/rb", ALL).unwrap());
    let theirs = watch(only_b, "/rb", ALL).unwrap();
    assert_eq!(rename("/ra/f", "/rb/g"), Ok(()));
    let first = queued(w);
    moved((a, "f"), (b, "g"), 0, &first);
    assert_eq!(queued(only_b), [(theirs, MOVED_TO, first[0].2, "g".to_string())], "every watcher hears the same cookie");
    assert_eq!(rename("/rb/g", "/rb/h"), Ok(()));
    let second = queued(w);
    moved((b, "g"), (b, "h"), 0, &second);
    assert_ne!(first[0].2, second[0].2, "every rename has a cookie of its own");
    assert_eq!(queued(only_b).len(), 2);
    // A rename that does nothing tells nothing.
    assert_eq!(rename("/rb/h", "/rb/h"), Ok(()));
    assert_eq!(rename("/rb/missing", "/rb/h"), Err(Error::NotFound));
    assert_eq!(queued(w), []);

    // As with inotify, a destination that is replaced is no DELETE: its own watch hears of its link count and ends.
    put("/ra/new", b"new");
    let replaced = watch(w, "/rb/h", ALL).unwrap();
    assert_eq!(queued(w), [seen(a, watches::CREATE, "new"), seen(a, MODIFY, "new")]);
    assert_eq!(rename("/ra/new", "/rb/h"), Ok(()));
    let all = queued(w);
    moved((a, "new"), (b, "h"), 0, &all[..2]);
    assert_eq!(all[2..], [seen(replaced, ATTRIBUTES, ""), (replaced, REMOVED, 0, String::new())]);
    assert_eq!(get("/rb/h"), b"new");
    // A replaced file that has another name stays watched, and tells of its link count under that name too.
    put("/ra/kept", b"kept");
    link("/ra/kept", "/ra/too").unwrap();
    put("/ra/over", b"over");
    let kept = watch(w, "/ra/kept", ALL).unwrap();
    queued(w);
    assert_eq!(rename("/ra/over", "/ra/kept"), Ok(()));
    let all = queued(w);
    moved((a, "over"), (a, "kept"), 0, &all[..2]);
    assert_eq!(all[2..], [seen(a, ATTRIBUTES, "too"), seen(kept, ATTRIBUTES, "")]);
    assert_eq!(get("/ra/too"), b"kept");

    // An empty directory that is replaced ends the same way.
    mkdir("/ra/d").unwrap();
    mkdir("/rb/e").unwrap();
    let gone = watch(w, "/rb/e", ALL).unwrap();
    queued(w);
    assert_eq!(rename("/ra/d", "/rb/e"), Ok(()));
    let all = queued(w);
    moved((a, "d"), (b, "e"), DIRECTORY, &all[..2]);
    assert_eq!(all[2..], [seen(gone, ATTRIBUTES | DIRECTORY, ""), (gone, REMOVED, 0, String::new())]);
    // A rename that is refused tells nothing.
    put("/rb/e/inside", b"x");
    mkdir("/ra/d2").unwrap();
    queued(w);
    assert_eq!(rename("/ra/d2", "/rb/e"), Err(Error::NotEmpty));
    assert_eq!(queued(w), []);
    close_watcher(only_b);
    close_watcher(w);
}

#[test]
fn a_file_with_several_names_reports_under_each() {
    let _world = together();
    for directory in ["/names1", "/names2", "/names3"] { mkdir(directory).unwrap(); }
    let w = watcher();
    let (first, second) = (watch(w, "/names1", ALL).unwrap(), watch(w, "/names2", ALL).unwrap());
    let file = open("/names1/a", READ | WRITE | CREATE).unwrap();
    assert_eq!(queued(w), [seen(first, watches::CREATE, "a")]);
    // A new name is a change of the link count, told under the names the file had, and then a CREATE.
    assert_eq!(link("/names1/a", "/names1/b"), Ok(()));
    assert_eq!(queued(w), [seen(first, ATTRIBUTES, "a"), seen(first, watches::CREATE, "b")]);
    assert_eq!(link("/names1/b", "/names2/c"), Ok(()));
    assert_eq!(queued(w), [seen(first, ATTRIBUTES, "a"), seen(first, ATTRIBUTES, "b"), seen(second, watches::CREATE, "c")]);
    assert_eq!(link("/names2/c", "/names3/d"), Ok(()));
    let under_each = |kinds: u32| [seen(first, kinds, "a"), seen(first, kinds, "b"), seen(second, kinds, "c")];
    assert_eq!(queued(w), under_each(ATTRIBUTES));
    assert_eq!(write(file, 0, b"data"), Ok(4));
    assert_eq!(queued(w), under_each(MODIFY));
    assert_eq!(read(file, 0, 4).unwrap(), b"data");
    assert_eq!(queued(w), under_each(ACCESS));
    assert_eq!(chmod("/names3/d", 0o600), Ok(()));
    assert_eq!(queued(w), under_each(ATTRIBUTES));
    // Two names of one file: the rename does nothing and tells nothing.
    assert_eq!(rename("/names1/b", "/names2/c"), Ok(()));
    assert_eq!(queued(w), []);
    // A name that goes is a change of the link count under the names that stay, and then a DELETE.
    assert_eq!(remove("/names1/a"), Ok(()));
    assert_eq!(queued(w), [seen(first, ATTRIBUTES, "b"), seen(second, ATTRIBUTES, "c"), seen(first, DELETE, "a")]);
    assert_eq!(remove("/names3/d"), Ok(()));
    assert_eq!(queued(w), [seen(first, ATTRIBUTES, "b"), seen(second, ATTRIBUTES, "c")]);
    assert_eq!(remove("/names1/b"), Ok(()));
    assert_eq!(queued(w), [seen(second, ATTRIBUTES, "c"), seen(first, DELETE, "b")]);
    assert_eq!(remove("/names2/c"), Ok(()));
    assert_eq!(queued(w), [seen(second, DELETE, "c")]);
    assert_eq!(write(file, 0, b"gone"), Ok(4));
    assert_eq!(queued(w), []);
    close(file);
    close_watcher(w);
}

#[test]
fn a_watched_node_reports_without_a_name_until_it_is_gone() {
    let _world = together();
    mkdir("/own").unwrap();
    put("/own/f", b"data");
    let w = watcher();
    // The file is watched first: the order of the events is not the order of the ids.
    let (own, directory) = (watch(w, "/own/f", ALL).unwrap(), watch(w, "/own", ALL).unwrap());
    let file = open("/own/f", READ | WRITE).unwrap();
    assert_eq!(write(file, 4, b"more"), Ok(4));
    assert_eq!(read(file, 0, 2).unwrap(), b"da");
    assert_eq!(chmod("/own/f", 0o600), Ok(()));
    assert_eq!(queued(w), [seen(directory, MODIFY, "f"), seen(own, MODIFY, ""), seen(directory, ACCESS, "f"), seen(own, ACCESS, ""),
        seen(directory, ATTRIBUTES, "f"), seen(own, ATTRIBUTES, "")]);
    assert_eq!(link("/own/f", "/own/g"), Ok(()));
    assert_eq!(queued(w), [seen(directory, ATTRIBUTES, "f"), seen(own, ATTRIBUTES, ""), seen(directory, watches::CREATE, "g")]);
    // The watch is of the node, not of the name it was made with.
    assert_eq!(remove("/own/f"), Ok(()));
    assert_eq!(queued(w), [seen(directory, ATTRIBUTES, "g"), seen(own, ATTRIBUTES, ""), seen(directory, DELETE, "f")]);
    assert_eq!(rename("/own/g", "/own/h"), Ok(()));
    assert_eq!(queued(w).len(), 2);
    // With the last name the watch ends, handles or not, and in the order of inotify its end comes before the DELETE.
    assert_eq!(remove("/own/h"), Ok(()));
    assert_eq!(queued(w), [seen(own, ATTRIBUTES, ""), (own, REMOVED, 0, String::new()), seen(directory, DELETE, "h")]);
    assert_eq!(write(file, 0, b"late"), Ok(4));
    assert_eq!(unsafe { MemFs::set_file_mode(file, 0o644) }, Ok(()));
    assert_eq!(queued(w), []);
    assert_eq!(unwatch(w, own), Err(Error::InvalidArgument), "the watch has ended");
    close(file);

    // A directory tells of its entries and of itself, and ends when it is removed, without a word on its link count.
    mkdir("/own/d").unwrap();
    let inner = watch(w, "/own/d", ALL).unwrap();
    assert_eq!(inner, directory + 1);
    assert_eq!(chmod("/own/d", 0o700), Ok(()));
    put("/own/d/x", b"x");
    assert_eq!(remove("/own/d/x"), Ok(()));
    assert_eq!(rmdir("/own/d"), Ok(()));
    assert_eq!(queued(w), [seen(directory, watches::CREATE | DIRECTORY, "d"), seen(directory, ATTRIBUTES | DIRECTORY, "d"), seen(inner, ATTRIBUTES | DIRECTORY, ""),
        seen(inner, watches::CREATE, "x"), seen(inner, MODIFY, "x"), seen(inner, DELETE, "x"), (inner, REMOVED, 0, String::new()), seen(directory, DELETE | DIRECTORY, "d")]);
    close_watcher(w);
}

#[test]
fn remove_ends_a_watch_and_says_so() {
    let _world = together();
    mkdir("/ends").unwrap();
    let w = watcher();
    let id = watch(w, "/ends", watches::CREATE).unwrap();
    put("/ends/f", b"x");
    assert_eq!(unwatch(w, id), Ok(()));
    put("/ends/g", b"x");
    // What was queued before the end stays, REMOVED comes whatever the watch asked for, and nothing follows it.
    assert_eq!(queued(w), [seen(id, watches::CREATE, "f"), (id, REMOVED, 0, String::new())]);
    for unknown in [id, 0, id + 1, u32::MAX] { assert_eq!(unwatch(w, unknown), Err(Error::InvalidArgument)); }
    assert_eq!(queued(w), []);
    // The node can be watched again, under an id that was not used before.
    assert_eq!(watch(w, "/ends", ALL), Ok(id + 1));
    assert_eq!(remove("/ends/g"), Ok(()));
    assert_eq!(queued(w), [seen(id + 1, DELETE, "g")]);
    // A watcher that closes with watches and events takes them along.
    put("/ends/h", b"x");
    close_watcher(w);
    assert_eq!(remove("/ends/h"), Ok(()));
}

#[test]
fn a_watch_follows_its_directory_through_a_rename() {
    let _world = together();
    for directory in ["/follows", "/follows/d", "/follows/other"] { mkdir(directory).unwrap(); }
    let w = watcher();
    let id = watch(w, "/follows/d", ALL).unwrap();
    assert_eq!(rename("/follows/d", "/follows/other/e"), Ok(()));
    put("/follows/other/e/f", b"x");
    assert_eq!(queued(w), [seen(id, watches::CREATE, "f"), seen(id, MODIFY, "f")]);
    // The path the watch was made with names another node now.
    mkdir("/follows/d").unwrap();
    put("/follows/d/g", b"x");
    assert_eq!(queued(w), []);
    assert_eq!(watch(w, "/follows/other/e", ALL), Ok(id));
    assert_eq!(watch(w, "/follows/d", ALL), Ok(id + 1));
    close_watcher(w);
}

#[test]
fn a_full_queue_drops_what_comes_and_says_so_once() {
    const LIMIT: usize = 1024;
    let _world = together();
    mkdir("/full").unwrap();
    put("/full/f", b"x");
    let w = watcher();
    let id = watch(w, "/full", ATTRIBUTES | DELETE).unwrap();
    for round in 0..LIMIT + 10 { chmod("/full/f", 0o600 | (round as u32 & 7)).unwrap(); }
    // The report of the loss takes a place in the queue, so two events have to go before there is room again.
    assert_eq!(await_event(w, 0), Ok(seen(id, ATTRIBUTES, "f")));
    assert_eq!(chmod("/full/f", 0o644), Ok(()));
    assert_eq!(await_event(w, 0), Ok(seen(id, ATTRIBUTES, "f")));
    assert_eq!(remove("/full/f"), Ok(()));
    let rest = queued(w);
    assert_eq!(rest.len(), LIMIT);
    assert!(rest[..LIMIT - 2].iter().all(|event| *event == seen(id, ATTRIBUTES, "f")));
    assert_eq!(rest[LIMIT - 2..], [(0, OVERFLOW, 0, String::new()), seen(id, DELETE, "f")]);
    // The next loss is told again. REMOVED is an event like the others and can be the one that is lost.
    put("/full/f", b"x");
    for round in 0..LIMIT { chmod("/full/f", 0o600 | (round as u32 & 7)).unwrap(); }
    assert_eq!(unwatch(w, id), Ok(()));
    let rest = queued(w);
    assert_eq!(rest.len(), LIMIT + 1);
    assert_eq!(rest[LIMIT], (0, OVERFLOW, 0, String::new()));
    assert_eq!(unwatch(w, id), Err(Error::InvalidArgument));
    // A name that an event cannot carry is an event the consumer did not get. The table lets no such name in.
    let id = watch(w, "/full", watches::CREATE | DELETE).unwrap();
    put("/full/nul\0name", b"x");
    assert_eq!(remove("/full/f"), Ok(()));
    assert_eq!(queued(w), [(0, OVERFLOW, 0, String::new()), seen(id, DELETE, "f")]);
    close_watcher(w);
}

thread_local! {
    /// What the reads of this thread asked the wait function for.
    static ASKED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// Counts the calls of the wait function on this thread for the test that started it.
    static WAITING: RefCell<Option<Arc<AtomicU64>>> = const { RefCell::new(None) };
}
fn wait(ns: u64) {
    ASKED.with(|asked| asked.borrow_mut().push(ns));
    WAITING.with(|waiting| if let Some(count) = &*waiting.borrow() { count.fetch_add(1, Ordering::SeqCst); });
    thread::yield_now();
}
struct Sent(*mut c_void);
// SAFETY: a watcher is read by one thread while others add and remove, which is what the tests below do.
unsafe impl Send for Sent {}
impl Sent { fn handle(&self) -> *mut c_void { self.0 } }
/// Reads on a thread of its own, which hands back the event and what it asked the wait function for. Returns once
/// that thread waits: whatever the caller does next happens beside a read that found nothing.
fn reader(watcher: *mut c_void, timeout_ns: u64) -> thread::JoinHandle<(Result<Seen>, Vec<u64>)> {
    set_wait(wait);
    let (sent, waits) = (Sent(watcher), Arc::new(AtomicU64::new(0)));
    let reader = thread::spawn({
        let waits = waits.clone();
        move || {
            WAITING.with(|waiting| *waiting.borrow_mut() = Some(waits));
            (await_event(sent.handle(), timeout_ns), ASKED.with(|asked| asked.take()))
        }
    });
    while waits.load(Ordering::SeqCst) == 0 { thread::yield_now(); }
    reader
}

#[test]
fn a_read_waits_for_the_event_another_thread_causes() {
    let _world = together();
    mkdir("/waits").unwrap();
    let w = watcher();
    let id = watch(w, "/waits", ALL).unwrap();
    let waiting = reader(w, FOREVER);
    put("/waits/f", b"x");
    let (event, asked) = waiting.join().unwrap();
    assert_eq!(event, Ok(seen(id, watches::CREATE, "f")));
    assert!(!asked.is_empty() && asked.iter().all(|ns| *ns == 10_000_000), "{asked:?}");
    assert_eq!(queued(w), [seen(id, MODIFY, "f")]);
    // A watch added beside a read that waits reports to it.
    let waiting = reader(w, 3_600_000_000_000);
    let file = watch(w, "/waits/f", ATTRIBUTES).unwrap();
    assert_eq!(unwatch(w, id), Ok(()));
    let (event, asked) = waiting.join().unwrap();
    assert_eq!(event, Ok((id, REMOVED, 0, String::new())));
    assert!(!asked.is_empty() && asked.iter().all(|ns| *ns == 10_000_000), "{asked:?}");
    let waiting = reader(w, 3_600_000_000_000);
    assert_eq!(chmod("/waits/f", 0o600), Ok(()));
    assert_eq!(waiting.join().unwrap().0, Ok(seen(file, ATTRIBUTES, "")));
    close_watcher(w);
}

#[test]
fn a_timed_read_ends_when_it_has_asked_the_wait_function_for_its_time() {
    let _world = together();
    set_wait(wait);
    mkdir("/timed").unwrap();
    let w = watcher();
    let id = watch(w, "/timed", watches::CREATE).unwrap();
    let expire = |timeout_ns: u64| {
        ASKED.with(|asked| asked.borrow_mut().clear());
        (await_event(w, timeout_ns), ASKED.with(|asked| asked.take()))
    };
    assert_eq!(expire(0), (Err(Error::Timeout), vec![]));
    assert_eq!(expire(1), (Err(Error::Timeout), vec![1]));
    assert_eq!(expire(10_000_000), (Err(Error::Timeout), vec![10_000_000]));
    assert_eq!(expire(25_000_000), (Err(Error::Timeout), vec![10_000_000, 10_000_000, 5_000_000]));
    // An event that is there is taken without a wait, whatever the timeout.
    put("/timed/f", b"x");
    assert_eq!(expire(25_000_000), (Ok(seen(id, watches::CREATE, "f")), vec![]));
    close_watcher(w);
}

#[test]
fn remove_releases_a_reader_that_waits_forever() {
    let _world = together();
    mkdir("/released").unwrap();
    let w = watcher();
    let id = watch(w, "/released", ALL).unwrap();
    let waiting = reader(w, FOREVER);
    assert_eq!(unwatch(w, id), Ok(()));
    let (event, asked) = waiting.join().unwrap();
    assert_eq!(event, Ok((id, REMOVED, 0, String::new())));
    assert!(!asked.is_empty());
    close_watcher(w);
}

const PAGE: usize = 4096;
const MAP_READ: u32 = dotnet_pal_rs::runtime::READ;
const MAP_WRITE: u32 = dotnet_pal_rs::runtime::WRITE;
fn map(file: *mut c_void, offset: u64, length: usize, access: u32, shared: bool) -> Result<*mut u8> { unsafe { MemFs::map(file, offset, length, access, shared) } }
fn unmap(address: *mut u8, length: usize) -> Result<()> { unsafe { MemFs::unmap(address, length) } }
fn sync(address: *mut u8, length: usize) -> Result<()> { unsafe { MemFs::sync(address, length) } }
/// Mapped bytes are read and written the way a consumer does it: through the pointer, never through a reference.
fn peek(address: *mut u8, offset: usize, length: usize) -> Vec<u8> {
    let mut out = vec![0xee; length];
    unsafe { ptr::copy_nonoverlapping(address.add(offset), out.as_mut_ptr(), length) };
    out
}
fn poke(address: *mut u8, offset: usize, data: &[u8]) { unsafe { ptr::copy_nonoverlapping(data.as_ptr(), address.add(offset), data.len()) } }
fn pattern(length: usize) -> Vec<u8> { (0..length).map(|index| (index % 251) as u8 + 1).collect() }

#[test]
fn a_shared_mapping_is_the_bytes_of_the_file() {
    let _world = together();
    mkdir("/shared").unwrap();
    let bytes = pattern(5000);
    put("/shared/f", &bytes);
    let file = open("/shared/f", READ | WRITE).unwrap();
    let whole = map(file, 0, 2 * PAGE, MAP_READ | MAP_WRITE, true).unwrap();
    let second = map(file, PAGE as u64, PAGE, MAP_READ, true).unwrap();
    assert!(whole.addr().is_multiple_of(PAGE) && second.addr().is_multiple_of(PAGE));
    assert_eq!(peek(whole, 0, 5000), bytes);
    assert_eq!(peek(second, 0, 5000 - PAGE), bytes[PAGE..]);
    // A write to the file is in both mappings at once.
    assert_eq!(write(file, 4090, b"0123456789"), Ok(10));
    assert_eq!((peek(whole, 4090, 10), peek(second, 0, 4)), (b"0123456789".to_vec(), b"6789".to_vec()));
    // A write through one mapping is in the file and in the other mapping at once.
    poke(second, 2, b"XY");
    assert_eq!(read(file, 4094, 6).unwrap(), b"4567XY");
    assert_eq!(peek(whole, PAGE + 2, 2), b"XY");
    poke(whole, 0, b"first");
    assert_eq!(get("/shared/f")[..5], *b"first");
    put("/shared/other", b"unrelated");
    let late = open("/shared/f", WRITE).unwrap();
    assert_eq!(write(late, 1, b"IRST"), Ok(4));
    close(late);
    assert_eq!(peek(whole, 0, 6), [b"fIRST", &bytes[5..6]].concat());
    assert_eq!(sync(whole, 2 * PAGE), Ok(()));
    assert_eq!(sync(second, PAGE), Ok(()));
    // The mapping outlives the handle it was made with.
    close(file);
    poke(whole, 5, b"!");
    assert_eq!(get("/shared/f")[..6], *b"fIRST!");
    assert_eq!(unmap(second, PAGE), Ok(()));
    assert_eq!(peek(whole, PAGE + 2, 2), b"XY");
    assert_eq!(unmap(whole, 2 * PAGE), Ok(()));
    assert_eq!((get("/shared/f").len(), &get("/shared/f")[4096..4100]), (5000, &b"67XY"[..]));
}

#[test]
fn a_private_mapping_is_a_copy_nobody_else_sees() {
    let _world = together();
    mkdir("/private").unwrap();
    let bytes = pattern(5000);
    put("/private/f", &bytes);
    // A handle that cannot write is enough: what is written stays in the copy.
    let file = open("/private/f", READ).unwrap();
    let copy = map(file, PAGE as u64, PAGE, MAP_READ | MAP_WRITE, false).unwrap();
    close(file);
    assert!(copy.addr().is_multiple_of(PAGE));
    assert_eq!(peek(copy, 0, 5000 - PAGE), bytes[PAGE..]);
    assert_eq!(peek(copy, 5000 - PAGE, 2 * PAGE - 5000), vec![0; 2 * PAGE - 5000]);
    poke(copy, 0, b"mine");
    assert_eq!(get("/private/f"), bytes);
    let writer = open("/private/f", READ | WRITE).unwrap();
    assert_eq!(write(writer, PAGE as u64 + 2, b"theirs"), Ok(6));
    assert_eq!(peek(copy, 0, 8), [b"mine", &bytes[PAGE + 4..PAGE + 8]].concat());
    // Beside a shared mapping of the same page, and beside another copy.
    let shared = map(writer, PAGE as u64, PAGE, MAP_READ | MAP_WRITE, true).unwrap();
    let other = map(writer, PAGE as u64, 10, MAP_READ | MAP_WRITE, false).unwrap();
    assert!(copy != shared && copy != other && shared != other);
    poke(shared, 0, b"S");
    poke(copy, 1, b"P");
    poke(other, 2, b"O");
    assert_eq!(peek(shared, 0, 3), [b"S", &bytes[PAGE + 1..PAGE + 2], b"t"].concat());
    assert_eq!((peek(copy, 0, 3), peek(other, 0, 3)), (b"mPn".to_vec(), [&bytes[PAGE..PAGE + 2], b"O"].concat()));
    assert_eq!(sync(copy, PAGE), Ok(()));
    assert_eq!(unmap(other, 10), Ok(()));
    assert_eq!(unmap(copy, PAGE), Ok(()));
    assert_eq!(unmap(shared, PAGE), Ok(()));
    close(writer);
}

#[test]
fn bytes_behind_the_end_read_as_zero_and_what_is_written_there_is_lost() {
    let _world = together();
    mkdir("/zeros").unwrap();
    put("/zeros/f", &[0xab; 5000]);
    let file = open("/zeros/f", READ | WRITE).unwrap();
    let mapped = map(file, 0, 2 * PAGE, MAP_READ | MAP_WRITE, true).unwrap();
    assert_eq!(peek(mapped, 4990, 2 * PAGE - 4990), [vec![0xab; 10], vec![0; 2 * PAGE - 5000]].concat());
    // Not part of the file, and gone when the file grows over it, by a new size or by a write.
    poke(mapped, 6000, b"lost");
    assert_eq!(read(file, 5000, 2000).unwrap(), b"");
    assert_eq!(set_size(file, 7000), Ok(()));
    assert_eq!(read(file, 4999, 2001).unwrap(), [vec![0xab], vec![0; 2000]].concat());
    assert_eq!(peek(mapped, 6000, 4), [0; 4]);
    poke(mapped, 7500, b"lost");
    assert_eq!(write(file, 7600, b"end"), Ok(3));
    assert_eq!(read(file, 7000, 1000).unwrap(), [vec![0; 600], b"end".to_vec()].concat());
    assert_eq!(peek(mapped, 7500, 4), [0; 4]);
    // A file that shrinks stays where it is, and what was cut off is zero again.
    assert_eq!(set_size(file, 100), Ok(()));
    assert_eq!(status(file).size, 100);
    assert_eq!(peek(mapped, 0, 2 * PAGE), [vec![0xab; 100], vec![0; 2 * PAGE - 100]].concat());
    assert_eq!(set_size(file, 200), Ok(()));
    assert_eq!(read(file, 0, 300).unwrap(), [vec![0xab; 100], vec![0; 100]].concat());
    close(open("/zeros/f", WRITE | TRUNCATE).unwrap());
    assert_eq!((status(file).size, peek(mapped, 0, 200)), (0, vec![0; 200]));
    assert_eq!(write(file, 2, b"again"), Ok(5));
    assert_eq!(peek(mapped, 0, 8), b"\0\0again\0");
    assert_eq!(unmap(mapped, 2 * PAGE), Ok(()));
    assert_eq!(get("/zeros/f"), b"\0\0again");
    close(file);
}

#[test]
fn a_mapped_file_grows_to_the_end_of_its_pages_and_freely_after_the_last_unmap() {
    let _world = together();
    mkdir("/pinned").unwrap();
    let bytes = pattern(5000);
    put("/pinned/f", &bytes);
    let file = open("/pinned/f", READ | WRITE).unwrap();
    let first = map(file, 0, 2 * PAGE, MAP_READ | MAP_WRITE, true).unwrap();
    assert_eq!(set_size(file, 2 * PAGE as u64 + 1), Err(Error::NoSpace));
    assert_eq!(write(file, 2 * PAGE as u64, b"x"), Err(Error::NoSpace));
    assert_eq!(write(file, 2 * PAGE as u64 - 2, b"abc"), Err(Error::NoSpace));
    assert_eq!((status(file).size, peek(first, 2 * PAGE - 2, 2)), (5000, vec![0; 2]), "a write that does not fit changes nothing");
    assert_eq!(write(file, 2 * PAGE as u64 - 2, b"ab"), Ok(2));
    assert_eq!(status(file).size, 2 * PAGE as u64);
    let grown = [bytes.clone(), vec![0; 2 * PAGE - 5002], b"ab".to_vec()].concat();
    assert_eq!(peek(first, 0, 2 * PAGE), grown);
    // The bound holds while any shared mapping is left; a private one is none.
    let second = map(file, PAGE as u64, PAGE, MAP_READ, true).unwrap();
    let copy = map(file, 0, PAGE, MAP_READ, false).unwrap();
    assert_eq!(unmap(first, 2 * PAGE), Ok(()));
    assert_eq!(set_size(file, 2 * PAGE as u64 + 1), Err(Error::NoSpace));
    assert_eq!(unmap(second, PAGE), Ok(()));
    assert_eq!(set_size(file, 10000), Ok(()));
    assert_eq!(read(file, 0, 20000).unwrap(), [grown.clone(), vec![0; 10000 - 2 * PAGE]].concat());
    assert_eq!(write(file, 20000, b"far"), Ok(3));
    assert_eq!(unmap(copy, PAGE), Ok(()));
    // Mapped again, it is pinned at the size it has now.
    let third = map(file, 4 * PAGE as u64, PAGE, MAP_READ | MAP_WRITE, true).unwrap();
    assert_eq!(peek(third, 20000 - 4 * PAGE, 4), b"far\0");
    assert_eq!(set_size(file, 5 * PAGE as u64 + 1), Err(Error::NoSpace));
    assert_eq!(unmap(third, PAGE), Ok(()));
    // Unmapped, it changes its size where it is while that wastes little, and moves when it has to.
    assert_eq!(set_size(file, 20100), Ok(()));
    assert_eq!(read(file, 19999, 200).unwrap(), [b"\0far".to_vec(), vec![0; 97]].concat());
    assert_eq!(write(file, 5 * PAGE as u64 - 1, b"over the pages"), Ok(14));
    assert_eq!(read(file, 5 * PAGE as u64 - 2, 100).unwrap(), b"\0over the pages");
    let again = map(file, 0, PAGE, MAP_READ, true).unwrap();
    assert_eq!(unmap(again, PAGE), Ok(()));
    assert_eq!(set_size(file, 100), Ok(()));
    assert_eq!(set_size(file, 5000), Ok(()));
    assert_eq!(get("/pinned/f"), [&bytes[..100], &[0; 4900]].concat());
    let again = map(file, 0, PAGE, MAP_READ, true).unwrap();
    assert_eq!(unmap(again, PAGE), Ok(()));
    close(open("/pinned/f", WRITE | TRUNCATE).unwrap());
    assert_eq!(write(file, 0, b"fresh"), Ok(5));
    assert_eq!(get("/pinned/f"), b"fresh");
    close(file);
}

#[test]
fn a_shared_mapping_keeps_a_removed_file_as_a_handle_does() {
    let _world = alone();
    mkdir("/kept").unwrap();
    let base = used_bytes();
    put("/kept/f", &[7; 5000]);
    let file = open("/kept/f", READ | WRITE).unwrap();
    let mapped = map(file, 0, 2 * PAGE, MAP_READ | MAP_WRITE, true).unwrap();
    let copy = map(file, 0, PAGE, MAP_READ, false).unwrap();
    // The bytes of the file count, not the pages they are in, and not a private copy.
    assert_eq!(used_bytes(), base + 5000);
    assert_eq!(remove("/kept/f"), Ok(()));
    close(file);
    assert_eq!((used_bytes(), stat("/kept/f")), (base + 5000, Err(Error::NotFound)));
    poke(mapped, 0, b"still here");
    assert_eq!(peek(mapped, 0, 12), b"still here\x07\x07");
    assert_eq!(unmap(mapped, 2 * PAGE), Ok(()));
    assert_eq!(used_bytes(), base);
    assert_eq!(peek(copy, 0, 2), [7; 2]);
    assert_eq!(unmap(copy, PAGE), Ok(()));

    // A range that is mapped twice is unmapped twice, and the file is held until then.
    put("/kept/g", &[8; 100]);
    let file = open("/kept/g", READ).unwrap();
    let (one, two) = (map(file, 0, PAGE, MAP_READ, true).unwrap(), map(file, 0, PAGE, MAP_READ, true).unwrap());
    close(file);
    // So does a file that a rename replaced.
    put("/kept/h", b"new");
    assert_eq!(rename("/kept/h", "/kept/g"), Ok(()));
    assert_eq!(used_bytes(), base + 103);
    assert_eq!(unmap(one, PAGE), Ok(()));
    assert_eq!((used_bytes(), peek(two, 0, 2)), (base + 103, vec![8; 2]));
    assert_eq!(unmap(two, PAGE), Ok(()));
    assert_eq!(used_bytes(), base + 3);
    assert_eq!(unmap(two, PAGE), Err(Error::InvalidArgument));

    // The capacity bounds a mapped file as it bounds any other.
    set_capacity(base + 3 + 5000 + 100);
    put("/kept/i", &[9; 5000]);
    let file = open("/kept/i", READ | WRITE).unwrap();
    let mapped = map(file, 0, PAGE, MAP_READ, true).unwrap();
    assert_eq!(set_size(file, 5101), Err(Error::NoSpace));
    assert_eq!(write(file, 5100, b"x"), Err(Error::NoSpace));
    assert_eq!(write(file, 5098, b"xy"), Ok(2));
    assert_eq!(used_bytes(), base + 3 + 5100);
    assert_eq!(set_size(file, 10), Ok(()));
    assert_eq!(used_bytes(), base + 3 + 10);
    assert_eq!(unmap(mapped, PAGE), Ok(()));
    close(file);
    assert_eq!(remove("/kept/i"), Ok(()));
    assert_eq!(remove("/kept/g"), Ok(()));
    assert_eq!(used_bytes(), base);
    set_capacity(64 * 1024 * 1024);
}

#[test]
fn map_checks_the_handle_the_access_and_the_range() {
    let _world = together();
    mkdir("/rules").unwrap();
    put("/rules/f", &pattern(5000));
    put("/rules/empty", b"");
    put("/rules/page", &pattern(PAGE));
    let (reader, writer, both) = (open("/rules/f", READ).unwrap(), open("/rules/f", WRITE).unwrap(), open("/rules/f", READ | WRITE).unwrap());
    // Every mapping reads the file, so it needs a handle that may; a shared one that writes needs one that may write.
    for (access, shared) in [(MAP_READ, true), (MAP_READ, false), (MAP_READ | MAP_WRITE, true), (MAP_WRITE, false)] {
        assert_eq!(map(writer, 0, PAGE, access, shared), Err(Error::AccessDenied));
    }
    assert_eq!(map(reader, 0, PAGE, MAP_READ | MAP_WRITE, true), Err(Error::AccessDenied));
    assert_eq!(map(reader, 0, PAGE, MAP_WRITE, true), Err(Error::AccessDenied));
    let granted = [(reader, MAP_READ, true), (reader, MAP_READ | MAP_WRITE, false), (reader, MAP_WRITE, false), (both, MAP_READ | MAP_WRITE, true), (both, MAP_WRITE, true)];
    for (handle, access, shared) in granted {
        let mapped = map(handle, 0, PAGE, access, shared).unwrap();
        assert_eq!(unmap(mapped, PAGE), Ok(()));
    }
    for access in [EXECUTE, MAP_READ | EXECUTE, MAP_READ | MAP_WRITE | EXECUTE] {
        assert_eq!(map(both, 0, PAGE, access, true), Err(Error::Unsupported));
        assert_eq!(map(both, 0, PAGE, access, false), Err(Error::Unsupported));
    }
    let directory = opendir("/rules").unwrap();
    assert_eq!(map(directory, 0, PAGE, MAP_READ, true), Err(Error::InvalidArgument));
    assert_eq!(map(ptr::null_mut(), 0, PAGE, MAP_READ, true), Err(Error::InvalidArgument));
    closedir(directory);
    // The offset is a multiple of the page, and the range ends in the page that holds the last byte at the latest.
    for shared in [true, false] {
        let refused = [(1, 10), (PAGE as u64 - 1, 1), (PAGE as u64 + 100, 10), (0, 0), (0, 2 * PAGE + 1), (PAGE as u64, PAGE + 1), (2 * PAGE as u64, 1), (u64::MAX - 4095, PAGE), (0, usize::MAX)];
        for (offset, length) in refused {
            assert_eq!(map(both, offset, length, MAP_READ, shared), Err(Error::InvalidArgument), "{offset} {length}");
        }
        for (offset, length) in [(0, 1), (0, 5000), (0, 2 * PAGE), (PAGE as u64, 1), (PAGE as u64, PAGE)] {
            let mapped = map(both, offset, length, MAP_READ, shared).unwrap();
            assert_eq!(peek(mapped, 0, 1), pattern(5000)[offset as usize..offset as usize + 1]);
            assert_eq!(unmap(mapped, length), Ok(()));
        }
        let (empty, page) = (open("/rules/empty", READ).unwrap(), open("/rules/page", READ).unwrap());
        assert_eq!(map(empty, 0, 1, MAP_READ, shared), Err(Error::InvalidArgument), "an empty file has no page");
        assert_eq!(map(page, 0, PAGE + 1, MAP_READ, shared), Err(Error::InvalidArgument));
        let mapped = map(page, 0, PAGE, MAP_READ, shared).unwrap();
        assert_eq!(peek(mapped, 0, PAGE), pattern(PAGE));
        assert_eq!(unmap(mapped, PAGE), Ok(()));
        close(empty);
        close(page);
    }
    // unmap and sync take what one map returned: that address with that length, while it is mapped.
    for shared in [true, false] {
        let mapped = map(both, PAGE as u64, 100, MAP_READ, shared).unwrap();
        for call in [unmap, sync] {
            assert_eq!(call(mapped, 99), Err(Error::InvalidArgument));
            assert_eq!(call(mapped, PAGE), Err(Error::InvalidArgument));
            assert_eq!(call(mapped.wrapping_add(1), 99), Err(Error::InvalidArgument));
            assert_eq!(call(mapped.wrapping_sub(PAGE), 100), Err(Error::InvalidArgument));
            assert_eq!(call(ptr::null_mut(), 100), Err(Error::InvalidArgument));
        }
        assert_eq!(sync(mapped, 100), Ok(()));
        assert_eq!(unmap(mapped, 100), Ok(()));
        assert_eq!(sync(mapped, 100), Err(Error::InvalidArgument));
        assert_eq!(unmap(mapped, 100), Err(Error::InvalidArgument));
    }
    for handle in [reader, writer, both] { close(handle); }
}
