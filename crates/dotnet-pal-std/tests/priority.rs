//! Exercises the priority group of the std port through the negotiated C table, on
//! whatever Unix runs the test, against getpriority and, on Linux, the stat files of
//! procfs. What changes the priority of the test itself runs in a child of its own.
#![cfg(unix)]
use dotnet_pal_rs::io::ACCESS_DENIED;
use dotnet_pal_rs::priority::{self, Stats};
use dotnet_pal_rs::runtime::NOT_FOUND;
use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::{mem::size_of, ptr};

const CHILD: &str = "PAL_STD_PRIORITY_CHILD";
/// One test at a time: the counters are the process's.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
fn ops() -> &'static priority::Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & priority::CAP, priority::CAP);
    &api.priority
}
/// What the calls of this test should have added to the three counters.
static TALLY: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
fn tally(status: u32, index: usize) -> u32 { TALLY[if status == OK { index } else { 2 }].fetch_add(1, Ordering::Relaxed); status }
fn counters() -> ([u64; 3], [u64; 3]) {
    let mut out = Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<Stats>()) }, OK);
    ([out.get_ok, out.set_ok, out.rejected_or_failed], [0, 1, 2].map(|index| TALLY[index].load(Ordering::Relaxed)))
}
/// Runs `body` and checks that the counters moved by exactly what its calls should have added.
fn counted(body: impl FnOnce()) {
    let (stats, tally) = counters();
    body();
    let (stats_after, tally_after) = counters();
    assert_eq!([0, 1, 2].map(|index| stats_after[index] - stats[index]), [0, 1, 2].map(|index| tally_after[index] - tally[index]));
}
/// Status and value of one get; the value starts as garbage.
fn get(process: u64) -> (u32, i32) {
    let mut value = 77;
    (tally(unsafe { ops().get.unwrap()(process, &mut value) }, 0), value)
}
fn set(process: u64, value: i32) -> u32 { tally(unsafe { ops().set.unwrap()(process, value) }, 1) }
/// What getpriority says about a process or, on Linux, a thread. -1 is a value: only errno tells it from a failure.
fn nice_of(id: u32) -> i32 {
    #[cfg(target_os = "linux")]
    unsafe { *libc::__errno_location() = 0 };
    #[cfg(target_os = "macos")]
    unsafe { *libc::__error() = 0 };
    let value = unsafe { libc::getpriority(libc::PRIO_PROCESS, id as libc::id_t) };
    assert!(value != -1 || std::io::Error::last_os_error().raw_os_error().unwrap_or(0) == 0);
    value
}
/// The nice values procfs shows for every thread of a process.
#[cfg(target_os = "linux")]
fn thread_values(process: u32) -> Vec<i32> {
    std::fs::read_dir(format!("/proc/{process}/task")).unwrap().map(|thread| {
        let stat = std::fs::read_to_string(thread.unwrap().path().join("stat")).unwrap();
        // The 19th field, counted behind the command, which may hold spaces and parentheses.
        stat[stat.rfind(')').unwrap() + 2..].split(' ').nth(16).unwrap().parse().unwrap()
    }).collect()
}
/// Whether a process like this one may make itself more favoured again: root outside a container, or a nice limit that allows it.
fn may_lower() -> bool {
    match unsafe { libc::fork() } {
        -1 => panic!("fork"),
        // Only calls a forked child of a threaded process may make.
        0 => unsafe { libc::_exit(if libc::setpriority(libc::PRIO_PROCESS, 0, 19) == 0 && libc::setpriority(libc::PRIO_PROCESS, 0, -20) == 0 { 0 } else { 1 }) },
        child => { let mut status = 0; assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child); libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 }
    }
}
struct Reaped(Child);
impl Drop for Reaped { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
fn this_test(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "child_mode", "--nocapture", "--test-threads=1"]).env(CHILD, mode);
    command
}

#[test]
fn this_process_by_zero_and_by_its_id() {
    let _serial = serial();
    let output = this_test("self").output().unwrap();
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    // The harness prints the name of the child's test in front of the line.
    for line in String::from_utf8_lossy(&output.stdout).lines() { if let Some(at) = line.find("priority:") { println!("{}", &line[at..]); } }
}
#[test]
fn child_mode() {
    let Some(mode) = std::env::var_os(CHILD) else { return; };
    if mode == "crowd" {
        // Two more threads, a line that says they run, and a wait to be ended.
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        for _ in 0..2 { let barrier = barrier.clone(); std::thread::spawn(move || { barrier.wait(); loop { std::thread::park(); } }); }
        barrier.wait();
        println!("ready");
        loop { std::thread::park(); }
    }
    counted(|| {
        let (own, start) = (std::process::id(), nice_of(0));
        assert!(start <= 4, "the test starts from a nice value of 4 or less");
        assert_eq!(get(0), (OK, start));
        assert_eq!(get(own as u64), (OK, start));
        // The harness runs this test on a thread of its own, so the process has two threads at least: on Linux every one of them follows.
        assert_eq!(set(0, 5), OK);
        assert_eq!((get(0), nice_of(0), nice_of(own)), ((OK, 5), 5, 5));
        #[cfg(target_os = "linux")]
        {
            let values = thread_values(own);
            assert!(values.len() >= 2 && values.iter().all(|value| *value == 5), "{values:?}");
            // A thread with a value of its own does not change what the process answers, and is brought back by the next change.
            let (asked, answer) = (std::sync::Arc::new(std::sync::Barrier::new(2)), std::sync::Arc::new(std::sync::Barrier::new(2)));
            let apart = { let (asked, answer) = (asked.clone(), answer.clone()); std::thread::spawn(move || {
                let thread = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
                assert_eq!(unsafe { libc::setpriority(libc::PRIO_PROCESS, thread as libc::id_t, 9) }, 0);
                assert_eq!((nice_of(0), get(0)), (9, (OK, 5)));
                asked.wait();
                answer.wait();
                assert_eq!(nice_of(0), 10);
            }) };
            asked.wait();
            assert!(thread_values(own).contains(&9));
            assert_eq!(set(own as u64, 10), OK);
            let values = thread_values(own);
            assert!(values.len() >= 3 && values.iter().all(|value| *value == 10), "{values:?}");
            answer.wait();
            apart.join().unwrap();
        }
        #[cfg(not(target_os = "linux"))]
        assert_eq!((set(own as u64, 10), get(0), nice_of(0)), (OK, (OK, 10), 10));
        // Going back up is a privilege, and a refusal changes nothing.
        let lowering = may_lower();
        if lowering {
            for value in [0, -20, -1, 19, 0] { assert_eq!((set(0, value), get(0), nice_of(0)), (OK, (OK, value), value)); }
        } else {
            assert_eq!((set(0, 0), set(0, 9), set(0, -1)), (ACCESS_DENIED, ACCESS_DENIED, ACCESS_DENIED));
            assert_eq!((get(0), nice_of(0)), ((OK, 10), 10));
            #[cfg(target_os = "linux")]
            assert!(thread_values(own).iter().all(|value| *value == 10));
        }
        println!("priority: start={start} lowering={lowering}");
    });
}
#[test]
fn a_child_by_its_id_until_it_is_gone() {
    let _serial = serial();
    counted(|| {
        let base = nice_of(0);
        assert!(base <= 10);
        let child = Reaped(Command::new("sleep").arg("60").spawn().unwrap());
        let id = child.0.id();
        assert_eq!(set(id as u64, base + 7), OK);
        assert_eq!((get(id as u64), nice_of(id)), ((OK, base + 7), base + 7));
        #[cfg(target_os = "linux")]
        assert_eq!(thread_values(id), [base + 7]);
        // What another program sets is what the boundary reads. macOS has a value 20, which is the boundary's 19.
        assert_eq!(unsafe { libc::setpriority(libc::PRIO_PROCESS, id as libc::id_t, base + 8) }, 0);
        assert_eq!(get(id as u64), (OK, base + 8));
        assert_eq!(unsafe { libc::setpriority(libc::PRIO_PROCESS, id as libc::id_t, 20) }, 0);
        assert!(nice_of(id) == if cfg!(target_os = "macos") { 20 } else { 19 });
        assert_eq!(get(id as u64), (OK, 19));
        // A process that has ended and been waited for is no process.
        drop(child);
        assert_eq!((get(id as u64), set(id as u64, 19)), ((NOT_FOUND, 0), NOT_FOUND));
    });
}
#[cfg(target_os = "linux")]
#[test]
fn every_thread_of_another_process() {
    use std::io::{BufRead, BufReader};
    let _serial = serial();
    counted(|| {
        let base = nice_of(0);
        let mut crowd = Reaped(this_test("crowd").stdout(std::process::Stdio::piped()).spawn().unwrap());
        let mut line = String::new();
        // The harness prints its own lines first, and the name of the child's test in front of the line.
        let mut output = BufReader::new(crowd.0.stdout.take().unwrap());
        while !line.trim_end().ends_with("ready") { line.clear(); assert!(output.read_line(&mut line).unwrap() > 0); }
        let id = crowd.0.id();
        let threads = thread_values(id).len();
        assert!(threads >= 3);
        // setpriority alone changes the first thread.
        assert_eq!(unsafe { libc::setpriority(libc::PRIO_PROCESS, id as libc::id_t, base + 3) }, 0);
        assert_eq!(thread_values(id).iter().filter(|value| **value == base + 3).count(), 1);
        assert_eq!(set(id as u64, base + 8), OK);
        assert_eq!((thread_values(id), get(id as u64)), (vec![base + 8; threads], (OK, base + 8)));
    });
}
#[test]
fn ids_and_values_that_are_none() {
    let _serial = serial();
    counted(|| {
        let (own, before) = (std::process::id() as u64, nice_of(0));
        // An id no process can have is no process, and never this one by the low half of the number.
        for process in [i32::MAX as u64 + 1, u32::MAX as u64, 1 << 32, (1 << 32) + own, (1 << 63) + own, u64::MAX] {
            assert_eq!((get(process), set(process, 15)), ((NOT_FOUND, 0), NOT_FOUND));
        }
        for value in [-21, 20, i32::MIN, i32::MAX] { assert_eq!((set(0, value), set(own, value)), (INVALID_ARGUMENT, INVALID_ARGUMENT)); }
        assert_eq!(nice_of(0), before);
        assert_eq!(tally(unsafe { ops().get.unwrap()(0, ptr::null_mut()) }, 0), INVALID_ARGUMENT);
        assert_eq!(unsafe { ops().read_stats.unwrap()(ptr::null_mut(), size_of::<Stats>()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { ops().read_stats.unwrap()(&mut Stats::default(), size_of::<Stats>() - 1) }, INVALID_ARGUMENT);
    });
}
