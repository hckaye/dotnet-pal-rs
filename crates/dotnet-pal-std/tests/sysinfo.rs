//! Exercises the system information group of the std port through the negotiated
//! C table, on whatever desktop OS runs the test.
use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
use dotnet_pal_rs::system::{self, MAX_ENVIRONMENT_ENTRY, TEXT_EXECUTABLE_PATH, TEXT_HOME_DIRECTORY, TEXT_OS_NAME, TEXT_OS_RELEASE, TEXT_OS_VERSION, TEXT_USER_NAME};
use dotnet_pal_rs::{INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use std::{ffi::OsStr, mem::size_of, ptr, sync::{Mutex, MutexGuard}, time::{Duration, Instant}};

const CAPACITY: usize = MAX_ENVIRONMENT_ENTRY;
/// One test at a time: one of them changes the environment, and the counters are the process's.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
fn ops() -> &'static system::Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & system::CAP, system::CAP);
    &api.system
}
/// Status, needed length and the whole buffer of one call; no buffer at all for capacity 0.
fn call(capacity: usize, ask: impl FnOnce(*mut u8, *mut usize) -> u32) -> (u32, usize, Vec<u8>) {
    let (mut needed, mut buffer) = (usize::MAX, vec![0xAAu8; capacity]);
    let status = ask(if capacity == 0 { ptr::null_mut() } else { buffer.as_mut_ptr() }, &mut needed);
    (status, needed, buffer)
}
fn entry(index: usize, capacity: usize) -> (u32, usize, Vec<u8>) { call(capacity, |out, needed| unsafe { ops().environment_entry.unwrap()(index, out, capacity, needed) }) }
fn text(what: u32, capacity: usize) -> (u32, usize, Vec<u8>) { call(capacity, |out, needed| unsafe { ops().text.unwrap()(what, out, capacity, needed) }) }
/// The text, its NUL and a cleared rest.
fn padded(text: &[u8], capacity: usize) -> Vec<u8> { let mut all = text.to_vec(); all.resize(capacity, 0); all }
/// A call delivers `expected` whole where it fits and its length alone, with a cleared buffer, where it does not.
fn delivers(ask: impl Fn(usize) -> (u32, usize, Vec<u8>), expected: &[u8]) {
    let needed = expected.len() + 1;
    assert_eq!(ask(CAPACITY), (OK, needed, padded(expected, CAPACITY)));
    assert_eq!(ask(needed), (OK, needed, padded(expected, needed)));
    assert_eq!(ask(needed - 1), (BUFFER_TOO_SMALL, needed, vec![0; needed - 1]));
    assert_eq!(ask(0), (BUFFER_TOO_SMALL, needed, Vec::new()));
}
#[cfg(unix)]
fn raw(text: &OsStr) -> Option<&[u8]> { Some(std::os::unix::ffi::OsStrExt::as_bytes(text)) }
#[cfg(not(unix))]
fn raw(text: &OsStr) -> Option<&[u8]> { text.to_str().map(str::as_bytes) }
fn stats() -> system::Stats {
    let mut out = system::Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<system::Stats>()) }, OK);
    out
}

#[test]
fn the_environment_enumerates_as_std_reads_it() {
    let _serial = serial();
    std::env::set_var("PAL_STD_SYSINFO_PLAIN", "value");
    std::env::set_var("PAL_STD_SYSINFO_EMPTY", "");
    std::env::set_var("PAL_STD_SYSINFO_EQUALS", "a=b=");
    std::env::set_var("PAL_STD_SYSINFO_LONG", "v".repeat(3000));
    std::env::set_var("PAL_STD_SYSINFO_OVERSIZED", "o".repeat(MAX_ENVIRONMENT_ENTRY));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::env::set_var("PAL_STD_SYSINFO_BYTES", OsStr::from_bytes(b"\xff\xfe not text"));
        // A string std reads as a variable named "=PAL_STD_SYSINFO_ODD", where the C library takes it (macOS refuses).
        let taken = unsafe { libc::putenv(c"=PAL_STD_SYSINFO_ODD=x".as_ptr().cast_mut()) } == 0;
        assert_eq!(taken, std::env::vars_os().any(|(name, _)| name == "=PAL_STD_SYSINFO_ODD"));
    }
    // The variables std lists, in its order, without those the boundary cannot carry: a name with '=', and on Windows text that is not Unicode.
    let expected: Vec<Vec<u8>> = std::env::vars_os().filter_map(|(name, value)| {
        let (name, value) = (raw(&name)?, raw(&value)?);
        (!name.is_empty() && !name.contains(&b'=')).then(|| [name, b"=", value].concat())
    }).collect();
    for own in ["PAL_STD_SYSINFO_PLAIN=value", "PAL_STD_SYSINFO_EQUALS=a=b="] { assert!(expected.iter().any(|text| text == own.as_bytes())); }
    assert_eq!(expected.iter().any(|text| text == b"PAL_STD_SYSINFO_EMPTY="), cfg!(unix), "Windows has no empty variable");
    #[cfg(unix)]
    assert!(expected.iter().any(|text| text == b"PAL_STD_SYSINFO_BYTES=\xff\xfe not text"));
    let before = stats();
    let mut too_long = 0;
    for (index, variable) in expected.iter().enumerate() {
        assert!(variable[0] != b'=');
        // An entry longer than the boundary carries keeps its index and is refused.
        if variable.len() >= MAX_ENVIRONMENT_ENTRY { assert_eq!(entry(index, CAPACITY), (OS_ERROR, 0, vec![0; CAPACITY])); too_long += 1; continue; }
        delivers(|capacity| entry(index, capacity), variable);
    }
    assert!(too_long >= 1);
    for index in [expected.len(), expected.len() + 1, usize::MAX] { assert_eq!(entry(index, CAPACITY), (NOT_FOUND, 0, vec![0; CAPACITY])); }
    let mut needed = 7usize;
    assert_eq!(unsafe { ops().environment_entry.unwrap()(0, ptr::null_mut(), 8, &mut needed) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { ops().environment_entry.unwrap()(0, [0u8; 8].as_mut_ptr(), 8, ptr::null_mut()) }, INVALID_ARGUMENT);
    let after = stats();
    let delivered = (expected.len() - too_long) as u64;
    assert_eq!(after.environment_ok, before.environment_ok + 2 * delivered);
    assert_eq!(after.rejected_or_failed, before.rejected_or_failed + 2 * delivered + too_long as u64 + 5);
    assert_eq!((after.text_ok, after.times_ok, after.identity_ok), (before.text_ok, before.times_ok, before.identity_ok));
}

#[test]
fn texts_describe_this_process_system_and_user() {
    let _serial = serial();
    let executable = std::env::current_exe().unwrap();
    #[cfg(unix)]
    let executable = executable.canonicalize().unwrap();
    delivers(|capacity| text(TEXT_EXECUTABLE_PATH, capacity), raw(executable.as_os_str()).unwrap());
    let (status, needed, name) = text(TEXT_OS_NAME, CAPACITY);
    assert_eq!(status, OK);
    let name = &name[..needed - 1];
    if cfg!(target_os = "linux") { assert_eq!(name, b"Linux"); }
    if cfg!(target_os = "macos") { assert_eq!(name, b"Darwin"); }
    if cfg!(windows) { assert_eq!(name, b"Windows"); }
    delivers(|capacity| text(TEXT_OS_NAME, capacity), name);
    for what in [TEXT_OS_RELEASE, TEXT_OS_VERSION] {
        let (status, needed, value) = text(what, CAPACITY);
        if cfg!(unix) {
            assert_eq!(status, OK);
            assert!(needed >= 2 && value[..needed - 1].iter().all(|byte| *byte != 0) && value[needed - 1..].iter().all(|byte| *byte == 0));
        } else { assert_eq!((status, needed, value), (UNSUPPORTED, 0, vec![0; CAPACITY])); }
    }
    // The user: the passwd entry of the effective user id, which a process may lack (a container started under a bare number).
    #[cfg(unix)]
    {
        let entry = unsafe { libc::getpwuid(libc::geteuid()) };
        if entry.is_null() {
            for what in [TEXT_USER_NAME, TEXT_HOME_DIRECTORY] { assert_eq!(text(what, CAPACITY), (UNSUPPORTED, 0, vec![0; CAPACITY])); }
        } else {
            let (user, home) = unsafe { (std::ffi::CStr::from_ptr((*entry).pw_name).to_bytes().to_vec(), std::ffi::CStr::from_ptr((*entry).pw_dir).to_bytes().to_vec()) };
            assert!(!user.is_empty());
            delivers(|capacity| text(TEXT_USER_NAME, capacity), &user);
            delivers(|capacity| text(TEXT_HOME_DIRECTORY, capacity), &home);
        }
    }
    #[cfg(windows)]
    for (what, variable) in [(TEXT_USER_NAME, "USERNAME"), (TEXT_HOME_DIRECTORY, "USERPROFILE")] {
        match std::env::var(variable) {
            Ok(value) => delivers(|capacity| text(what, capacity), value.as_bytes()),
            Err(_) => assert_ne!(text(what, CAPACITY).0, OK),
        }
    }
    for what in [0, TEXT_HOME_DIRECTORY + 1, u32::MAX] { assert_eq!(text(what, CAPACITY).0, INVALID_ARGUMENT); assert_eq!(text(what, CAPACITY).1, 0); }
    let mut needed = 7usize;
    assert_eq!(unsafe { ops().text.unwrap()(TEXT_OS_NAME, ptr::null_mut(), 8, &mut needed) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { ops().text.unwrap()(TEXT_OS_NAME, [0u8; 8].as_mut_ptr(), 8, ptr::null_mut()) }, INVALID_ARGUMENT);
}

#[test]
fn cpu_time_uptime_and_ids_follow_the_process() {
    let _serial = serial();
    let s = ops();
    let before = stats();
    let times = || { let (mut user, mut kernel) = (7u64, 7u64); assert_eq!(unsafe { s.process_times.unwrap()(&mut user, &mut kernel) }, OK); (user, kernel) };
    #[cfg(unix)]
    let usage = || {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        let ns = |time: libc::timeval| time.tv_sec as u64 * 1_000_000_000 + time.tv_usec as u64 * 1000;
        (ns(usage.ru_utime), ns(usage.ru_stime))
    };
    #[cfg(unix)]
    {
        let (earlier, (user, kernel), later) = (usage(), times(), usage());
        assert!(earlier.0 <= user && user <= later.0 && earlier.1 <= kernel && kernel <= later.1, "{earlier:?} {user} {kernel} {later:?}");
    }
    let (user, kernel) = times();
    let started = Instant::now();
    let mut sum = 0u64;
    while started.elapsed() < Duration::from_millis(300) { for i in 0..100_000u64 { sum = std::hint::black_box(sum.wrapping_add(i * i)); } }
    let (user_later, kernel_later) = times();
    assert!(user_later > user && kernel_later >= kernel, "{user} {kernel} then {user_later} {kernel_later}");
    assert!(user_later - user >= 100_000_000, "a busy thread was on a CPU for {} ns of 300 ms", user_later - user);

    let uptime = || { let mut value = 7u64; assert_eq!(unsafe { s.uptime_ns.unwrap()(&mut value) }, OK); value };
    let first = uptime();
    std::thread::sleep(Duration::from_millis(50));
    let second = uptime();
    assert!(first > 0 && second >= first + 30_000_000, "{first} then {second}");
    #[cfg(target_os = "linux")]
    {
        let boottime = || { let mut value: libc::timespec = unsafe { std::mem::zeroed() }; assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) }, 0); value.tv_sec as u64 * 1_000_000_000 + value.tv_nsec as u64 };
        let (earlier, value, later) = (boottime(), uptime(), boottime());
        assert!(earlier <= value && value <= later);
    }

    let (mut user_id, mut group_id) = (7u32, 7u32);
    let status = unsafe { s.user_ids.unwrap()(&mut user_id, &mut group_id) };
    #[cfg(unix)]
    assert_eq!((status, user_id, group_id), (OK, unsafe { libc::geteuid() }, unsafe { libc::getegid() }));
    #[cfg(not(unix))]
    assert_eq!((status, user_id, group_id), (UNSUPPORTED, 0, 0));

    let mut value = 0u64;
    assert_eq!(unsafe { s.process_times.unwrap()(ptr::null_mut(), &mut value) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.process_times.unwrap()(&mut value, ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.uptime_ns.unwrap()(ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.user_ids.unwrap()(ptr::null_mut(), &mut group_id) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.user_ids.unwrap()(&mut user_id, ptr::null_mut()) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.read_stats.unwrap()(ptr::null_mut(), size_of::<system::Stats>()) }, INVALID_ARGUMENT);
    let after = stats();
    assert_eq!(after.times_ok, before.times_ok + if cfg!(unix) { 5 } else { 4 } + if cfg!(target_os = "linux") { 1 } else { 0 });
    assert_eq!(after.identity_ok, before.identity_ok + if cfg!(unix) { 1 } else { 0 });
    assert_eq!(after.rejected_or_failed, before.rejected_or_failed + if cfg!(unix) { 5 } else { 6 });
}
