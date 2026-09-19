//! Exercises the notifications group of the std port with signals the OS really
//! delivers to the test process. The handler is installed once per process, so
//! everything that needs it lives in one test. The default actions that end or
//! stop a process are observed from outside: the children are this test binary,
//! started again with only `child_mode` selected.
#![cfg(unix)]
use dotnet_pal_rs::notifications::{deliver, Stats, CONTINUE, HANGUP, INTERRUPT, STOP, TERMINATE, WINDOW_CHANGE};
use dotnet_pal_rs::{kernel::BUSY, INVALID_ARGUMENT, OK};
use std::os::unix::process::CommandExt;
use std::sync::{atomic::{AtomicU32, Ordering}, mpsc, Mutex};
use std::{ffi::c_void, process::{Command, Stdio}, thread::ThreadId, time::Duration};

fn api() -> &'static dotnet_pal_rs::Api {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    unsafe { &*api }
}

const CHILD: &str = "PAL_NOTIFICATIONS_CHILD";
const WAIT: Duration = Duration::from_secs(10);
/// What the handler does with a kind it is told about.
const NOTHING: u32 = 0;
const DISABLE: u32 = 1;
const DEFAULT_ACTION: u32 = 2;
#[derive(Debug, PartialEq)]
enum Event { Report(u32, usize, ThreadId), Reacted(u32) }
static COOKIE: u32 = 0x1234;
static EVENTS: Mutex<Option<mpsc::Sender<Event>>> = Mutex::new(None);
static REACTION: AtomicU32 = AtomicU32::new(NOTHING);
static GATE: Mutex<()> = Mutex::new(());

/// Ordinary thread context: locks, the heap, a channel and the boundary itself are all in reach.
unsafe extern "C" fn handler(kind: u32, data: *mut c_void) {
    let tell = |event| EVENTS.lock().unwrap().as_ref().unwrap().send(event).unwrap();
    tell(Event::Report(kind, data as usize, std::thread::current().id()));
    let n = &api().notifications;
    match REACTION.load(Ordering::SeqCst) {
        DISABLE => { let _gate = GATE.lock().unwrap(); tell(Event::Reacted(unsafe { n.disable.unwrap()(kind) })); }
        DEFAULT_ACTION => tell(Event::Reacted(unsafe { n.default_action.unwrap()(kind) })),
        _ => {}
    }
}
fn events() -> mpsc::Receiver<Event> {
    let (sender, receiver) = mpsc::channel();
    *EVENTS.lock().unwrap() = Some(sender);
    receiver
}
fn send(number: i32) { assert_eq!(unsafe { libc::kill(libc::getpid(), number) }, 0); }
fn stats() -> Stats {
    let mut stats = Stats::default();
    assert_eq!(unsafe { api().notifications.read_stats.unwrap()(&mut stats, std::mem::size_of::<Stats>()) }, OK);
    stats
}
/// Runs the child to its end, continuing it when it stops: whether SIGTSTP stopped it on the way, and its wait status.
/// Reaped with waitpid, which unlike `Child::wait` also reports a stop.
#[allow(clippy::zombie_processes)]
fn child(mode: &str) -> (bool, i32) {
    let child = Command::new(std::env::current_exe().unwrap()).args(["--exact", "child_mode", "--nocapture", "--test-threads=1"]).env(CHILD, mode)
        // A group of its own with this process outside it: the kernel discards a stop aimed at an orphaned group.
        .process_group(0).stdout(Stdio::null()).spawn().unwrap();
    let pid = child.id() as libc::pid_t;
    let (mut status, mut stopped) = (0, false);
    loop {
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) }, pid);
        if !libc::WIFSTOPPED(status) { return (stopped, status); }
        assert_eq!(libc::WSTOPSIG(status), libc::SIGTSTP);
        stopped = true;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
    }
}
fn ended_by(status: i32, number: i32) -> bool { libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == number }

#[test]
fn child_mode() {
    let Some(mode) = std::env::var_os(CHILD) else { return; };
    // However this was started (a background job ignores SIGINT), the signals used here start from their defaults.
    for number in [libc::SIGINT, libc::SIGTERM, libc::SIGTSTP] { unsafe { libc::signal(number, libc::SIG_DFL) }; }
    unsafe { libc::alarm(30) };
    let n = &api().notifications;
    let events = events();
    assert_eq!(unsafe { n.install.unwrap()(Some(handler), ptr(&COOKIE)) }, OK);
    match mode.to_str().unwrap() {
        "disabled" => { // disable gives the signal its default action back
            assert_eq!(unsafe { n.enable.unwrap()(INTERRUPT) }, OK);
            assert_eq!(unsafe { n.disable.unwrap()(INTERRUPT) }, OK);
            send(libc::SIGINT);
        }
        "default" => { // on a thread that has the signal unblocked
            assert_eq!(unsafe { n.enable.unwrap()(TERMINATE) }, OK);
            send(libc::SIGTERM);
            assert!(matches!(events.recv_timeout(WAIT), Ok(Event::Report(TERMINATE, ..))));
            unsafe { n.default_action.unwrap()(TERMINATE) };
        }
        "default-in-handler" => { // on the reporting thread, which has it blocked
            REACTION.store(DEFAULT_ACTION, Ordering::SeqCst);
            assert_eq!(unsafe { n.enable.unwrap()(TERMINATE) }, OK);
            send(libc::SIGTERM);
        }
        "late-report" => {
            // A second interrupt is caught before the handler gets to disable the kind on the first report:
            // its report finds nobody who wants it, and the port owes the signal its default action.
            REACTION.store(DISABLE, Ordering::SeqCst);
            assert_eq!(unsafe { n.enable.unwrap()(INTERRUPT) }, OK);
            let gate = GATE.lock().unwrap();
            send(libc::SIGINT);
            assert!(matches!(events.recv_timeout(WAIT), Ok(Event::Report(INTERRUPT, ..))));
            send(libc::SIGINT);
            std::thread::sleep(Duration::from_millis(200));
            drop(gate);
        }
        "ignored" => {
            // A hangup the process was started to ignore (nohup) is reported once enabled, and its default action stays "ignore".
            unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
            assert_eq!(unsafe { n.enable.unwrap()(HANGUP) }, OK);
            send(libc::SIGHUP);
            assert!(matches!(events.recv_timeout(WAIT), Ok(Event::Report(HANGUP, ..))));
            assert_eq!(unsafe { n.default_action.unwrap()(HANGUP) }, OK);
            assert_eq!(unsafe { n.disable.unwrap()(HANGUP) }, OK);
            assert_eq!(unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) }, libc::SIG_IGN);
            return;
        }
        "stop" => {
            REACTION.store(DEFAULT_ACTION, Ordering::SeqCst);
            assert_eq!(unsafe { n.enable.unwrap()(STOP) }, OK);
            send(libc::SIGTSTP); // caught, so nothing stops until the handler asks for it
            assert!(matches!(events.recv_timeout(WAIT), Ok(Event::Report(STOP, ..))));
            // Stopped here until the parent continues the process.
            assert_eq!(events.recv_timeout(WAIT), Ok(Event::Reacted(OK)));
            // The kind is still enabled and is reported again.
            REACTION.store(NOTHING, Ordering::SeqCst);
            send(libc::SIGTSTP);
            assert!(matches!(events.recv_timeout(WAIT), Ok(Event::Report(STOP, ..))));
            return;
        }
        other => panic!("unknown child mode {other}"),
    }
    std::thread::sleep(WAIT);
    panic!("the default action left the process running");
}
fn ptr(cookie: &'static u32) -> *mut c_void { cookie as *const u32 as *mut c_void }

#[test]
fn reports_come_from_a_thread_of_the_port() {
    if std::env::var_os(CHILD).is_some() { return; }
    assert!(ended_by(child("disabled").1, libc::SIGINT));
    assert!(ended_by(child("default").1, libc::SIGTERM));
    assert!(ended_by(child("default-in-handler").1, libc::SIGTERM));
    assert!(ended_by(child("late-report").1, libc::SIGINT));
    assert_eq!(child("ignored"), (false, 0));
    let (stopped, status) = child("stop");
    assert!(stopped && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0, "stopped={stopped} status={status:#x}");

    let api = api();
    assert_eq!(api.header.capabilities & dotnet_pal_rs::notifications::CAP, dotnet_pal_rs::notifications::CAP);
    let n = &api.notifications;
    let (install, enable, disable, default_action) = (n.install.unwrap(), n.enable.unwrap(), n.disable.unwrap(), n.default_action.unwrap());
    assert_eq!(unsafe { install(None, ptr(&COOKIE)) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { enable(INTERRUPT) }, INVALID_ARGUMENT);
    let events = events();
    assert_eq!(unsafe { install(Some(handler), ptr(&COOKIE)) }, OK);
    assert_eq!(unsafe { install(Some(handler), ptr(&COOKIE)) }, BUSY);
    assert_eq!(unsafe { enable(0) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { enable(10) }, INVALID_ARGUMENT);
    let action = |number: i32| {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::sigaction(number, std::ptr::null(), &mut action) }, 0);
        action.sa_sigaction
    };
    let before = [libc::SIGINT, libc::SIGTERM, libc::SIGWINCH].map(action);
    for kind in [INTERRUPT, TERMINATE, WINDOW_CHANGE] { assert_eq!(unsafe { enable(kind) }, OK); }

    // Enabled, so this process survives all three; every report comes from the same thread, which is not this one.
    let mut reporter = None;
    let mut expect = |number: i32, kind: u32| {
        send(number);
        let Ok(Event::Report(got, data, thread)) = events.recv_timeout(WAIT) else { panic!("no report of kind {kind}") };
        assert_eq!((got, data), (kind, ptr(&COOKIE) as usize));
        assert_ne!(thread, std::thread::current().id());
        assert_eq!(*reporter.get_or_insert(thread), thread);
    };
    expect(libc::SIGWINCH, WINDOW_CHANGE);
    expect(libc::SIGINT, INTERRUPT);
    expect(libc::SIGTERM, TERMINATE);

    // Disabled, the window change is the target's business again (it ignores it); enabled again, it is reported again.
    assert_eq!(unsafe { disable(WINDOW_CHANGE) }, OK);
    send(libc::SIGWINCH);
    assert_eq!(events.recv_timeout(Duration::from_millis(300)), Err(mpsc::RecvTimeoutError::Timeout));
    assert_eq!(unsafe { enable(WINDOW_CHANGE) }, OK);
    expect(libc::SIGWINCH, WINDOW_CHANGE);
    assert_eq!(unsafe { disable(INTERRUPT) }, OK);
    assert_eq!(unsafe { enable(INTERRUPT) }, OK);
    assert_eq!(unsafe { enable(INTERRUPT) }, OK);
    expect(libc::SIGINT, INTERRUPT);

    // Nothing to do for the kinds the target ignores by default.
    assert_eq!(unsafe { default_action(WINDOW_CHANGE) }, OK);
    assert_eq!(unsafe { default_action(CONTINUE) }, OK);
    assert_eq!(unsafe { default_action(0) }, INVALID_ARGUMENT);
    // A report of a kind nobody enabled is dropped by the front end.
    assert!(!deliver(HANGUP));
    // Back to the actions this process started with, although INTERRUPT was enabled twice in a row.
    assert_ne!([libc::SIGINT, libc::SIGTERM, libc::SIGWINCH].map(action), before);
    for kind in [INTERRUPT, TERMINATE, WINDOW_CHANGE] { assert_eq!(unsafe { disable(kind) }, OK); }
    assert_eq!([libc::SIGINT, libc::SIGTERM, libc::SIGWINCH].map(action), before);

    let stats = stats();
    assert_eq!((stats.installs, stats.enabled, stats.disabled, stats.delivered, stats.dropped, stats.rejected), (1, 6, 5, 5, 1, 6));
}
