//! Exercises the volumes group of the std port through the negotiated C table, on
//! whatever desktop OS runs the test, against what the C library lists and
//! measures itself: getmntent and statvfs on Linux, getmntinfo and statfs on macOS.
use dotnet_pal_rs::runtime::MAX_NAME;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use dotnet_pal_rs::volumes::{self, Status};
use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
use std::{mem::size_of, ptr, sync::{Mutex, MutexGuard}};

const CAPACITY: usize = MAX_NAME + 1;
/// One test at a time: the counters are the process's.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
fn ops() -> &'static volumes::Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & volumes::CAP, volumes::CAP);
    &api.volumes
}
/// Status, needed length and the whole buffer of one call; no buffer at all for capacity 0.
fn entry(index: usize, capacity: usize) -> (u32, usize, Vec<u8>) {
    let (mut needed, mut buffer) = (usize::MAX, vec![0xAAu8; capacity]);
    let status = unsafe { ops().entry.unwrap()(index, if capacity == 0 { ptr::null_mut() } else { buffer.as_mut_ptr() }, capacity, &mut needed) };
    (status, needed, buffer)
}
/// Status of a call and what it left in an output that started as garbage.
fn status(path: &[u8]) -> (u32, Status) {
    let mut out = Status { total_bytes: 7, free_bytes: 7, available_bytes: 7, format: [0xAA; 32] };
    (unsafe { ops().status.unwrap()(path.as_ptr(), path.len(), &mut out, size_of::<Status>()) }, out)
}
fn cleared(s: &Status) -> bool { s.total_bytes == 0 && s.free_bytes == 0 && s.available_bytes == 0 && s.format == [0; 32] }
fn stats() -> volumes::Stats {
    let mut out = volumes::Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<volumes::Stats>()) }, OK);
    out
}
fn counted(s: &volumes::Stats) -> (u64, u64, u64) { (s.entry_ok, s.status_ok, s.rejected_or_failed) }

/// The mount points as the C library lists them, with the type text of each.
#[cfg(target_os = "linux")]
fn listed() -> Vec<(Vec<u8>, Vec<u8>)> {
    let table = unsafe { libc::setmntent(c"/proc/self/mounts".as_ptr(), c"r".as_ptr()) };
    assert!(!table.is_null());
    let mut all = Vec::new();
    loop {
        let mount = unsafe { libc::getmntent(table) };
        if mount.is_null() { break; }
        all.push(unsafe { (std::ffi::CStr::from_ptr((*mount).mnt_dir).to_bytes().to_vec(), std::ffi::CStr::from_ptr((*mount).mnt_type).to_bytes().to_vec()) });
    }
    unsafe { libc::endmntent(table) };
    all
}
#[cfg(target_os = "macos")]
fn listed() -> Vec<(Vec<u8>, Vec<u8>)> {
    let text = |field: &[std::ffi::c_char]| field.iter().map(|c| *c as u8).take_while(|byte| *byte != 0).collect::<Vec<u8>>();
    let mut list = ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut list, libc::MNT_NOWAIT) };
    assert!(count > 0);
    unsafe { std::slice::from_raw_parts(list, count as usize) }.iter().map(|mount| (text(&mount.f_mntonname), text(&mount.f_fstypename))).collect()
}
/// Capacity, free and available bytes and, where the call names it, the format, as the C library measures them for a path.
#[cfg(target_os = "linux")]
fn measured(path: &[u8]) -> (u64, u64, u64, Option<Vec<u8>>) {
    let (path, mut space) = (std::ffi::CString::new(path).unwrap(), unsafe { std::mem::zeroed::<libc::statvfs>() });
    assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut space) }, 0);
    let unit = space.f_frsize as u64;
    (space.f_blocks as u64 * unit, space.f_bfree as u64 * unit, space.f_bavail as u64 * unit, None)
}
#[cfg(target_os = "macos")]
fn measured(path: &[u8]) -> (u64, u64, u64, Option<Vec<u8>>) {
    let (path, mut space) = (std::ffi::CString::new(path).unwrap(), unsafe { std::mem::zeroed::<libc::statfs>() });
    assert_eq!(unsafe { libc::statfs(path.as_ptr(), &mut space) }, 0);
    let unit = u64::from(space.f_bsize);
    (space.f_blocks * unit, space.f_bfree * unit, space.f_bavail * unit, Some(space.f_fstypename.iter().map(|c| *c as u8).take_while(|byte| *byte != 0).collect()))
}
/// The type text of the mount that holds a resolved path, found the way df finds it: the longest mount point that leads to the
/// path, the last of them where one covers another. The provider matches devices instead, so the two are independent.
#[cfg(target_os = "linux")]
fn holder(path: &[u8]) -> Vec<u8> {
    let leads = |point: &[u8]| path.starts_with(point) && (point == b"/" || path.len() == point.len() || path[point.len()] == b'/');
    listed().into_iter().filter(|(point, _)| leads(point)).max_by_key(|(point, _)| point.len()).map(|(_, kind)| kind).unwrap()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn mount_points_enumerate_as_the_c_library_lists_them() {
    let _serial = serial();
    // A volume may come or go while the test runs. Only a table that stood still, the same before and after, is compared.
    for attempt in 0.. {
        let (before, earlier) = (stats(), listed());
        let mut points = Vec::new();
        loop {
            let (code, needed, buffer) = entry(points.len(), CAPACITY);
            if code == NOT_FOUND { assert_eq!((needed, buffer), (0, vec![0; CAPACITY])); break; }
            assert_eq!(code, OK);
            // The text, its NUL, and a cleared rest.
            assert!(needed >= 2 && buffer[needed - 1] == 0 && !buffer[..needed - 1].contains(&0) && buffer[needed..].iter().all(|byte| *byte == 0));
            points.push(buffer[..needed - 1].to_vec());
        }
        let later = listed();
        if earlier != later { assert!(attempt < 20, "the mount table never stood still"); continue; }
        assert_eq!(points, earlier.iter().map(|(point, _)| point.clone()).collect::<Vec<_>>());
        assert!(points.iter().any(|point| point == b"/"));
        for point in &points {
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(point[0], b'/');
            assert!(std::fs::symlink_metadata(std::ffi::OsStr::from_bytes(point)).is_ok(), "{}", String::from_utf8_lossy(point));
            measured(point);
        }
        // Each delivers whole where it fits and its length alone, with a cleared buffer, where it does not.
        for (index, point) in points.iter().enumerate() {
            let needed = point.len() + 1;
            let mut whole = point.clone();
            whole.push(0);
            assert_eq!(entry(index, needed), (OK, needed, whole));
            assert_eq!(entry(index, needed - 1), (BUFFER_TOO_SMALL, needed, vec![0; needed - 1]));
            assert_eq!(entry(index, 0), (BUFFER_TOO_SMALL, needed, Vec::new()));
        }
        for index in [points.len() + 1, usize::MAX] { assert_eq!(entry(index, CAPACITY), (NOT_FOUND, 0, vec![0; CAPACITY])); }
        if listed() != later { assert!(attempt < 20, "the mount table never stood still"); continue; }
        let count = points.len() as u64;
        assert_eq!(counted(&stats()), (before.entry_ok + 2 * count, before.status_ok, before.rejected_or_failed + 2 * count + 3));
        break;
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn status_is_the_space_and_the_format_of_the_volume_that_holds_a_path() {
    use std::os::unix::ffi::OsStrExt;
    let _serial = serial();
    let before = stats();
    let scratch = std::env::temp_dir().canonicalize().unwrap();
    let paths: Vec<Vec<u8>> = vec![b"/".to_vec(), scratch.as_os_str().as_bytes().to_vec(), if cfg!(target_os = "linux") { b"/proc".to_vec() } else { b"/dev".to_vec() }];
    let mut asked = 0;
    for path in &paths {
        // Free space moves under a running system: the numbers are compared while two measurements around the call agree.
        for attempt in 0.. {
            let (earlier, (code, s), later) = (measured(path), status(path), measured(path));
            asked += 1;
            assert_eq!(code, OK);
            if earlier != later { assert!(attempt < 50, "the free space of {} never stood still", String::from_utf8_lossy(path)); continue; }
            assert_eq!((s.total_bytes, s.free_bytes, s.available_bytes), (earlier.0, earlier.1, earlier.2), "{}", String::from_utf8_lossy(path));
            assert!(s.available_bytes <= s.free_bytes && s.free_bytes <= s.total_bytes);
            let length = s.format.iter().position(|byte| *byte == 0).unwrap();
            assert!(s.format[length..].iter().all(|byte| *byte == 0));
            #[cfg(target_os = "linux")]
            assert_eq!(s.format[..length], holder(path)[..], "{}", String::from_utf8_lossy(path));
            #[cfg(target_os = "macos")]
            assert_eq!(Some(s.format[..length].to_vec()), earlier.3, "{}", String::from_utf8_lossy(path));
            assert!(length > 0);
            break;
        }
    }
    if cfg!(target_os = "linux") { assert_eq!(status(b"/proc").1.format[..5], *b"proc\0"); asked += 1; }
    // A path that names nothing, directly or through a file, leaves a cleared status.
    let file = scratch.join(format!("pal-std-volumes-{}", std::process::id()));
    std::fs::write(&file, b"x").unwrap();
    let through = [file.as_os_str().as_bytes(), b"/x"].concat();
    for missing in [&b"/no/such/pal/volume"[..], &through] {
        let (code, s) = status(missing);
        assert!(code == NOT_FOUND && cleared(&s));
    }
    // A file is on a volume as much as a directory is.
    let (code, s) = status(file.as_os_str().as_bytes());
    assert!(code == OK && s.total_bytes == measured(file.as_os_str().as_bytes()).0);
    std::fs::remove_file(&file).unwrap();
    assert_eq!(counted(&stats()), (before.entry_ok, before.status_ok + asked + 1, before.rejected_or_failed + 2));
}

#[test]
fn malformed_arguments_are_refused_before_the_provider() {
    let _serial = serial();
    let (v, before) = (ops(), stats());
    let (mut needed, mut buffer) = (7usize, [0xAAu8; 8]);
    assert_eq!(unsafe { v.entry.unwrap()(0, buffer.as_mut_ptr(), 8, ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.entry.unwrap()(0, buffer.as_mut_ptr(), 8, (&raw mut needed).cast::<u8>().wrapping_add(1).cast()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.entry.unwrap()(0, ptr::null_mut(), 8, &mut needed) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.entry.unwrap()(0, buffer.as_mut_ptr(), isize::MAX as usize + 1, &mut needed) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.entry.unwrap()(0, (usize::MAX - 3) as *mut u8, 8, &mut needed) }, INVALID_ARGUMENT);
    assert_eq!((needed, buffer), (7, [0xAA; 8]));
    let mut out = Status { total_bytes: 7, free_bytes: 7, available_bytes: 7, format: [0xAA; 32] };
    assert_eq!(unsafe { v.status.unwrap()(b"/".as_ptr(), 1, ptr::null_mut(), size_of::<Status>()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.status.unwrap()(b"/".as_ptr(), 1, (&raw mut out).cast::<u8>().wrapping_add(1).cast(), size_of::<Status>()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.status.unwrap()(b"/".as_ptr(), 1, &mut out, size_of::<Status>() - 1) }, INVALID_ARGUMENT);
    assert_eq!(out.total_bytes, 7);
    // A path is 1 to MAX_NAME bytes without a NUL; the output is cleared once it is known to be one.
    let long = vec![b'a'; MAX_NAME + 1];
    for (path, length) in [(ptr::null(), 1), (b"/".as_ptr(), 0), (long.as_ptr(), long.len()), (b"/a\0b".as_ptr(), 4), ((usize::MAX - 1) as *const u8, 8)] {
        out = Status { total_bytes: 7, free_bytes: 7, available_bytes: 7, format: [0xAA; 32] };
        assert_eq!(unsafe { v.status.unwrap()(path, length, &mut out, size_of::<Status>()) }, INVALID_ARGUMENT);
        assert!(cleared(&out));
    }
    assert_eq!(unsafe { v.read_stats.unwrap()(ptr::null_mut(), size_of::<volumes::Stats>()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { v.read_stats.unwrap()(&mut volumes::Stats::default(), size_of::<volumes::Stats>() - 1) }, INVALID_ARGUMENT);
    // Targets without a provider behind the table: every well-formed question is UNSUPPORTED and leaves nothing.
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        assert_eq!(entry(0, CAPACITY), (dotnet_pal_rs::UNSUPPORTED, 0, vec![0; CAPACITY]));
        let (code, s) = status(b"/");
        assert!(code == dotnet_pal_rs::UNSUPPORTED && cleared(&s));
    }
    let unsupported = if cfg!(any(target_os = "linux", target_os = "android", target_vendor = "apple")) { 0 } else { 2 };
    assert_eq!(counted(&stats()), (before.entry_ok, before.status_ok, before.rejected_or_failed + 13 + unsupported));
}
