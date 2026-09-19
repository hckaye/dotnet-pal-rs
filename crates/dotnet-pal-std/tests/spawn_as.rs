//! Drives the provider of children under another identity through the negotiated C
//! table, and handles those children through the processes group, whose children
//! they are. Most of it needs the privilege to change identity (root, as in the
//! test container) and says so when it is skipped. What a process without the
//! privilege may and may not do runs everywhere: directly when the tests are not
//! root, and otherwise in a copy of this test binary started as another user.
//! Unix only; the tests take turns because they count this process's descriptors.
#![cfg(unix)]
use dotnet_pal_rs::io::{ACCESS_DENIED, BROKEN_PIPE, NOT_DIRECTORY};
use dotnet_pal_rs::kernel::{INFINITE, TIMEOUT};
use dotnet_pal_rs::processes::{Spawned, PIPE_ERROR, PIPE_INPUT, PIPE_OUTPUT};
use dotnet_pal_rs::runtime::NOT_FOUND;
use dotnet_pal_rs::spawn_as::{Identity, Stats, MAX_GROUPS};
use dotnet_pal_rs::{Api, INVALID_ARGUMENT, OK};
use std::{ffi::{c_void, CString}, os::unix::{fs::PermissionsExt, process::CommandExt}, ptr, sync::{Mutex, MutexGuard}, time::{Duration, Instant}};

const MS: u64 = 1_000_000;
const WHO: &str = "id -u; id -g; id -G";
static TURN: Mutex<()> = Mutex::new(());
/// One test at a time, with the default action for SIGPIPE.
fn turn() -> MutexGuard<'static, ()> {
    let guard = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    guard
}
fn api() -> &'static Api {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    let both = dotnet_pal_rs::spawn_as::CAP | dotnet_pal_rs::processes::CAP;
    assert_eq!(api.header.capabilities & both, both);
    api
}
fn stats() -> Stats {
    let mut stats = Stats::default();
    assert_eq!(unsafe { api().spawn_as.read_stats.unwrap()(&mut stats, std::mem::size_of::<Stats>()) }, OK);
    stats
}
/// Starts and refusals of `spawn_as` since `before`.
fn counted(before: &Stats) -> (u64, u64) {
    let now = stats();
    (now.spawn_ok - before.spawn_ok, now.rejected_or_failed - before.rejected_or_failed)
}
/// Open descriptors of this process; the listing's own is in every count.
fn descriptors() -> usize { std::fs::read_dir("/dev/fd").unwrap().count() }
fn privileged() -> bool {
    let root = unsafe { libc::geteuid() } == 0;
    if !root { eprintln!("note: not root, so the children under another identity were not started"); }
    root
}
/// A user, a group and a list of groups.
#[derive(Clone)]
struct Who(u32, u32, Vec<u32>);
fn other() -> Who { Who(12345, 23456, vec![23456, 34567, 45678]) }

/// Starts a program under an identity or, without one, through the processes group. What the boundary takes is
/// counted, not terminated: the paths are followed by other bytes.
fn start(who: Option<&Who>, program: &str, arguments: &[&str], environment: Option<&[&str]>, directory: Option<&str>, pipes: u32) -> (u32, Spawned) {
    let owned = |list: &[&str]| list.iter().map(|text| CString::new(*text).unwrap()).collect::<Vec<_>>();
    let (arguments, environment) = (owned(arguments), environment.map(owned));
    let pointers = |list: &[CString]| list.iter().map(|text| text.as_ptr().cast::<u8>()).collect::<Vec<_>>();
    let (argv, envp) = (pointers(&arguments), environment.as_deref().map(pointers));
    let unterminated = |text: &str| [text.as_bytes(), b"XXXX"].concat();
    let (path, home) = (unterminated(program), directory.map(unterminated));
    let (envp, envc) = (envp.as_ref().map_or(ptr::null(), |list| list.as_ptr()), envp.as_ref().map_or(0, |list| list.len()));
    let (home, homec) = (home.as_ref().map_or(ptr::null(), |path| path.as_ptr()), directory.map_or(0, str::len));
    let filled = ptr::dangling_mut::<c_void>();
    let mut spawned = Spawned { process: filled, id: 7, input: filled, output: filled, error: filled };
    let size = std::mem::size_of::<Spawned>();
    let status = match who {
        Some(who) => {
            let identity = Identity { user_id: who.0, group_id: who.1, groups: if who.2.is_empty() { ptr::null() } else { who.2.as_ptr() }, group_count: who.2.len() };
            unsafe { api().spawn_as.spawn_as.unwrap()(path.as_ptr(), program.len(), argv.as_ptr(), argv.len(), envp, envc, home, homec, pipes, &identity, &mut spawned, size) }
        }
        None => unsafe { api().processes.spawn.unwrap()(path.as_ptr(), program.len(), argv.as_ptr(), argv.len(), envp, envc, home, homec, pipes, &mut spawned, size) },
    };
    if status == OK {
        assert!(!spawned.process.is_null() && spawned.id != 0 && unsafe { libc::kill(spawned.id as libc::pid_t, 0) } == 0);
        assert_eq!((spawned.input.is_null(), spawned.output.is_null(), spawned.error.is_null()), (pipes & PIPE_INPUT == 0, pipes & PIPE_OUTPUT == 0, pipes & PIPE_ERROR == 0));
    } else {
        assert!(spawned.process.is_null() && spawned.id == 0 && spawned.input.is_null() && spawned.output.is_null() && spawned.error.is_null());
    }
    (status, spawned)
}
fn started(who: Option<&Who>, program: &str, arguments: &[&str], environment: Option<&[&str]>, directory: Option<&str>, pipes: u32) -> Spawned {
    let (status, spawned) = start(who, program, arguments, environment, directory, pipes);
    assert_eq!(status, OK, "{program} {arguments:?}");
    spawned
}
fn refusal(who: &Who, program: &str, arguments: &[&str], directory: Option<&str>) -> u32 { start(Some(who), program, arguments, None, directory, PIPE_INPUT | PIPE_OUTPUT | PIPE_ERROR).0 }
fn wait(process: *mut c_void, timeout_ns: u64) -> (u32, i32) {
    let mut code = 7;
    (unsafe { api().processes.wait.unwrap()(process, timeout_ns, &mut code) }, code)
}
fn terminate(process: *mut c_void, forceful: u32) -> u32 { unsafe { api().processes.terminate.unwrap()(process, forceful) } }
fn release(process: *mut c_void) -> u32 { unsafe { api().processes.release.unwrap()(process) } }
fn close(pipe: *mut c_void) -> u32 { unsafe { api().processes.pipe_close.unwrap()(pipe) } }
fn read(pipe: *mut c_void, buffer: &mut [u8]) -> (u32, usize) {
    let mut got = 7;
    (unsafe { api().processes.pipe_read.unwrap()(pipe, buffer.as_mut_ptr(), buffer.len(), &mut got) }, got)
}
fn write(pipe: *mut c_void, data: &[u8]) -> (u32, usize) {
    let mut written = 7;
    (unsafe { api().processes.pipe_write.unwrap()(pipe, data.as_ptr(), data.len(), &mut written) }, written)
}
/// Reads a pipe to its end, which is zero bytes with OK, as often as it is asked.
fn drain(pipe: *mut c_void) -> String {
    let (mut all, mut buffer) = (Vec::new(), [0u8; 4096]);
    loop {
        let (status, got) = read(pipe, &mut buffer);
        assert!(status == OK && got <= buffer.len());
        if got == 0 { break; }
        all.extend_from_slice(&buffer[..got]);
    }
    assert_eq!(read(pipe, &mut buffer), (OK, 0));
    String::from_utf8(all).unwrap()
}
/// Closes the pipes, waits for the end, gives the handle back.
fn finish(child: Spawned) -> i32 {
    for pipe in [child.input, child.output, child.error] { if !pipe.is_null() { assert_eq!(close(pipe), OK); } }
    let (status, code) = wait(child.process, INFINITE);
    assert_eq!((status, release(child.process)), (OK, OK));
    code
}
/// Runs a program to its end and keeps what it wrote to its output.
fn run(who: Option<&Who>, program: &str, arguments: &[&str], environment: Option<&[&str]>, directory: Option<&str>) -> (i32, String) {
    let child = started(who, program, arguments, environment, directory, PIPE_OUTPUT);
    let output = drain(child.output);
    (finish(child), output)
}
fn shell(who: Option<&Who>, script: &str) -> (i32, String) { run(who, "/bin/sh", &["sh", "-c", script], None, None) }
/// A handle other threads may use while this one does.
#[derive(Clone, Copy)]
struct Shared(*mut c_void);
unsafe impl Send for Shared {}
impl Shared { fn get(self) -> *mut c_void { self.0 } }
/// The ids and the groups of every thread of this process, as procfs lists them.
#[cfg(target_os = "linux")]
fn threads_identities() -> Vec<String> {
    let mut all = Vec::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap() {
        // A thread that has ended since is no thread to look at.
        let Ok(text) = std::fs::read_to_string(task.unwrap().path().join("status")) else { continue; };
        all.push(text.lines().filter(|line| ["Uid:", "Gid:", "Groups:"].iter().any(|name| line.starts_with(name))).collect::<Vec<_>>().join("\n"));
    }
    all
}

#[test]
fn a_child_is_the_user_the_group_and_the_groups_it_was_given() {
    let _turn = turn();
    if !privileged() { return; }
    let before = stats();
    let (code, text) = shell(Some(&other()), WHO);
    // macOS adds the groups its directory service computes for the user to what id lists.
    if cfg!(target_os = "linux") { assert_eq!((code, text.as_str()), (0, "12345\n23456\n23456 34567 45678\n")); } else { assert!(code == 0 && text.starts_with("12345\n23456\n23456 "), "{text}"); }
    // The identity this process has is one it may take, without its groups too.
    let (code, text) = shell(Some(&Who(0, 0, vec![])), WHO);
    if cfg!(target_os = "linux") { assert_eq!((code, text.as_str()), (0, "0\n0\n0\n")); } else { assert!(code == 0 && text.starts_with("0\n0\n0"), "{text}"); }
    #[cfg(target_os = "linux")]
    {
        // Real, effective, saved and file system id are the new ones, and nothing of the privilege is left to take up again.
        let child = started(Some(&other()), "/bin/sleep", &["sleep", "5"], None, None, 0);
        let status = std::fs::read_to_string(format!("/proc/{}/status", child.id)).unwrap();
        let line = |name: &str| status.lines().find_map(|line| line.strip_prefix(name)).unwrap_or_else(|| panic!("{name} in {status}")).to_owned();
        assert_eq!((line("Name:\t"), line("Uid:\t"), line("Gid:\t"), line("Groups:\t")), ("sleep".into(), "12345\t12345\t12345\t12345".into(), "23456\t23456\t23456\t23456".into(), "23456 34567 45678 ".into()));
        assert_eq!((line("CapPrm:\t"), line("CapEff:\t")), ("0000000000000000".into(), "0000000000000000".into()));
        assert_eq!((terminate(child.process, 1), finish(child)), (OK, 128 + libc::SIGKILL));
        // The list is the whole list: the primary group is a supplementary group only when the list names it, and an empty list leaves none.
        let groups = format!("{WHO}; grep Groups: /proc/self/status");
        assert_eq!(shell(Some(&Who(12345, 23456, vec![45678, 34567])), &groups), (0, "12345\n23456\n23456 34567 45678\nGroups:\t34567 45678 \n".into()));
        assert_eq!(shell(Some(&Who(12345, 23456, vec![])), &groups), (0, "12345\n23456\n23456\nGroups:\t \n".into()));
        // The child cannot do what only the parent's user may.
        let child = started(Some(&other()), "/bin/sh", &["sh", "-c", "id -u; cat /etc/shadow"], None, None, PIPE_OUTPUT | PIPE_ERROR);
        assert!(std::fs::read("/etc/shadow").is_ok());
        let (output, errors) = (drain(child.output), drain(child.error));
        assert!(output == "12345\n" && errors.contains("Permission denied") && finish(child) != 0, "{output} {errors}");
    }
    assert_eq!(counted(&before), (if cfg!(target_os = "linux") { 6 } else { 2 }, 0));
}

#[test]
fn the_child_is_a_child_of_the_processes_group() {
    let _turn = turn();
    if !privileged() { return; }
    let (who, before, opened) = (other(), stats(), descriptors());
    for code in [0, 7, 255] { assert_eq!(shell(Some(&who), &format!("exit {code}")), (code, String::new())); }
    assert_eq!(run(Some(&who), "/bin/sh", &["another name", "-c", "echo \"$0\""], None, None), (0, "another name\n".into()));
    let script = "for a; do printf '[%s]' \"$a\"; done; echo $#";
    assert_eq!(run(Some(&who), "/bin/sh", &["sh", "-c", script, "zero", "two words", "", "*"], None, None), (0, "[two words][][*]3\n".into()));
    let child = started(Some(&who), "/bin/sh", &["sh", "-c", "echo $$"], None, None, PIPE_OUTPUT);
    assert_eq!(drain(child.output).trim().parse::<u64>().unwrap(), child.id);
    assert_eq!(finish(child), 0);
    // A given environment is the whole environment; none given is the parent's, not the new user's.
    let (code, text) = run(Some(&who), "/usr/bin/env", &["env"], Some(&["PAL_GIVEN=yes", "PAL_EMPTY="]), None);
    let mut lines = text.lines().collect::<Vec<_>>();
    lines.sort_unstable();
    assert_eq!((code, lines), (0, vec!["PAL_EMPTY=", "PAL_GIVEN=yes"]));
    let path = std::env::var("PATH").expect("the test needs a PATH to find inherited");
    let (code, text) = run(Some(&who), "/usr/bin/env", &["env"], None, None);
    assert!(code == 0 && text.lines().any(|line| line == format!("PATH={path}")) && !text.contains("PAL_GIVEN"));
    // All three pipes: the child reads what the parent writes until the parent closes, and its two outputs stay apart.
    let mut child = started(Some(&who), "/bin/sh", &["sh", "-c", "cat; echo problem >&2"], None, None, PIPE_INPUT | PIPE_OUTPUT | PIPE_ERROR);
    assert_eq!(write(child.input, b"to the child\n"), (OK, 13));
    assert_eq!(close(std::mem::replace(&mut child.input, ptr::null_mut())), OK);
    assert_eq!((drain(child.output), drain(child.error)), ("to the child\n".into(), "problem\n".into()));
    assert_eq!(finish(child), 0);
    // A write to a child that has ended is a status.
    let child = started(Some(&who), "/bin/sh", &["sh", "-c", "exit 3"], None, None, PIPE_INPUT);
    assert_eq!((wait(child.process, INFINITE), write(child.input, b"x"), finish(child)), ((OK, 3), (BROKEN_PIPE, 0), 3));
    // Waits run out, a request to end reaches the other user's process, and every later wait says the same.
    let child = started(Some(&who), "/bin/sleep", &["sleep", "5"], None, None, 0);
    assert_eq!(wait(child.process, 0), (TIMEOUT, 0));
    let begin = Instant::now();
    assert_eq!(wait(child.process, 100 * MS), (TIMEOUT, 0));
    assert!(begin.elapsed() >= Duration::from_millis(100) && begin.elapsed() < Duration::from_secs(2));
    assert_eq!(terminate(child.process, 0), OK);
    for limit in [INFINITE, 0, 50 * MS] { assert_eq!(wait(child.process, limit), (OK, 128 + libc::SIGTERM)); }
    assert_eq!((terminate(child.process, 0), terminate(child.process, 1), release(child.process)), (NOT_FOUND, NOT_FOUND, OK));
    // A thread that waits without a limit, as the consumer's watcher does, while another one ends the child.
    let child = started(Some(&who), "/bin/sleep", &["sleep", "5"], None, None, 0);
    let process = Shared(child.process);
    let answer = std::thread::scope(|scope| {
        let watcher = scope.spawn(move || wait(process.get(), INFINITE));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(terminate(process.get(), 1), OK);
        watcher.join().unwrap()
    });
    assert_eq!((answer, finish(child)), ((OK, 128 + libc::SIGKILL), 128 + libc::SIGKILL));
    // Giving the handle back does not end the child: it is this test that ends it, and that reaps it.
    let child = started(Some(&who), "/bin/sleep", &["sleep", "5"], None, None, 0);
    let (pid, mut status) = (child.id as libc::pid_t, 0);
    assert_eq!((release(child.process), descriptors()), (OK, opened));
    assert!(unsafe { libc::kill(pid, 0) == 0 && libc::waitpid(pid, &mut status, libc::WNOHANG) == 0 });
    assert!(unsafe { libc::kill(pid, libc::SIGKILL) == 0 && libc::waitpid(pid, &mut status, 0) == pid } && libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGKILL);
    for round in 0..20 {
        let child = started(Some(&who), "/bin/sh", &["sh", "-c", &format!("exit {}", round % 8)], None, None, 0);
        assert_eq!((wait(child.process, INFINITE), release(child.process)), ((OK, round % 8), OK));
        assert!(unsafe { libc::waitpid(child.id as libc::pid_t, &mut status, libc::WNOHANG) } == -1);
    }
    assert_eq!((descriptors(), counted(&before)), (opened, (33, 0)));
}

#[test]
fn a_child_starts_with_no_signal_blocked_or_ignored_and_the_thread_keeps_its_mask() {
    let _turn = turn();
    if !privileged() { return; }
    let current = || {
        let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, ptr::null(), &mut mask) }, 0);
        mask
    };
    let listed = |mask: &libc::sigset_t| (1..32).filter(|number| unsafe { libc::sigismember(mask, *number) } == 1).collect::<Vec<_>>();
    let (mut blocked, before) = (unsafe { std::mem::zeroed::<libc::sigset_t>() }, current());
    unsafe { libc::sigemptyset(&mut blocked); libc::sigaddset(&mut blocked, libc::SIGTERM); libc::sigaddset(&mut blocked, libc::SIGUSR1) };
    for (block, ignore) in [(true, false), (false, true), (true, true)] {
        if block { assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, ptr::null_mut()) }, 0); }
        if ignore { unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) }; }
        let during = listed(&current());
        let (status, child) = start(Some(&other()), "/bin/sleep", &["sleep", "5"], None, None, 0);
        let after = listed(&current());
        unsafe { libc::signal(libc::SIGTERM, libc::SIG_DFL) };
        assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, ptr::null_mut()) }, 0);
        assert_eq!((status, after), (OK, during));
        assert_eq!((terminate(child.process, 0), wait(child.process, 3000 * MS)), (OK, (OK, 128 + libc::SIGTERM)), "blocked {block}, ignored {ignore}");
        assert_eq!(finish(child), 128 + libc::SIGTERM);
    }
}

#[test]
fn a_child_that_cannot_start_is_named_and_leaves_nothing() {
    let _turn = turn();
    if !privileged() { return; }
    let (who, before, opened) = (other(), stats(), descriptors());
    assert_eq!(refusal(&who, "/nonexistent/program", &["program"], None), NOT_FOUND);
    assert_eq!(refusal(&who, "sh", &["sh", "-c", "exit 0"], None), NOT_FOUND, "a bare name is a file in the working directory, not a search of PATH");
    assert_eq!(refusal(&who, "/bin/sh/program", &["program"], None), NOT_DIRECTORY);
    assert_eq!(refusal(&who, "/bin", &["program"], None), ACCESS_DENIED);
    // A directory and a program of the parent's user alone: the parent's user gets in, the child's does not.
    let home = std::env::temp_dir().canonicalize().unwrap().join(format!("pal-spawn-as-{}", std::process::id()));
    std::fs::create_dir(&home).unwrap();
    std::fs::set_permissions(&home, PermissionsExt::from_mode(0o700)).unwrap();
    let (tool, home_text) = (home.join("tool"), home.to_str().unwrap().to_owned());
    std::fs::write(&tool, "#!/bin/sh\nexit 4\n").unwrap();
    std::fs::set_permissions(&tool, PermissionsExt::from_mode(0o700)).unwrap();
    let shell_arguments = ["sh", "-c", "exit 0"];
    assert_eq!(finish(started(None, "/bin/sh", &shell_arguments, None, Some(&home_text), 0)), 0);
    assert_eq!(refusal(&who, "/bin/sh", &shell_arguments, Some(&home_text)), ACCESS_DENIED, "the directory is entered as the new user");
    std::fs::set_permissions(&home, PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(run(Some(&who), "/bin/sh", &["sh", "-c", "pwd -P"], None, Some(&home_text)), (0, format!("{home_text}\n")));
    assert_eq!(finish(started(None, tool.to_str().unwrap(), &["program"], None, None, 0)), 4);
    assert_eq!(refusal(&who, tool.to_str().unwrap(), &["program"], None), ACCESS_DENIED, "the program is executed as the new user");
    std::fs::set_permissions(&tool, PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(finish(started(Some(&who), tool.to_str().unwrap(), &["program"], None, None, 0)), 4);
    // A relative program is a file in the child's working directory.
    assert_eq!(finish(started(Some(&who), "tool", &["program"], None, Some(&home_text), 0)), 4);
    assert_eq!(finish(started(Some(&who), "./tool", &["program"], None, Some(&home_text), 0)), 4);
    assert_eq!(refusal(&who, "/bin/sh", &shell_arguments, Some("/nonexistent/directory")), NOT_FOUND);
    assert_eq!(refusal(&who, "/bin/sh", &shell_arguments, tool.to_str()), NOT_DIRECTORY);
    // An id the kernel has no user for.
    if cfg!(target_os = "linux") {
        assert_eq!(refusal(&Who(u32::MAX, 23456, who.2.clone()), "/bin/sh", &shell_arguments, None), INVALID_ARGUMENT);
        assert_eq!(refusal(&Who(12345, u32::MAX, who.2.clone()), "/bin/sh", &shell_arguments, None), INVALID_ARGUMENT);
    }
    std::fs::remove_dir_all(&home).unwrap();
    assert_eq!((descriptors(), counted(&before)), (opened, (4, if cfg!(target_os = "linux") { 10 } else { 8 })));
    let mut status = 0;
    assert!(unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) } == -1, "a child that failed was reaped");
}

/// What a process without the privilege may do: be what it is.
fn without_privilege() {
    let (user, group) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut held = vec![0 as libc::gid_t; 64];
    let count = unsafe { libc::getgroups(64, held.as_mut_ptr()) };
    assert!(user != 0 && count >= 0);
    held.truncate(count as usize);
    let (before, opened) = (stats(), descriptors());
    // Its own identity, and a list that names more groups than it holds (and, on macOS, than a process can hold): the child gains none of them.
    let (code, same) = shell(None, WHO);
    assert_eq!((code, shell(Some(&Who(user, group, held.clone())), WHO)), (0, (0, same.clone())));
    let generous = [&[45678][..], &held, &[7, 8, 9]].concat();
    assert_eq!(shell(Some(&Who(user, group, generous)), WHO), (0, same));
    // Another user, another group, and, for a process that holds groups, a list that lacks one of them: it cannot put a group down.
    let mut denied = vec![other(), Who(user + 1, group, held.clone()), Who(user, group + 1, held.clone()), Who(0, 0, held.clone())];
    if !held.is_empty() { denied.extend([Who(user, group, held[..held.len() - 1].to_vec()), Who(user, group, vec![])]); }
    for who in &denied { assert_eq!(refusal(who, "/bin/sh", &["sh", "-c", "exit 0"], None), ACCESS_DENIED, "{} {} {:?}", who.0, who.1, who.2); }
    // The other failures keep their names.
    let own = Who(user, group, held.clone());
    assert_eq!(refusal(&own, "/nonexistent/program", &["program"], None), NOT_FOUND);
    assert_eq!(refusal(&own, "/bin/sh", &["sh", "-c", "exit 0"], Some("/nonexistent/directory")), NOT_FOUND);
    assert_eq!((descriptors(), counted(&before)), (opened, (2, denied.len() as u64 + 2)));
}
#[test]
fn a_process_without_the_privilege_may_only_be_itself() {
    let _turn = turn();
    if unsafe { libc::geteuid() } != 0 { return without_privilege(); }
    // A copy of this test binary, where any user can execute it, runs this test again as a user that holds two groups and no privilege.
    let copy = std::env::temp_dir().join(format!("pal-spawn-as-copy-{}", std::process::id()));
    std::fs::copy(std::env::current_exe().unwrap(), &copy).unwrap();
    std::fs::set_permissions(&copy, PermissionsExt::from_mode(0o755)).unwrap();
    let mut command = std::process::Command::new(&copy);
    command.args(["--exact", "a_process_without_the_privilege_may_only_be_itself", "--test-threads=1"]).current_dir("/");
    unsafe {
        command.pre_exec(|| {
            let held: [libc::gid_t; 2] = [54321, 54322];
            if libc::setgroups(2, held.as_ptr()) != 0 || libc::setgid(54321) != 0 || libc::setuid(54321) != 0 { return Err(std::io::Error::last_os_error()); }
            Ok(())
        });
    }
    let output = command.output();
    std::fs::remove_file(&copy).unwrap();
    let output = output.unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && text.contains("1 passed"), "{text}\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn threads_that_start_children_at_once_keep_the_identity_of_this_process() {
    let _turn = turn();
    if !privileged() { return; }
    let (before, opened) = (stats(), descriptors());
    #[cfg(target_os = "linux")]
    let (identities, dumpable) = (threads_identities(), unsafe { libc::prctl(libc::PR_GET_DUMPABLE) });
    #[cfg(target_os = "linux")]
    assert!(identities.iter().all(|identity| *identity == identities[0]) && dumpable == 1, "{identities:?} {dumpable}");
    let looks = std::thread::scope(|scope| {
        let spawners = (0..5u32).map(|number| scope.spawn(move || {
            let who = Who(12345 + number, 23456 + number, vec![34567 + number, 45678 + number]);
            let expected = format!("{}\n{}\n{} {} {}\n", who.0, who.1, who.1, who.2[0], who.2[1]);
            for _ in 0..10 {
                let (code, text) = shell(Some(&who), WHO);
                if cfg!(target_os = "linux") { assert_eq!((code, text), (0, expected.clone())); } else { assert!(code == 0 && text.starts_with(&expected[..expected.len() - 1]), "{text}"); }
            }
        })).collect::<Vec<_>>();
        let mut looks = 0;
        // The identity is the child's alone: every thread of this process keeps its ids and groups at every moment a look is taken.
        while !spawners.iter().all(|spawner| spawner.is_finished()) {
            #[cfg(target_os = "linux")]
            for identity in threads_identities() { assert_eq!(identity, identities[0]); }
            looks += 1;
            std::thread::yield_now();
        }
        for spawner in spawners { spawner.join().unwrap(); }
        looks
    });
    #[cfg(target_os = "linux")]
    assert!(threads_identities().iter().all(|identity| *identity == identities[0]) && unsafe { libc::prctl(libc::PR_GET_DUMPABLE) } == dumpable);
    assert!(looks > 0 && unsafe { libc::getuid() == 0 && libc::geteuid() == 0 });
    assert_eq!((descriptors(), counted(&before)), (opened, (50, 0)));
}

#[test]
fn arguments_are_validated_and_calls_counted() {
    let _turn = turn();
    let spawn_as = api().spawn_as.spawn_as.unwrap();
    let (before, opened) = (stats(), descriptors());
    let arguments = [c"sh".as_ptr().cast::<u8>(), c"-c".as_ptr().cast(), c"exit 0".as_ptr().cast()];
    let holed = [c"sh".as_ptr().cast::<u8>(), ptr::null(), c"exit 0".as_ptr().cast()];
    let nameless = [c"=1".as_ptr().cast::<u8>()];
    let (program, size) = (b"/bin/sh", std::mem::size_of::<Spawned>());
    let groups = [unsafe { libc::getgid() }; 2];
    let identity = Identity { user_id: unsafe { libc::getuid() }, group_id: unsafe { libc::getgid() }, groups: groups.as_ptr(), group_count: 2 };
    let filled = ptr::dangling_mut::<c_void>();
    let mut out = Spawned { process: filled, id: 7, input: filled, output: filled, error: filled };
    let mut refused = 0;
    let mut invalid = |status: u32, out: &Spawned| {
        assert!(status == INVALID_ARGUMENT && out.process.is_null() && out.id == 0 && out.input.is_null() && out.output.is_null() && out.error.is_null());
        refused += 1;
    };
    unsafe {
        // The request is the one of spawn, and is refused as spawn refuses it.
        invalid(spawn_as(ptr::null(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 0, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 0, ptr::null(), 0, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, ptr::null(), 3, ptr::null(), 0, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, holed.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 1, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, nameless.as_ptr(), 1, ptr::null(), 0, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 4, 0, &identity, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 8, &identity, &mut out, size), &out);
        // The identity: there is one, where an identity can be, and its list is as long as it says and no longer than a list may be.
        let many = vec![0u32; MAX_GROUPS + 1];
        let list = |groups: *const u32, group_count: usize| Identity { groups, group_count, ..identity };
        let mut odd = [0u64; 8];
        let misplaced = odd.as_mut_ptr().cast::<u8>().add(1).cast::<Identity>();
        misplaced.write_unaligned(identity);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, ptr::null(), &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, misplaced, &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &list(ptr::null(), 1), &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &list(many.as_ptr(), MAX_GROUPS + 1), &mut out, size), &out);
        invalid(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &list(many.as_ptr().cast::<u8>().add(1).cast(), 1), &mut out, size), &out);
        // An output that cannot be written to is left alone.
        out.id = 7;
        assert_eq!(spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &identity, ptr::null_mut(), size), INVALID_ARGUMENT);
        assert_eq!((spawn_as(program.as_ptr(), 7, arguments.as_ptr(), 3, ptr::null(), 0, ptr::null(), 0, 0, &identity, &mut out, size - 1), out.id), (INVALID_ARGUMENT, 7));
        assert_eq!(api().spawn_as.read_stats.unwrap()(ptr::null_mut(), std::mem::size_of::<Stats>()), INVALID_ARGUMENT);
        let mut short = Stats::default();
        assert_eq!(api().spawn_as.read_stats.unwrap()(&mut short, std::mem::size_of::<Stats>() - 1), INVALID_ARGUMENT);
    }
    assert_eq!((descriptors(), counted(&before)), (opened, (0, refused + 2)));
}
