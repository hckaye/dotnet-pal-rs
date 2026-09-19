//! Linux flock/OFD locks belong to the open file description, not just the
//! parent's handle. Reproduce the pre-exec inheritance behind table.rs's race
//! deterministically, keeping the child alive with a pipe rather than sleeps.
#![cfg(target_os = "linux")]
use dotnet_pal_rs::{files, port::{Error, Files}};
use dotnet_pal_std::Std;
use std::{ffi::c_void, os::fd::{FromRawFd, OwnedFd}, ptr};

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) { let _ = unsafe { <Std as Files>::close(self.0) }; }
}
struct Child { pid: libc::pid_t, release: Option<OwnedFd> }
impl Child {
    fn finish(&mut self) -> i32 {
        // EOF lets the child close its copy of every inherited description.
        drop(self.release.take());
        let mut status = 0;
        loop {
            let result = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if result == self.pid { self.pid = 0; return status; }
            assert!(result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR));
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        drop(self.release.take());
        if self.pid != 0 {
            // Also reap on an assertion failure. No child is abandoned.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
            while unsafe { libc::waitpid(self.pid, ptr::null_mut(), 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {}
        }
    }
}

#[test]
fn close_releases_flock_and_ofd_locks_after_the_last_inherited_description() {
    let path = std::env::temp_dir().join(format!("pal-lock-inheritance-{}", std::process::id()));
    std::fs::write(&path, b"lock").unwrap();
    use std::os::unix::ffi::OsStrExt;
    for range in [false, true] {
        let open = || Handle(unsafe { <Std as Files>::open(path.as_os_str().as_bytes(), files::READ | files::WRITE, 0) }.unwrap());
        let (holder, contender) = (open(), open());
        let lock = |handle: &Handle| unsafe {
            if range { <Std as Files>::lock_range(handle.0, 0, 1, files::LOCK_EXCLUSIVE) }
            else { <Std as Files>::lock(handle.0, files::LOCK_EXCLUSIVE, false) }
        };
        lock(&holder).unwrap();
        let mut ends = [0; 2];
        assert_eq!(unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            // Only async-signal-safe calls between fork and _exit. No Rust
            // allocation, locks, assertions or destructors run in the child.
            unsafe {
                libc::close(ends[1]);
                libc::alarm(10);
                let mut byte = 0u8;
                libc::read(ends[0], (&mut byte as *mut u8).cast(), 1);
                libc::_exit(0);
            }
        }
        unsafe { libc::close(ends[0]) };
        let release = unsafe { OwnedFd::from_raw_fd(ends[1]) };
        assert!(pid > 0, "fork failed: {}", std::io::Error::last_os_error());
        let mut child = Child { pid, release: Some(release) };
        drop(holder);
        let while_inherited = lock(&contender);
        let status = child.finish();
        let after_last_close = lock(&contender);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        assert_eq!(while_inherited, Err(Error::WouldBlock), "range={range}: the child still owns the lock");
        assert_eq!(after_last_close, Ok(()), "range={range}: the last close releases the lock");
    }
    std::fs::remove_file(path).unwrap();
}
