//! Exercises set_current_directory of the std port's files group through the
//! negotiated C table. The working directory belongs to the process, so this test
//! has a test binary to itself: beside the tests of table.rs it would move the
//! paths under them.
use dotnet_pal_rs::files::{self, Status, NODE_FILE, READ};
use dotnet_pal_rs::io::NOT_DIRECTORY;
use dotnet_pal_rs::runtime::NOT_FOUND;
use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
use std::{mem::size_of, path::{Path, PathBuf}, ptr};

/// Goes back to where the test started and removes the scratch tree, even when an assertion fails first.
struct Scratch { root: PathBuf, start: PathBuf }
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.start);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn ops() -> &'static files::Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & files::CAP, files::CAP);
    &api.files
}
fn bytes(path: &Path) -> &[u8] { path.to_str().unwrap().as_bytes() }
fn enter(path: &[u8]) -> u32 { unsafe { ops().set_current_directory.unwrap()(path.as_ptr(), path.len()) } }
fn current() -> PathBuf {
    let (mut needed, mut text) = (0usize, vec![0u8; dotnet_pal_rs::runtime::MAX_NAME + 1]);
    assert_eq!(unsafe { ops().current_directory.unwrap()(text.as_mut_ptr(), text.len(), &mut needed) }, OK);
    PathBuf::from(std::str::from_utf8(&text[..needed - 1]).unwrap())
}
fn directory_ok() -> u64 {
    let mut out = files::Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<files::Stats>()) }, OK);
    out.directory_ok
}

#[test]
fn set_current_directory_moves_what_relative_paths_mean() {
    let f = ops();
    let start = std::env::current_dir().unwrap();
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("dotnet-pal-std-cwd-{}-{unique}", std::process::id()));
    let _cleanup = Scratch { root: root.clone(), start: start.clone() };
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("plain.txt"), b"plain").unwrap();
    std::fs::write(root.join("sub").join("inside.txt"), b"inside").unwrap();
    // The OS reports the directory by its resolved name, whatever links the temporary directory sits behind.
    let canonical = std::fs::canonicalize(&root).unwrap();
    let counted = directory_ok();

    assert_eq!(enter(bytes(&root.join("sub"))), OK);
    assert_eq!((std::env::current_dir().unwrap(), current()), (canonical.join("sub"), canonical.join("sub")));
    let mut handle = ptr::null_mut();
    assert_eq!(unsafe { f.open.unwrap()(b"inside.txt".as_ptr(), 10, READ, 0, &mut handle) }, OK, "a relative path starts at the new directory");
    assert_eq!(unsafe { f.close.unwrap()(handle) }, OK);
    let (mut described, relative) = (Status::default(), Path::new("..").join("plain.txt"));
    assert_eq!(unsafe { f.path_status.unwrap()(bytes(&relative).as_ptr(), bytes(&relative).len(), 1, &mut described, size_of::<Status>()) }, OK);
    assert_eq!((described.kind, described.size), (NODE_FILE, 5));
    assert_eq!(unsafe { f.path_status.unwrap()(b"plain.txt".as_ptr(), 9, 1, &mut described, size_of::<Status>()) }, NOT_FOUND, "and no longer at the old one");

    assert_eq!((enter(b".."), std::env::current_dir().unwrap(), current()), (OK, canonical.clone(), canonical.clone()));
    assert_eq!((enter(b"missing"), enter(b"plain.txt"), std::env::current_dir().unwrap()), (NOT_FOUND, NOT_DIRECTORY, canonical.clone()), "a refused move moves nothing");
    assert_eq!(unsafe { f.set_current_directory.unwrap()(ptr::null(), 3) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { f.set_current_directory.unwrap()(b"sub".as_ptr(), 0) }, INVALID_ARGUMENT, "empty path");
    assert_eq!((enter(b"sub\0"), std::env::current_dir().unwrap()), (INVALID_ARGUMENT, canonical));

    assert_eq!((enter(bytes(&start)), std::env::current_dir().unwrap()), (OK, start));
    assert!(directory_ok() >= counted + 5, "the move counts as a directory operation");
}
