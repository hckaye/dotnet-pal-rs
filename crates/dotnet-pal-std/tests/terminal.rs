//! The terminal group of the std port, checked against what the kernel reports
//! for a pseudo-terminal. `cargo test` has no terminal to offer, and a test that
//! replaced descriptors 0, 1 and 2 in the harness's own process would take the
//! harness's output with it. So the terminal part runs in a child: this test
//! binary again, told by an environment variable to put the slave side of a
//! pseudo-terminal behind its standard streams, to play the person at the
//! keyboard on the master side, and to exit with the verdict.
#![cfg(unix)]
use dotnet_pal_rs::runtime::NOT_FOUND;
use dotnet_pal_rs::terminal::{self, CONTROL_END_OF_FILE, CONTROL_END_OF_LINE, CONTROL_END_OF_LINE_2, CONTROL_ERASE};
use dotnet_pal_rs::{INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use std::{mem::MaybeUninit, ptr, time::Instant};

const CHILD: &str = "DOTNET_PAL_STD_TERMINAL_CHILD";

fn api() -> &'static dotnet_pal_rs::Api {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    unsafe { &*api }
}
/// The group's callbacks, and what the caller saw of them to compare with the counters.
struct Terminal { ops: &'static terminal::Ops, seen: [u64; 5] }
impl Terminal {
    fn new() -> Self {
        let api = api();
        assert_eq!(api.header.capabilities & terminal::CAP, terminal::CAP);
        Self { ops: &api.terminal, seen: [0; 5] }
    }
    fn note(&mut self, group: usize, status: u32) -> u32 { self.seen[if status == OK { group } else { 4 }] += 1; status }
    fn size(&mut self, stream: u32) -> (u32, u32, u32) {
        let (mut columns, mut rows) = (7, 7);
        let status = unsafe { self.ops.window_size.unwrap()(stream, &mut columns, &mut rows) };
        (self.note(0, status), columns, rows)
    }
    fn mode(&mut self, raw: u32, min_bytes: u32, timeout_ds: u32, interrupt_as_input: u32) -> u32 {
        let status = unsafe { self.ops.set_input_mode.unwrap()(raw, min_bytes, timeout_ds, interrupt_as_input) };
        self.note(1, status)
    }
    fn ready(&mut self) -> (u32, u32) {
        let mut ready = 7;
        let status = unsafe { self.ops.input_ready.unwrap()(&mut ready) };
        (self.note(2, status), ready)
    }
    fn control(&mut self, which: u32) -> (u32, u32) {
        let mut value = 7;
        let status = unsafe { self.ops.control_character.unwrap()(which, &mut value) };
        (self.note(3, status), value)
    }
}

/// What `cargo test` gives: usually no terminal at all. A stream that is one (a
/// run from a shell) is only asked for its size; its mode is left alone.
#[test]
fn streams_that_are_no_terminal() {
    let mut t = Terminal::new();
    for stream in 0..3u32 {
        let mut size = MaybeUninit::<libc::winsize>::zeroed();
        let answered = unsafe { libc::ioctl(stream as i32, libc::TIOCGWINSZ as _, size.as_mut_ptr()) } == 0;
        let size = unsafe { size.assume_init() };
        let expected = match (answered, size.ws_col, size.ws_row) {
            (false, _, _) => (if unsafe { libc::isatty(stream as i32) } == 1 { OS_ERROR } else { NOT_FOUND }, 0, 0),
            (true, 0, _) | (true, _, 0) => (UNSUPPORTED, 0, 0),
            (true, columns, rows) => (OK, columns as u32, rows as u32),
        };
        assert_eq!(t.size(stream), expected, "stream {stream}");
    }
    if unsafe { libc::isatty(0) } != 1 {
        assert_eq!(t.mode(1, 1, 0, 0), NOT_FOUND);
        assert_eq!(t.mode(0, 1, 0, 0), NOT_FOUND);
        assert_eq!(t.control(CONTROL_ERASE), (NOT_FOUND, 0));
    }
    assert_eq!(t.ready().0, OK);
    // Argument validation needs no terminal.
    assert_eq!(t.size(3), (INVALID_ARGUMENT, 0, 0));
    for (raw, min_bytes, timeout_ds, interrupt_as_input) in [(2, 1, 0, 0), (1, 256, 0, 0), (1, 1, 256, 0), (1, 1, 0, 2)] {
        assert_eq!(t.mode(raw, min_bytes, timeout_ds, interrupt_as_input), INVALID_ARGUMENT);
    }
    assert_eq!(t.control(0), (INVALID_ARGUMENT, 0));
    assert_eq!(t.control(5), (INVALID_ARGUMENT, 0));
    let (mut columns, mut rows, mut value) = (0, 0, 0);
    assert_eq!(unsafe { t.ops.window_size.unwrap()(0, ptr::null_mut(), &mut rows) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { t.ops.window_size.unwrap()(0, &mut columns, ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { t.ops.input_ready.unwrap()(ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { t.ops.control_character.unwrap()(CONTROL_ERASE, ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { t.ops.control_character.unwrap()(9, &mut value) }, INVALID_ARGUMENT);
    let mut stats = terminal::Stats::default();
    assert_eq!(unsafe { t.ops.read_stats.unwrap()(ptr::null_mut(), std::mem::size_of::<terminal::Stats>()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { t.ops.read_stats.unwrap()(&mut stats, std::mem::size_of::<terminal::Stats>() - 1) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { t.ops.read_stats.unwrap()(&mut stats, std::mem::size_of::<terminal::Stats>()) }, OK);
    assert!(stats.rejected_or_failed >= 12);
}

#[test]
fn against_a_pseudo_terminal() {
    if std::env::var_os(CHILD).is_some() { child(); }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "against_a_pseudo_terminal", "--test-threads=1"]).env(CHILD, "1").output().unwrap();
    assert!(output.status.success(), "{}\n{}{}", output.status, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

fn open_terminal() -> Option<(i32, i32)> {
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master < 0 || libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 { return None; }
        let name = libc::ptsname(master);
        if name.is_null() { return None; }
        let slave = libc::open(name, libc::O_RDWR | libc::O_NOCTTY);
        (slave >= 0).then_some((master, slave))
    }
}
fn resize(master: i32, columns: u16, rows: u16) {
    let size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    assert_eq!(unsafe { libc::ioctl(master, libc::TIOCSWINSZ as _, &size) }, 0);
}
fn settings_of(fd: i32) -> libc::termios {
    let mut settings = MaybeUninit::<libc::termios>::zeroed();
    assert_eq!(unsafe { libc::tcgetattr(fd, settings.as_mut_ptr()) }, 0);
    unsafe { settings.assume_init() }
}
/// macOS marks a terminal that returns to line mode with PENDIN until the next
/// read has gone over the input again; every other bit is compared.
const TRANSIENT: libc::tcflag_t = if cfg!(target_os = "macos") { libc::PENDIN } else { 0 };
type Fields = (libc::tcflag_t, libc::tcflag_t, libc::tcflag_t, libc::tcflag_t, [libc::cc_t; libc::NCCS], libc::speed_t, libc::speed_t, u8);
fn fields(settings: &libc::termios) -> Fields {
    #[cfg(target_os = "linux")]
    let line = settings.c_line;
    #[cfg(not(target_os = "linux"))]
    let line = 0;
    (settings.c_iflag, settings.c_oflag, settings.c_cflag, settings.c_lflag & !TRANSIENT, settings.c_cc, settings.c_ispeed, settings.c_ospeed, line)
}
/// The settings a mode must produce: always from the original ones, whatever the terminal went through in between.
fn expect_mode(slave: i32, original: &libc::termios, raw: bool, min_bytes: u8, timeout_ds: u8, interrupt_as_input: bool) {
    let mut expected = *original;
    if raw {
        expected.c_iflag &= !(libc::IXON | libc::IXOFF | libc::ICRNL | libc::INLCR | libc::IGNCR);
        expected.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN);
        expected.c_cc[libc::VMIN] = min_bytes;
        expected.c_cc[libc::VTIME] = timeout_ds;
    }
    if interrupt_as_input { expected.c_lflag &= !libc::ISIG; } else { expected.c_lflag |= libc::ISIG; }
    assert_eq!(fields(&settings_of(slave)), fields(&expected), "raw={raw} min={min_bytes} timeout={timeout_ds} interrupt={interrupt_as_input}");
}
fn waits(fd: i32, milliseconds: i32) -> i32 {
    let mut entry = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    loop {
        let count = unsafe { libc::poll(&mut entry, 1, milliseconds) };
        if count >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) { return count; }
    }
}
fn type_in(master: i32, text: &[u8]) { assert_eq!(unsafe { libc::write(master, text.as_ptr().cast(), text.len()) }, text.len() as isize); }
fn read_from(fd: i32) -> Vec<u8> {
    let mut bytes = [0u8; 16];
    let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
    assert!(count >= 0, "read: {}", std::io::Error::last_os_error());
    bytes[..count as usize].to_vec()
}
/// An echo reaches the master byte by byte: gather until the expected text is there.
fn expect_echo(master: i32, text: &[u8]) {
    let mut echoed = Vec::new();
    while echoed.len() < text.len() {
        assert_eq!(waits(master, 2000), 1, "echo so far: {echoed:?}");
        echoed.extend(read_from(master));
    }
    assert_eq!(echoed, text);
}
/// With input that is no terminal there is no mode to set and no editing character, but readiness still answers.
fn input_is_a_pipe(t: &mut Terminal, slave: i32) {
    let mut feed = [0i32; 2];
    assert!(unsafe { libc::pipe(feed.as_mut_ptr()) } == 0 && unsafe { libc::dup2(feed[0], 0) } == 0);
    assert_eq!(t.mode(1, 1, 0, 0), NOT_FOUND);
    assert_eq!(t.mode(0, 1, 0, 0), NOT_FOUND);
    assert_eq!(t.control(CONTROL_ERASE), (NOT_FOUND, 0));
    assert_eq!(t.size(0), (NOT_FOUND, 0, 0));
    assert_eq!(t.ready(), (OK, 0));
    type_in(feed[1], b"p");
    assert_eq!(t.ready(), (OK, 1));
    assert_eq!(unsafe { libc::dup2(slave, 0) }, 0);
    unsafe { libc::close(feed[0]); libc::close(feed[1]) };
}
/// A process group in the background of its controlling terminal is stopped by
/// SIGTTOU when it changes the terminal, as the first worker shows with a plain
/// tcsetattr; the same change through the boundary is stopped the same way and
/// leaves the terminal of the foreground job alone. The session gets a terminal of its own: on macOS the end of a session revokes its terminal
/// for every process that has it open. The forked processes report by exit code
/// and never panic. Returns zero when every step held.
fn background_group(t: &Terminal) -> i32 {
    unsafe {
        let leader = libc::fork();
        if leader < 0 { return 100; }
        if leader == 0 {
            libc::alarm(30);
            if libc::setsid() < 0 { libc::_exit(10); }
            let Some((_own_master, own)) = open_terminal() else { libc::_exit(11) };
            if libc::ioctl(own, libc::TIOCSCTTY as _, 0) != 0 || libc::dup2(own, 0) != 0 { libc::_exit(12); }
            let mut before = MaybeUninit::<libc::termios>::zeroed();
            if libc::tcgetattr(0, before.as_mut_ptr()) != 0 { libc::_exit(13); }
            let before = before.assume_init();
            for through_boundary in [false, true] {
                let worker = libc::fork();
                if worker < 0 { libc::_exit(14); }
                if worker == 0 {
                    libc::alarm(30);
                    if libc::setpgid(0, 0) != 0 { libc::_exit(2); }
                    if through_boundary { libc::_exit((t.ops.set_input_mode.unwrap()(1, 1, 0, 0) != OK) as i32); }
                    libc::_exit((libc::tcsetattr(0, libc::TCSANOW, &before) != 0) as i32);
                }
                let mut state = 0;
                if libc::waitpid(worker, &mut state, libc::WUNTRACED) != worker { libc::_exit(15); }
                if !libc::WIFSTOPPED(state) || libc::WSTOPSIG(state) != libc::SIGTTOU { libc::kill(worker, libc::SIGKILL); libc::_exit(if through_boundary { 16 } else { 17 }); }
                libc::kill(worker, libc::SIGKILL);
                libc::waitpid(worker, &mut state, 0);
            }
            let mut after = MaybeUninit::<libc::termios>::zeroed();
            if libc::tcgetattr(0, after.as_mut_ptr()) != 0 { libc::_exit(18); }
            let both = libc::ICANON | libc::ECHO;
            libc::_exit(if before.c_lflag & both == both && after.assume_init().c_lflag & both == both { 0 } else { 19 });
        }
        let mut state = 0;
        if libc::waitpid(leader, &mut state, 0) != leader || !libc::WIFEXITED(state) { return 101; }
        libc::WEXITSTATUS(state)
    }
}

fn child() -> ! {
    // Descriptors 1 and 2 become the terminal under test: a failure is reported on the stream the child was started with.
    let report = unsafe { libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3) };
    std::panic::set_hook(Box::new(move |info| {
        let text = format!("pseudo-terminal child: {info}\n");
        unsafe { libc::write(report, text.as_ptr().cast(), text.len()); libc::_exit(1) }
    }));
    unsafe { libc::alarm(60) };
    let (master, slave) = open_terminal().expect("no pseudo-terminal");
    resize(master, 132, 43);
    for fd in 0..3 { assert_eq!(unsafe { libc::dup2(slave, fd) }, fd); }
    let mut t = Terminal::new();

    // The window of each stream is the slave's, and follows it.
    for stream in 0..3 { assert_eq!(t.size(stream), (OK, 132, 43)); }
    resize(master, 80, 24);
    assert_eq!(t.size(1), (OK, 80, 24));
    assert_eq!(t.size(3), (INVALID_ARGUMENT, 0, 0));
    let mut redirected = [0i32; 2];
    assert!(unsafe { libc::pipe(redirected.as_mut_ptr()) } == 0 && unsafe { libc::dup2(redirected[1], 1) } == 1);
    assert_eq!(t.size(1), (NOT_FOUND, 0, 0));
    assert_eq!(t.size(0), (OK, 80, 24));
    assert_eq!(unsafe { libc::dup2(slave, 1) }, 1);
    unsafe { libc::close(redirected[0]); libc::close(redirected[1]) };
    // A terminal nobody gave a size reports none; the boundary has no cells to hand out.
    resize(master, 0, 0);
    assert_eq!(t.size(0), (UNSUPPORTED, 0, 0));
    resize(master, 132, 43);

    // The pseudo-terminal starts in line mode with echo and the interrupt key, which the checks below rely on.
    let original = settings_of(slave);
    let cooked = libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN;
    assert!(original.c_lflag & cooked == cooked && original.c_iflag & libc::ICRNL != 0);
    input_is_a_pipe(&mut t, slave);
    expect_mode(slave, &original, false, 0, 0, false);

    // Raw mode: a byte arrives as typed, at once, without a newline and without an echo.
    assert_eq!(t.mode(1, 1, 0, 0), OK);
    expect_mode(slave, &original, true, 1, 0, false);
    assert_eq!(t.ready(), (OK, 0));
    type_in(master, b"x");
    assert_eq!(waits(0, 2000), 1);
    assert_eq!(t.ready(), (OK, 1));
    assert_eq!(read_from(0), b"x");
    assert_eq!(t.ready(), (OK, 0));
    type_in(master, b"\r");
    assert_eq!(read_from(0), b"\r");
    assert_eq!(waits(master, 200), 0);
    // The interrupt key as a byte, and a read that gives up after two tenths of a second.
    assert_eq!(t.mode(1, 0, 2, 1), OK);
    expect_mode(slave, &original, true, 0, 2, true);
    let started = Instant::now();
    assert!(read_from(0).is_empty() && started.elapsed().as_millis() >= 150);
    type_in(master, b"\x03");
    assert_eq!(waits(0, 2000), 1);
    assert_eq!(read_from(0), b"\x03");
    // Line mode gives back the original settings, apart from the interrupt key while it stays a byte.
    assert_eq!(t.mode(0, 9, 9, 1), OK);
    expect_mode(slave, &original, false, 0, 0, true);
    assert_eq!(t.mode(0, 1, 0, 0), OK);
    assert_eq!(fields(&settings_of(slave)), fields(&original));
    type_in(master, b"ok\n");
    expect_echo(master, b"ok\r\n");
    assert_eq!(waits(0, 2000), 1);
    assert_eq!(t.ready(), (OK, 1));
    assert_eq!(read_from(0), b"ok\n");
    // Switching back and forth never drifts.
    for round in 0..64u32 {
        assert_eq!(t.mode(1, round % 7, round % 5, round & 1), OK);
        expect_mode(slave, &original, true, (round % 7) as u8, (round % 5) as u8, round & 1 != 0);
        if round % 3 == 0 { continue; } // raw mode on top of raw mode as well
        assert_eq!(t.mode(0, 1, 0, round & 1), OK);
        expect_mode(slave, &original, false, 0, 0, round & 1 != 0);
    }
    assert_eq!(t.mode(0, 1, 0, 0), OK);
    assert_eq!(fields(&settings_of(slave)), fields(&original));
    input_is_a_pipe(&mut t, slave);

    // The editing characters are the slave's; a disabled one is none.
    let mut edited = settings_of(slave);
    edited.c_cc[libc::VEOL] = libc::_POSIX_VDISABLE;
    edited.c_cc[libc::VEOL2] = 0x1d;
    assert_eq!(unsafe { libc::tcsetattr(slave, libc::TCSANOW, &edited) }, 0);
    assert!(edited.c_cc[libc::VERASE] != libc::_POSIX_VDISABLE && edited.c_cc[libc::VEOF] != libc::_POSIX_VDISABLE);
    assert_eq!(t.control(CONTROL_ERASE), (OK, edited.c_cc[libc::VERASE] as u32));
    assert_eq!(t.control(CONTROL_END_OF_FILE), (OK, edited.c_cc[libc::VEOF] as u32));
    assert_eq!(t.control(CONTROL_END_OF_LINE_2), (OK, 0x1d));
    assert_eq!(t.control(CONTROL_END_OF_LINE), (NOT_FOUND, 0));
    // A rejected call leaves the terminal alone.
    assert_eq!(t.mode(2, 1, 0, 0), INVALID_ARGUMENT);
    assert_eq!(t.mode(1, 1, 256, 1), INVALID_ARGUMENT);
    assert_eq!(fields(&settings_of(slave)), fields(&edited));

    assert_eq!(background_group(&t), 0);

    // A terminal that went away makes a read return at once, so input is ready.
    assert_eq!(t.mode(1, 1, 0, 0), OK);
    assert_eq!(t.ready(), (OK, 0));
    unsafe { libc::close(master) };
    assert_eq!(waits(0, 2000), 1);
    assert_eq!(t.ready(), (OK, 1));
    assert!(read_from(0).is_empty());

    let mut stats = terminal::Stats::default();
    assert_eq!(unsafe { t.ops.read_stats.unwrap()(&mut stats, std::mem::size_of::<terminal::Stats>()) }, OK);
    assert_eq!([stats.size_ok, stats.mode_ok, stats.ready_ok, stats.control_ok, stats.rejected_or_failed], t.seen);
    unsafe { libc::_exit(0) }
}
