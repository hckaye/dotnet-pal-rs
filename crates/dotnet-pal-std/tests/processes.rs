//! Drives the child process provider of the std port through the negotiated C table
//! with real children (sh, cat, sleep, env), the way the boundary's System.Native
//! does: one thread waits without a limit while others use the pipes and may end
//! the child. Unix only; the tests take turns because they count this process's
//! descriptors and swap its standard streams.
#![cfg(unix)]
use dotnet_pal_rs::io::{ACCESS_DENIED, BROKEN_PIPE, NOT_DIRECTORY};
use dotnet_pal_rs::kernel::{INFINITE, TIMEOUT};
use dotnet_pal_rs::processes::{Ops, Spawned, Stats, PIPE_ERROR, PIPE_INPUT, PIPE_OUTPUT};
use dotnet_pal_rs::runtime::NOT_FOUND;
use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
use std::{ffi::{c_void, CString}, ptr, sync::{Mutex, MutexGuard}, time::{Duration, Instant}};

const MS: u64 = 1_000_000;
static TURN: Mutex<()> = Mutex::new(());
/// One test at a time, with the default action for SIGPIPE: a write to a child that
/// has ended must not depend on the test harness ignoring the signal.
fn turn() -> MutexGuard<'static, ()> {
    let guard = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    guard
}
fn ops() -> &'static Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & dotnet_pal_rs::processes::CAP, dotnet_pal_rs::processes::CAP);
    &api.processes
}
fn stats() -> Stats {
    let mut stats = Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut stats, std::mem::size_of::<Stats>()) }, OK);
    stats
}
/// Open descriptors of this process; the listing's own is in every count.
fn descriptors() -> usize { std::fs::read_dir("/dev/fd").unwrap().count() }
/// Snapshot host-owned inheritable descriptors before the provider makes pipes.
/// The directory used to enumerate them is close-on-exec and is not inherited.
fn inherited_descriptors() -> Vec<i32> {
    let mut descriptors = std::fs::read_dir("/dev/fd").unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().parse::<i32>().unwrap())
        .filter(|fd| {
            let flags = unsafe { libc::fcntl(*fd, libc::F_GETFD) };
            flags >= 0 && flags & libc::FD_CLOEXEC == 0
        }).collect::<Vec<_>>();
    descriptors.sort_unstable();
    descriptors
}

/// Starts a program. What the boundary takes is counted, not terminated: the paths are followed by other bytes.
fn start(program: &str, arguments: &[&str], environment: Option<&[&str]>, directory: Option<&str>, pipes: u32) -> (u32, Spawned) {
    let owned = |list: &[&str]| list.iter().map(|text| CString::new(*text).unwrap()).collect::<Vec<_>>();
    let (arguments, environment) = (owned(arguments), environment.map(owned));
    let pointers = |list: &[CString]| list.iter().map(|text| text.as_ptr().cast::<u8>()).collect::<Vec<_>>();
    let (argv, envp) = (pointers(&arguments), environment.as_deref().map(pointers));
    let unterminated = |text: &str| [text.as_bytes(), b"XXXX"].concat();
    let (path, home) = (unterminated(program), directory.map(unterminated));
    let filled = ptr::dangling_mut::<c_void>();
    let mut spawned = Spawned { process: filled, id: 7, input: filled, output: filled, error: filled };
    let status = unsafe { ops().spawn.unwrap()(path.as_ptr(), program.len(), argv.as_ptr(), argv.len(), envp.as_ref().map_or(ptr::null(), |list| list.as_ptr()),
        envp.as_ref().map_or(0, |list| list.len()), home.as_ref().map_or(ptr::null(), |path| path.as_ptr()), directory.map_or(0, str::len), pipes,
        &mut spawned, std::mem::size_of::<Spawned>()) };
    if status == OK {
        assert!(!spawned.process.is_null() && spawned.id != 0 && unsafe { libc::kill(spawned.id as libc::pid_t, 0) } == 0);
        assert_eq!((spawned.input.is_null(), spawned.output.is_null(), spawned.error.is_null()), (pipes & PIPE_INPUT == 0, pipes & PIPE_OUTPUT == 0, pipes & PIPE_ERROR == 0));
    } else {
        assert!(spawned.process.is_null() && spawned.id == 0 && spawned.input.is_null() && spawned.output.is_null() && spawned.error.is_null());
    }
    (status, spawned)
}
fn started(program: &str, arguments: &[&str], environment: Option<&[&str]>, directory: Option<&str>, pipes: u32) -> Spawned {
    let (status, spawned) = start(program, arguments, environment, directory, pipes);
    assert_eq!(status, OK, "{program} {arguments:?}");
    spawned
}
fn wait(process: *mut c_void, timeout_ns: u64) -> (u32, i32) {
    let mut code = 7;
    (unsafe { ops().wait.unwrap()(process, timeout_ns, &mut code) }, code)
}
fn terminate(process: *mut c_void, forceful: u32) -> u32 { unsafe { ops().terminate.unwrap()(process, forceful) } }
fn release(process: *mut c_void) -> u32 { unsafe { ops().release.unwrap()(process) } }
fn close(pipe: *mut c_void) -> u32 { unsafe { ops().pipe_close.unwrap()(pipe) } }
fn read(pipe: *mut c_void, buffer: &mut [u8]) -> (u32, usize) {
    let mut got = 7;
    (unsafe { ops().pipe_read.unwrap()(pipe, buffer.as_mut_ptr(), buffer.len(), &mut got) }, got)
}
fn write(pipe: *mut c_void, data: &[u8]) -> (u32, usize) {
    let mut written = 7;
    (unsafe { ops().pipe_write.unwrap()(pipe, data.as_ptr(), data.len(), &mut written) }, written)
}
/// Reads a pipe to its end, which is zero bytes with OK, as often as it is asked.
fn drain(pipe: *mut c_void) -> Vec<u8> {
    let (mut all, mut buffer) = (Vec::new(), [0u8; 4096]);
    loop {
        let (status, got) = read(pipe, &mut buffer);
        assert!(status == OK && got <= buffer.len());
        if got == 0 { break; }
        all.extend_from_slice(&buffer[..got]);
    }
    assert_eq!(read(pipe, &mut buffer), (OK, 0));
    all
}
fn feed(pipe: *mut c_void, mut data: &[u8]) {
    while !data.is_empty() {
        let (status, written) = write(pipe, data);
        assert!(status == OK && written > 0 && written <= data.len());
        data = &data[written..];
    }
}
/// Closes the pipes, waits for the end, gives the handle back.
fn finish(child: Spawned) -> i32 {
    for pipe in [child.input, child.output, child.error] { if !pipe.is_null() { assert_eq!(close(pipe), OK); } }
    let (status, code) = wait(child.process, INFINITE);
    assert_eq!((status, release(child.process)), (OK, OK));
    code
}
/// Runs a program to its end and keeps what it wrote to its output.
fn run(program: &str, arguments: &[&str], environment: Option<&[&str]>, directory: Option<&str>) -> (i32, String) {
    let child = started(program, arguments, environment, directory, PIPE_OUTPUT);
    let output = drain(child.output);
    (finish(child), String::from_utf8(output).unwrap())
}
fn shell(script: &str) -> (i32, String) { run("/bin/sh", &["sh", "-c", script], None, None) }
/// A handle other threads may use while this one does.
#[derive(Clone, Copy)]
struct Shared(*mut c_void);
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}
impl Shared { fn get(self) -> *mut c_void { self.0 } }
fn mask() -> libc::sigset_t {
    let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, ptr::null(), &mut mask) }, 0);
    mask
}
fn pending() -> bool {
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::sigpending(&mut set) }, 0);
    unsafe { libc::sigismember(&set, libc::SIGPIPE) == 1 }
}

#[test]
fn exit_codes_arguments_and_identifier() {
    let _turn = turn();
    for code in [0, 7, 255] { assert_eq!(shell(&format!("exit {code}")), (code, String::new())); }
    // Argument 0 is what the program sees as its own name, whatever file was started.
    assert_eq!(run("/bin/sh", &["another name", "-c", "echo \"$0\""], None, None), (0, "another name\n".into()));
    // Arguments arrive as they are: with spaces, empty, unexpanded.
    let script = "for a; do printf '[%s]' \"$a\"; done; echo $#";
    assert_eq!(run("/bin/sh", &["sh", "-c", script, "zero", "two words", "", "tab\there", "*"], None, None), (0, "[two words][][tab\there][*]4\n".into()));
    let child = started("/bin/sh", &["sh", "-c", "echo $$"], None, None, PIPE_OUTPUT);
    assert_eq!(String::from_utf8(drain(child.output)).unwrap().trim().parse::<u64>().unwrap(), child.id);
    assert_eq!(finish(child), 0);
}

#[test]
fn environment_is_replaced_or_inherited_and_the_directory_applied() {
    let _turn = turn();
    // A given environment is the whole environment.
    let given = ["PAL_GIVEN=yes", "PAL_EMPTY=", "PAL_SPACED=a b=c"];
    let (code, text) = run("/usr/bin/env", &["env"], Some(&given), None);
    let mut lines = text.lines().collect::<Vec<_>>();
    lines.sort_unstable();
    assert_eq!((code, lines), (0, vec!["PAL_EMPTY=", "PAL_GIVEN=yes", "PAL_SPACED=a b=c"]));
    assert_eq!(run("/usr/bin/env", &["env"], Some(&[]), None), (0, String::new()));
    // None given is the parent's.
    let path = std::env::var("PATH").expect("the test needs a PATH to find inherited");
    let (code, text) = run("/usr/bin/env", &["env"], None, None);
    assert!(code == 0 && text.lines().any(|line| line == format!("PATH={path}")) && !text.contains("PAL_GIVEN"));
    assert_eq!(start("/usr/bin/env", &["env"], Some(&["no separator"]), None, 0).0, INVALID_ARGUMENT, "std cannot pass on an entry that names no variable");
    let scratch = std::env::temp_dir().canonicalize().unwrap();
    let here = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_ne!(scratch, here);
    assert_eq!(run("/bin/sh", &["sh", "-c", "pwd -P"], None, scratch.to_str()), (0, format!("{}\n", scratch.display())));
    assert_eq!(run("/bin/sh", &["sh", "-c", "pwd -P"], None, None), (0, format!("{}\n", here.display())));
    assert_eq!(std::env::current_dir().unwrap().canonicalize().unwrap(), here, "the parent stayed where it was");
}

#[test]
fn pipes_carry_all_three_streams_and_a_large_round_trip() {
    let _turn = turn();
    // The child reads what the parent writes until the parent closes, and its two outputs stay apart.
    let mut child = started("/bin/sh", &["sh", "-c", "cat; echo problem >&2"], None, None, PIPE_INPUT | PIPE_OUTPUT | PIPE_ERROR);
    feed(child.input, b"to the child\n");
    assert_eq!(close(std::mem::replace(&mut child.input, ptr::null_mut())), OK);
    assert_eq!((drain(child.output), drain(child.error)), (b"to the child\n".to_vec(), b"problem\n".to_vec()));
    assert_eq!(finish(child), 0);
    // A mebibyte through cat: more than any pipe holds, so writer and reader have to run at the same time. A third
    // thread waits for the child's end meanwhile, as the consumer's watcher does.
    let sent = (0..1usize << 20).map(|i| (i * 31 + (i >> 8)) as u8).collect::<Vec<_>>();
    let mut child = started("/bin/cat", &["cat"], None, None, PIPE_INPUT | PIPE_OUTPUT);
    let (input, process) = (Shared(std::mem::replace(&mut child.input, ptr::null_mut())), Shared(child.process));
    let (back, watched) = std::thread::scope(|scope| {
        scope.spawn(|| { feed(input.get(), &sent); assert_eq!(close(input.get()), OK); });
        let watcher = scope.spawn(move || wait(process.get(), INFINITE));
        (drain(child.output), watcher.join().unwrap())
    });
    assert!(back == sent, "{} bytes came back", back.len());
    assert_eq!((watched, finish(child)), ((OK, 0), 0));
}

#[test]
fn a_write_to_a_child_that_ended_is_a_status_not_a_signal() {
    let _turn = turn();
    let child = started("/bin/sh", &["sh", "-c", "exit 3"], None, None, PIPE_INPUT);
    assert_eq!(wait(child.process, INFINITE), (OK, 3));
    // The default action of SIGPIPE would end this process here. Nothing of it is left on the thread either.
    let before = mask();
    assert!(unsafe { libc::sigismember(&before, libc::SIGPIPE) } == 0 && !pending());
    assert_eq!(write(child.input, b"x"), (BROKEN_PIPE, 0));
    assert!(unsafe { libc::sigismember(&mask(), libc::SIGPIPE) } == 0 && !pending());
    // One that was pending behind the caller's own block is the caller's: it is still there afterwards.
    let mut only: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe { libc::sigemptyset(&mut only); libc::sigaddset(&mut only, libc::SIGPIPE) };
    assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &only, ptr::null_mut()) }, 0);
    assert_eq!(unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGPIPE) }, 0);
    assert_eq!(write(child.input, b"x"), (BROKEN_PIPE, 0));
    let mut taken = 0;
    assert!(pending() && unsafe { libc::sigwait(&only, &mut taken) } == 0 && taken == libc::SIGPIPE && !pending());
    assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, ptr::null_mut()) }, 0);
    assert_eq!(finish(child), 3);
}

#[test]
fn a_stream_that_is_not_a_pipe_is_the_parents_own() {
    let _turn = turn();
    // The child reads this process's input and writes to its error output, the stream the test harness leaves alone.
    let (mut supplied, mut captured) = ([0; 2], [0; 2]);
    assert!(unsafe { libc::pipe(supplied.as_mut_ptr()) == 0 && libc::pipe(captured.as_mut_ptr()) == 0 });
    let (saved_in, saved_error) = unsafe { (libc::dup(0), libc::dup(2)) };
    assert!(saved_in >= 0 && saved_error >= 0 && unsafe { libc::dup2(supplied[0], 0) >= 0 && libc::dup2(captured[1], 2) >= 0 });
    assert_eq!(unsafe { libc::write(supplied[1], b"from the parent's input\n".as_ptr().cast(), 24) }, 24);
    unsafe { libc::close(supplied[0]); libc::close(supplied[1]); libc::close(captured[1]) };
    let (status, child) = start("/bin/sh", &["sh", "-c", "cat >&2"], None, None, 0);
    assert!(unsafe { libc::dup2(saved_in, 0) >= 0 && libc::dup2(saved_error, 2) >= 0 });
    unsafe { libc::close(saved_in); libc::close(saved_error) };
    assert_eq!(status, OK);
    assert_eq!(finish(child), 0);
    let mut text = [0u8; 64];
    assert_eq!(unsafe { libc::read(captured[0], text.as_mut_ptr().cast(), text.len()) }, 24);
    assert_eq!(&text[..24], b"from the parent's input\n");
    assert_eq!(unsafe { libc::read(captured[0], text.as_mut_ptr().cast(), text.len()) }, 0);
    unsafe { libc::close(captured[0]) };
}

#[test]
fn waits_run_out_and_a_child_asked_to_end_reports_the_signal() {
    let _turn = turn();
    let child = started("/bin/sleep", &["sleep", "5"], None, None, 0);
    assert_eq!(wait(child.process, 0), (TIMEOUT, 0));
    let begin = Instant::now();
    assert_eq!(wait(child.process, 100 * MS), (TIMEOUT, 0));
    let waited = begin.elapsed();
    assert!(waited >= Duration::from_millis(100) && waited < Duration::from_secs(2), "{waited:?}");
    assert_eq!(unsafe { libc::kill(child.id as libc::pid_t, 0) }, 0);
    // Every later wait says the same at once, and there is nothing left to end.
    assert_eq!(terminate(child.process, 0), OK);
    for limit in [INFINITE, 0, INFINITE, 50 * MS] { assert_eq!(wait(child.process, limit), (OK, 128 + libc::SIGTERM)); }
    assert_eq!((terminate(child.process, 0), terminate(child.process, 1)), (NOT_FOUND, NOT_FOUND));
    assert_eq!(release(child.process), OK);
    // A child that ignores the request runs on until it is ended.
    let child = started("/bin/sh", &["sh", "-c", "trap '' TERM; echo ready; exec sleep 5"], None, None, PIPE_OUTPUT);
    let mut text = [0u8; 16];
    assert_eq!(read(child.output, &mut text), (OK, 6));
    assert_eq!((terminate(child.process, 0), wait(child.process, 200 * MS)), (OK, (TIMEOUT, 0)));
    let begin = Instant::now();
    assert_eq!((terminate(child.process, 1), wait(child.process, INFINITE)), (OK, (OK, 128 + libc::SIGKILL)));
    assert!(begin.elapsed() < Duration::from_secs(2));
    assert_eq!(finish(child), 128 + libc::SIGKILL);
}

#[test]
fn a_child_starts_with_no_signal_blocked_or_ignored() {
    let _turn = turn();
    // What this thread blocks and this program ignores stays here: a child that inherited either would never see the request to end.
    let (mut blocked, before) = (unsafe { std::mem::zeroed::<libc::sigset_t>() }, mask());
    unsafe { libc::sigemptyset(&mut blocked); libc::sigaddset(&mut blocked, libc::SIGTERM); libc::sigaddset(&mut blocked, libc::SIGUSR1) };
    for (block, ignore) in [(true, false), (false, true), (true, true)] {
        if block { assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, ptr::null_mut()) }, 0); }
        if ignore { unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN); libc::signal(libc::SIGPIPE, libc::SIG_IGN) }; }
        let (status, child) = start("/bin/sleep", &["sleep", "5"], None, None, 0);
        unsafe { libc::signal(libc::SIGTERM, libc::SIG_DFL); libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, ptr::null_mut()) }, 0);
        assert_eq!(status, OK);
        assert_eq!((terminate(child.process, 0), wait(child.process, 3000 * MS)), (OK, (OK, 128 + libc::SIGTERM)), "blocked {block}, ignored {ignore}");
        assert_eq!(finish(child), 128 + libc::SIGTERM);
    }
    // A child that could not start is named the same way on the path that resets them.
    assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, ptr::null_mut()) }, 0);
    let refused = [start("/nonexistent/program", &["program"], None, None, PIPE_OUTPUT).0, start("/bin", &["program"], None, None, 0).0,
        start("/bin/sh", &["sh"], None, Some("/nonexistent/directory"), 0).0];
    assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, ptr::null_mut()) }, 0);
    assert_eq!(refused, [NOT_FOUND, ACCESS_DENIED, NOT_FOUND]);
}

#[test]
fn waiting_threads_all_learn_the_code_and_do_not_keep_a_child_from_being_ended() {
    let _turn = turn();
    let child = started("/bin/sh", &["sh", "-c", "sleep 0.3; exit 9"], None, None, 0);
    let process = Shared(child.process);
    let answers = std::thread::scope(|scope| {
        let waiters = [INFINITE, INFINITE, 5000 * MS, 5000 * MS].map(|limit| scope.spawn(move || wait(process.get(), limit)));
        // Too short to see the end: this one runs out while the others keep waiting.
        assert_eq!(wait(process.get(), 20 * MS), (TIMEOUT, 0));
        waiters.map(|waiter| waiter.join().unwrap())
    });
    assert_eq!(answers, [(OK, 9); 4]);
    assert_eq!(finish(child), 9);
    // A thread that waits without a limit, as the consumer's watcher does, while another one ends the child.
    let child = started("/bin/sleep", &["sleep", "5"], None, None, 0);
    let process = Shared(child.process);
    let begin = Instant::now();
    let answer = std::thread::scope(|scope| {
        let watcher = scope.spawn(move || wait(process.get(), INFINITE));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(terminate(process.get(), 1), OK);
        watcher.join().unwrap()
    });
    assert!(answer == (OK, 128 + libc::SIGKILL) && begin.elapsed() < Duration::from_secs(2));
    assert_eq!(finish(child), 128 + libc::SIGKILL);
}

#[test]
fn a_child_that_cannot_start_is_named_and_leaves_nothing() {
    let _turn = turn();
    let opened = descriptors();
    let all = PIPE_INPUT | PIPE_OUTPUT | PIPE_ERROR;
    assert_eq!(start("/nonexistent/program", &["program"], None, None, all).0, NOT_FOUND);
    // The path is used as given: a bare name is a file in the working directory, not a search of PATH.
    assert_eq!(start("sh", &["sh", "-c", "exit 0"], None, None, all).0, NOT_FOUND);
    assert_eq!(start("/bin/sh/program", &["program"], None, None, all).0, NOT_DIRECTORY);
    // Linux and macOS refuse to execute a directory with EACCES, not EISDIR.
    assert_eq!(start("/bin", &["program"], None, None, all).0, ACCESS_DENIED);
    let plain = std::env::temp_dir().join(format!("pal-processes-{}", std::process::id()));
    std::fs::write(&plain, "#!/bin/sh\nexit 4\n").unwrap();
    std::fs::set_permissions(&plain, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    assert_eq!(start(plain.to_str().unwrap(), &["program"], None, None, all).0, ACCESS_DENIED);
    std::fs::set_permissions(&plain, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(finish(started(plain.to_str().unwrap(), &["program"], None, None, 0)), 4);
    assert_eq!(start("/bin/sh", &["sh", "-c", "exit 0"], None, Some("/nonexistent/directory"), all).0, NOT_FOUND);
    assert_eq!(start("/bin/sh", &["sh", "-c", "exit 0"], None, plain.to_str(), all).0, NOT_DIRECTORY);
    std::fs::remove_file(&plain).unwrap();
    assert_eq!(descriptors(), opened);
}

#[test]
fn no_descriptor_leaks_and_no_zombie_stays() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let _turn = turn();
    // Model an embedding host (including CI runners) with a deliberately
    // inheritable descriptor. F_DUPFD clears CLOEXEC, unlike File::try_clone.
    let source = std::fs::File::open("/dev/null").unwrap();
    let sentinel = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD, 128) };
    assert!(sentinel >= 0);
    let sentinel = unsafe { OwnedFd::from_raw_fd(sentinel) };
    let opened = descriptors();
    let inherited = inherited_descriptors();
    assert!(inherited.contains(&sentinel.as_raw_fd()));
    // Keep provider pipes alive while another child lists its descriptors.
    // Test existence after expansion to exclude the glob's now-closed directory.
    let held = [(); 2].map(|_| started("/bin/cat", &["cat"], None, None, PIPE_INPUT | PIPE_OUTPUT | PIPE_ERROR));
    let (code, text) = shell("for f in /dev/fd/*; do if [ -e \"$f\" ]; then echo \"${f##*/}\"; fi; done");
    let mut listed = text.lines().map(|line| line.parse::<i32>().unwrap()).collect::<Vec<_>>();
    listed.sort_unstable();
    assert_eq!(code, 0);
    assert_eq!(listed, inherited, "only the host's inheritable descriptors may reach the child");
    for child in held { assert_eq!(finish(child), 0); }
    assert_eq!(descriptors(), opened);
    // Giving the handle back does not end the child: it is this test that ends it, and that reaps it.
    let child = started("/bin/sleep", &["sleep", "5"], None, None, 0);
    let (pid, mut status) = (child.id as libc::pid_t, 0);
    assert_eq!((release(child.process), descriptors()), (OK, opened));
    std::thread::sleep(Duration::from_millis(50));
    assert!(unsafe { libc::kill(pid, 0) == 0 && libc::waitpid(pid, &mut status, libc::WNOHANG) == 0 });
    assert!(unsafe { libc::kill(pid, libc::SIGKILL) == 0 && libc::waitpid(pid, &mut status, 0) == pid } && libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGKILL);
    // A child that had already ended when its handle was given back, without a wait, leaves no zombie.
    let child = started("/bin/sh", &["sh", "-c", "exit 0"], None, None, 0);
    let pid = child.id as libc::pid_t;
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    assert_eq!(unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, info.as_mut_ptr(), libc::WEXITED | libc::WNOWAIT) }, 0);
    assert_eq!(release(child.process), OK);
    assert!(unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD));
    for round in 0..50 {
        let child = started("/bin/sh", &["sh", "-c", &format!("exit {}", round % 8)], None, None, 0);
        assert_eq!((wait(child.process, INFINITE), release(child.process)), ((OK, round % 8), OK));
        assert!(unsafe { libc::waitpid(child.id as libc::pid_t, &mut status, libc::WNOHANG) } == -1);
    }
    assert_eq!(descriptors(), opened);
}

#[test]
fn arguments_are_validated_and_calls_counted() {
    let _turn = turn();
    let p = ops();
    let child = started("/bin/cat", &["cat"], None, None, PIPE_INPUT | PIPE_OUTPUT);
    let before = stats();
    let arguments = [c"sh".as_ptr().cast::<u8>(), c"-c".as_ptr().cast(), c"exit 0".as_ptr().cast()];
    let holed = [c"sh".as_ptr().cast::<u8>(), ptr::null(), c"exit 0".as_ptr().cast()];
    let program = b"/bin/sh";
    let mut out = Spawned::EMPTY;
    let size = std::mem::size_of::<Spawned>();
    let spawn = p.spawn.unwrap();
    unsafe {
        assert_eq!(spawn(ptr::null(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 0, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, arguments.as_ptr(), 0, ptr::null(), 0, ptr::null(), 0, 0, &mut out, size), INVALID_ARGUMENT, "no argument 0");
        assert_eq!(spawn(program.as_ptr(), 7, ptr::null(), 3, ptr::null(), 0, ptr::null(), 0, 0, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, holed.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 1, ptr::null(), 0, 0, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 4, 0, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 8, &mut out, size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, ptr::null_mut(), size), INVALID_ARGUMENT);
        assert_eq!(spawn(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &mut out, size - 1), INVALID_ARGUMENT);
        let (mut code, mut done, mut byte) = (7, 7, 0u8);
        assert_eq!((p.wait.unwrap()(ptr::null_mut(), 0, &mut code), code), (INVALID_ARGUMENT, 0));
        assert_eq!(p.wait.unwrap()(child.process, 0, ptr::null_mut()), INVALID_ARGUMENT);
        assert_eq!((terminate(ptr::null_mut(), 0), terminate(child.process, 2), release(ptr::null_mut()), close(ptr::null_mut())), (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((p.pipe_read.unwrap()(ptr::null_mut(), &mut byte, 1, &mut done), done), (INVALID_ARGUMENT, 0));
        assert_eq!(p.pipe_read.unwrap()(child.output, ptr::null_mut(), 1, &mut done), INVALID_ARGUMENT);
        assert_eq!(p.pipe_read.unwrap()(child.output, &mut byte, 1, ptr::null_mut()), INVALID_ARGUMENT);
        assert_eq!(p.pipe_write.unwrap()(ptr::null_mut(), &byte, 1, &mut done), INVALID_ARGUMENT);
        assert_eq!(p.pipe_write.unwrap()(child.input, ptr::null(), 1, &mut done), INVALID_ARGUMENT);
        assert_eq!(p.pipe_write.unwrap()(child.input, &byte, 1, ptr::null_mut()), INVALID_ARGUMENT);
        assert_eq!(p.read_stats.unwrap()(ptr::null_mut(), std::mem::size_of::<Stats>()), INVALID_ARGUMENT);
    }
    let after = stats();
    assert_eq!(after.rejected_or_failed, before.rejected_or_failed + 22);
    assert_eq!((after.spawn_ok, after.wait_ok, after.terminate_ok, after.release_ok, after.pipe_read_ok, after.pipe_write_ok, after.pipe_close_ok),
        (before.spawn_ok, before.wait_ok, before.terminate_ok, before.release_ok, before.pipe_read_ok, before.pipe_write_ok, before.pipe_close_ok));
    // Nothing to transfer is no transfer, and no answer about the other end either.
    assert_eq!((read(child.output, &mut []), write(child.input, &[])), ((OK, 0), (OK, 0)));
    assert_eq!(unsafe { libc::kill(child.id as libc::pid_t, 0) }, 0);
    assert_eq!(finish(child), 0);
    let done = stats();
    assert_eq!((done.spawn_ok, done.wait_ok, done.release_ok, done.pipe_close_ok, done.pipe_read_ok, done.pipe_write_ok),
        (before.spawn_ok, before.wait_ok + 1, before.release_ok + 1, before.pipe_close_ok + 2, before.pipe_read_ok + 1, before.pipe_write_ok + 1));
}
