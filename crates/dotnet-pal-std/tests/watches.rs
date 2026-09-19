//! Exercises the watches group of the std port through the negotiated C table:
//! inotify on Linux, the polling provider everywhere else. The two differ in what
//! they can see and in the order they tell it, so every step changes one thing,
//! waits for its event and tolerates others around it; what both guarantee (the
//! watch, the kind, the name, the halves of a rename next to each other with one
//! cookie) is asserted exactly.
use dotnet_pal_rs::io::{ACCESS_DENIED, NAME_TOO_LONG, NOT_DIRECTORY};
use dotnet_pal_rs::kernel::TIMEOUT;
use dotnet_pal_rs::runtime::NOT_FOUND;
use dotnet_pal_rs::watches::{self, Event, ACCESS, ATTRIBUTES, CREATE, DELETE, DIRECTORY, FOREVER, MODIFY, MOVED_FROM, MOVED_TO, NO_FOLLOW, ONLY_DIRECTORY, OVERFLOW, REMOVED};
use dotnet_pal_rs::{INVALID_ARGUMENT, OK, UNSUPPORTED};
use std::{ffi::c_void, fs, io::Write, mem::size_of, path::{Path, PathBuf}, ptr, sync::{atomic::{AtomicBool, AtomicU32, Ordering}, Mutex, MutexGuard}, thread, time::{Duration, Instant}};

const ALL: u32 = ACCESS | MODIFY | ATTRIBUTES | MOVED_FROM | MOVED_TO | CREATE | DELETE;
/// How long a change may take to be reported. The polling provider looks ten times a second.
const PATIENCE: Duration = Duration::from_secs(10);
/// Long enough for the polling provider to have looked several times.
const QUIET: Duration = Duration::from_millis(450);
/// One test at a time: the counters are the process's.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
fn ops() -> &'static watches::Ops {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    let api = unsafe { &*api };
    assert_eq!(api.header.capabilities & watches::CAP, watches::CAP);
    &api.watches
}
fn stats() -> watches::Stats {
    let mut out = watches::Stats::default();
    assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<watches::Stats>()) }, OK);
    out
}
fn counts(s: &watches::Stats) -> [u64; 6] { [s.open_ok, s.close_ok, s.add_ok, s.remove_ok, s.read_ok, s.rejected_or_failed] }
#[cfg(unix)]
fn raw(path: &Path) -> &[u8] { std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()) }
#[cfg(not(unix))]
fn raw(path: &Path) -> &[u8] { path.to_str().unwrap().as_bytes() }
fn name(event: &Event) -> &str { std::str::from_utf8(event.name()).unwrap() }
fn blank(event: &Event) -> bool { event.watch == 0 && event.events == 0 && event.cookie == 0 && event.name_length == 0 && event.name.iter().all(|byte| *byte == 0) }
#[cfg(unix)]
fn is_root() -> bool { unsafe { libc::geteuid() == 0 } }
#[cfg(not(unix))]
fn is_root() -> bool { false }
/// Changes the permissions to something they were not, whatever the umask made them.
#[cfg(unix)]
fn flip(path: &Path) { let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(path).unwrap().permissions()); chmod(path, (mode ^ 0o010) & 0o7777); }
#[cfg(unix)]
fn chmod(path: &Path, mode: u32) { fs::set_permissions(path, <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(mode)).unwrap(); }
#[cfg(not(unix))]
fn chmod(path: &Path, mode: u32) { let mut p = fs::metadata(path).unwrap().permissions(); p.set_readonly(mode & 0o200 == 0); fs::set_permissions(path, p).unwrap(); }
fn append(path: &Path, text: &str) { fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap(); }

/// A scratch directory that goes away with the test.
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("pal-std-watches-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn at(&self, relative: &str) -> PathBuf { self.0.join(relative) }
    fn dir(&self, relative: &str) -> PathBuf { let path = self.at(relative); fs::create_dir(&path).unwrap(); path }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fn open_up(path: &Path) { if let Ok(list) = fs::read_dir(path) { for entry in list.flatten() { if entry.file_type().is_ok_and(|kind| kind.is_dir()) { chmod(&entry.path(), 0o700); open_up(&entry.path()); } } } }
        open_up(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Watcher(*mut c_void);
unsafe impl Send for Watcher {}
unsafe impl Sync for Watcher {}
impl Watcher {
    fn open() -> Self {
        let mut handle = ptr::null_mut();
        assert_eq!(unsafe { ops().open.unwrap()(&mut handle) }, OK);
        assert!(!handle.is_null());
        Self(handle)
    }
    fn add(&self, path: &Path, events: u32) -> Result<u32, u32> {
        // The boundary takes a counted path: what follows it is not a terminator.
        let mut bytes = raw(path).to_vec();
        let length = bytes.len();
        bytes.extend_from_slice(b"XXXX");
        let mut id = 77;
        let status = unsafe { ops().add.unwrap()(self.0, bytes.as_ptr(), length, events, &mut id) };
        if status == OK { assert_ne!(id, 0); Ok(id) } else { assert_eq!(id, 0); Err(status) }
    }
    fn remove(&self, id: u32) -> u32 { unsafe { ops().remove.unwrap()(self.0, id) } }
    fn read(&self, timeout: Duration) -> Result<Event, u32> { self.read_ns(timeout.as_nanos() as u64) }
    fn read_ns(&self, timeout_ns: u64) -> Result<Event, u32> {
        let mut event = Event { watch: 0x5555_5555, events: 0x5555_5555, cookie: 0x5555_5555, name_length: 0x5555_5555, name: [0x55; 256] };
        let status = unsafe { ops().read.unwrap()(self.0, timeout_ns, &mut event, size_of::<Event>()) };
        if status != OK { assert!(blank(&event)); return Err(status); }
        // The name is a text of its length and nothing of an earlier event is left behind it.
        assert!(event.name().iter().all(|byte| *byte != 0 && *byte != b'/') && event.name[event.name_length as usize..].iter().all(|byte| *byte == 0));
        assert_eq!(event.cookie != 0, event.events & (MOVED_FROM | MOVED_TO) != 0, "a cookie belongs to a rename and to nothing else");
        Ok(event)
    }
    /// Reads until an event satisfies `wanted`; everything read comes back, that event last.
    fn until(&self, what: &str, wanted: impl Fn(&Event) -> bool) -> Vec<Event> {
        let (deadline, mut seen) = (Instant::now() + PATIENCE, Vec::new());
        loop {
            let left = deadline.checked_duration_since(Instant::now()).unwrap_or_else(|| panic!("no event for: {what}; seen {:?}", describe(&seen)));
            match self.read(left) {
                Ok(event) => { let found = wanted(&event); seen.push(event); if found { return seen; } }
                Err(status) => assert_eq!(status, TIMEOUT),
            }
        }
    }
    /// Everything that arrives within `QUIET`.
    fn rest(&self) -> Vec<Event> {
        let (deadline, mut seen) = (Instant::now() + QUIET, Vec::new());
        while let Some(left) = deadline.checked_duration_since(Instant::now()) { match self.read(left) { Ok(event) => seen.push(event), Err(status) => assert_eq!(status, TIMEOUT) } }
        seen
    }
    fn close(self) { assert_eq!(unsafe { ops().close.unwrap()(self.0) }, OK); }
}
fn describe(events: &[Event]) -> Vec<(u32, u32, u32, String)> { events.iter().map(|e| (e.watch, e.events, e.cookie, name(e).to_owned())).collect() }
fn is(event: &Event, watch: u32, events: u32, text: &str) -> bool { event.watch == watch && event.events == events && name(event) == text }
/// Every event is about one of the watches and one of the names, and nothing was lost.
fn only(seen: &[Event], watches: &[u32], names: &[&str]) {
    for event in seen { assert!(watches.contains(&event.watch) && names.contains(&name(event)) && event.events & OVERFLOW == 0, "unexpected {:?}", describe(seen)); }
}

#[test]
fn changes_in_a_watched_directory_are_reported() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let id = w.add(&scratch.0, ALL).unwrap();
    fs::write(scratch.at("f"), b"").unwrap();
    only(&w.until("create f", |e| is(e, id, CREATE, "f")), &[id], &["f"]);
    append(&scratch.at("f"), "hello");
    only(&w.until("modify f", |e| is(e, id, MODIFY, "f")), &[id], &["f"]);
    #[cfg(unix)]
    {
        flip(&scratch.at("f"));
        only(&w.until("attributes f", |e| is(e, id, ATTRIBUTES, "f")), &[id], &["f"]);
        // A rename is two events next to each other that share a cookie.
        fs::rename(scratch.at("f"), scratch.at("g")).unwrap();
        let seen = w.until("moved to g", |e| is(e, id, MOVED_TO, "g"));
        only(&seen, &[id], &["f", "g"]);
        let (from, to) = (&seen[seen.len() - 2], &seen[seen.len() - 1]);
        assert!(is(from, id, MOVED_FROM, "f") && from.cookie != 0 && from.cookie == to.cookie, "{:?}", describe(&seen));
    }
    #[cfg(not(unix))]
    { fs::rename(scratch.at("f"), scratch.at("g")).unwrap(); w.until("create g", |e| is(e, id, CREATE, "g")); }
    fs::create_dir(scratch.at("d")).unwrap();
    only(&w.until("create d", |e| is(e, id, CREATE | DIRECTORY, "d")), &[id], &["d", "g"]);
    fs::remove_dir(scratch.at("d")).unwrap();
    only(&w.until("delete d", |e| is(e, id, DELETE | DIRECTORY, "d")), &[id], &["d"]);
    fs::remove_file(scratch.at("g")).unwrap();
    only(&w.until("delete g", |e| is(e, id, DELETE, "g")), &[id], &["g"]);
    assert!(w.rest().is_empty());
    w.close();
}

#[cfg(unix)]
#[test]
fn a_rename_between_watched_directories_is_paired() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let (a, b, c) = (scratch.dir("a"), scratch.dir("b"), scratch.dir("c"));
    let (ia, ib) = (w.add(&a, ALL).unwrap(), w.add(&b, ALL).unwrap());
    assert_ne!(ia, ib);
    let mut cookies = Vec::new();
    // Often enough that a provider which lists one directory after the other gets a rename between two listings.
    for round in 0..8 {
        let (x, y) = (format!("x{round}"), format!("y{round}"));
        fs::write(a.join(&x), b"content").unwrap();
        w.until("create", |e| is(e, ia, CREATE, &x));
        if round % 2 == 1 { thread::sleep(Duration::from_millis(17 * round)); }
        fs::rename(a.join(&x), b.join(&y)).unwrap();
        let seen = w.until("moved to", |e| is(e, ib, MOVED_TO, &y));
        only(&seen, &[ia, ib], &[&x, &y]);
        // Each half goes to the watch of its directory, the second right behind the first.
        let (from, to) = (&seen[seen.len() - 2], &seen[seen.len() - 1]);
        assert!(is(from, ia, MOVED_FROM, &x) && from.cookie == to.cookie && !cookies.contains(&to.cookie), "{:?}", describe(&seen));
        cookies.push(to.cookie);
        // Out of sight, the rename is a loss to the directory it left: half a rename or a deletion.
        fs::rename(b.join(&y), c.join(&y)).unwrap();
        only(&w.until("left b", |e| e.watch == ib && name(e) == y && (e.events == MOVED_FROM || e.events == DELETE)), &[ib], &[&y]);
        fs::rename(c.join(&y), a.join(&y)).unwrap();
        only(&w.until("came to a", |e| e.watch == ia && name(e) == y && (e.events == MOVED_TO || e.events == CREATE)), &[ia], &[&y]);
    }
    // A directory moves like a file, and says what it is.
    fs::create_dir(a.join("d")).unwrap();
    w.until("create d", |e| is(e, ia, CREATE | DIRECTORY, "d"));
    fs::rename(a.join("d"), b.join("e")).unwrap();
    let seen = w.until("moved d", |e| is(e, ib, MOVED_TO | DIRECTORY, "e"));
    assert!(is(&seen[seen.len() - 2], ia, MOVED_FROM | DIRECTORY, "d") && seen[seen.len() - 2].cookie == seen[seen.len() - 1].cookie);
    w.close();
}

/// A provider that lists directories one after the other meets a rename between two listings the sooner the longer
/// a listing takes. Both halves still belong together.
#[cfg(unix)]
#[test]
fn renames_between_large_directories_stay_paired() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let dirs = [scratch.dir("a"), scratch.dir("b")];
    for dir in &dirs { for n in 0..2000 { fs::write(dir.join(format!("filler{n}")), b"").unwrap(); } }
    let ids = [w.add(&dirs[0], MOVED_FROM | MOVED_TO | CREATE | DELETE).unwrap(), w.add(&dirs[1], MOVED_FROM | MOVED_TO | CREATE | DELETE).unwrap()];
    fs::write(dirs[0].join("m0"), b"moving").unwrap();
    w.until("create m0", |e| is(e, ids[0], CREATE, "m0"));
    for round in 0..40 {
        let (from, to) = (round % 2, (round + 1) % 2);
        let (old, new) = (format!("m{round}"), format!("m{}", round + 1));
        // The polling provider looks again a tenth of a second after it reported: the renames are spread over the
        // time that look takes, a millisecond apart.
        thread::sleep(Duration::from_millis(85 + round as u64));
        fs::rename(dirs[from].join(&old), dirs[to].join(&new)).unwrap();
        let seen = w.until("moved", |e| e.watch == ids[to] && name(e) == new);
        assert_eq!(seen.len(), 2, "round {round}: {:?}", describe(&seen));
        assert!(is(&seen[0], ids[from], MOVED_FROM, &old) && is(&seen[1], ids[to], MOVED_TO, &new) && seen[0].cookie == seen[1].cookie, "round {round}: {:?}", describe(&seen));
    }
    w.close();
}

#[test]
fn what_happens_to_the_watched_node_has_no_name() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let dir = scratch.dir("dir");
    fs::write(scratch.at("file"), b"first").unwrap();
    let (d, f) = (w.add(&dir, ALL).unwrap(), w.add(&scratch.at("file"), MODIFY | ATTRIBUTES).unwrap());
    assert_ne!(d, f);
    #[cfg(unix)]
    {
        flip(&dir);
        only(&w.until("attributes of dir", |e| is(e, d, ATTRIBUTES | DIRECTORY, "")), &[d], &[""]);
    }
    append(&scratch.at("file"), " second");
    only(&w.until("modify file", |e| is(e, f, MODIFY, "")), &[f], &[""]);
    #[cfg(unix)]
    {
        flip(&scratch.at("file"));
        only(&w.until("attributes of file", |e| is(e, f, ATTRIBUTES, "")), &[f], &[""]);
    }
    // The loss of the node ends the watch: REMOVED is the last thing said about it, and the id names nothing afterwards.
    fs::remove_file(scratch.at("file")).unwrap();
    only(&w.until("file gone", |e| is(e, f, REMOVED, "")), &[f], &[""]);
    assert!(w.rest().is_empty());
    assert_eq!(w.remove(f), INVALID_ARGUMENT);
    w.close();
}

#[test]
fn one_node_is_one_watch() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let dir = scratch.dir("dir");
    fs::write(dir.join("k"), b"").unwrap();
    let id = w.add(&dir, ALL).unwrap();
    // The same path again keeps the id and takes the new events: the write is no longer reported, the entry is.
    assert_eq!(w.add(&dir, CREATE), Ok(id));
    append(&dir.join("k"), "unseen");
    thread::sleep(QUIET);
    fs::write(dir.join("n"), b"").unwrap();
    let mut seen = w.until("create n", |e| is(e, id, CREATE, "n"));
    seen.extend(w.rest());
    assert_eq!(describe(&seen), describe(&seen[seen.len() - 1..]), "only the entry that came is reported");
    // Another spelling of the path and the path with every link resolved lead to the same node. The temporary
    // directory of macOS is itself behind a link (/var, /tmp), so the two differ there.
    let resolved = fs::canonicalize(&dir).unwrap();
    assert_eq!(w.add(&resolved, ALL), Ok(id));
    assert_eq!(w.add(&dir.join("..").join("dir").join("."), ALL), Ok(id));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&dir, scratch.at("link")).unwrap();
        assert_eq!(w.add(&scratch.at("link"), ALL), Ok(id));
        assert_eq!(w.add(&scratch.at("link"), ALL | ONLY_DIRECTORY), Ok(id));
        // With NO_FOLLOW the link is a node of its own, and no directory.
        let link = w.add(&scratch.at("link"), ALL | NO_FOLLOW).unwrap();
        assert_ne!(link, id);
        assert_eq!(w.add(&scratch.at("link"), ALL | NO_FOLLOW | ONLY_DIRECTORY), Err(NOT_DIRECTORY));
        fs::write(dir.join("through"), b"").unwrap();
        only(&w.until("create through", |e| is(e, id, CREATE, "through")), &[id], &["through"]);
        fs::remove_file(scratch.at("link")).unwrap();
        only(&w.until("link gone", |e| is(e, link, REMOVED, "")), &[link], &[""]);
    }
    w.close();
}

#[test]
fn what_cannot_be_watched_is_refused() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    fs::write(scratch.at("file"), b"").unwrap();
    assert_eq!(w.add(&scratch.at("missing"), ALL), Err(NOT_FOUND));
    assert_eq!(w.add(&scratch.at("missing").join("below"), ALL), Err(NOT_FOUND));
    assert_eq!(w.add(&scratch.at("file"), ALL | ONLY_DIRECTORY), Err(NOT_DIRECTORY));
    if cfg!(unix) {
        assert_eq!(w.add(&scratch.at("file").join("below"), ALL), Err(NOT_DIRECTORY));
        assert_eq!(w.add(&scratch.at(&"n".repeat(300)), ALL), Err(NAME_TOO_LONG));
        let (locked, sealed) = (scratch.dir("locked"), scratch.at("sealed"));
        fs::write(&sealed, b"").unwrap();
        chmod(&locked, 0);
        chmod(&sealed, 0);
        if is_root() { eprintln!("watches: running as root, ACCESS_DENIED not checked"); } else {
            assert_eq!(w.add(&locked, ALL), Err(ACCESS_DENIED));
            assert_eq!(w.add(&sealed, ALL), Err(ACCESS_DENIED));
        }
        chmod(&locked, 0o700);
    }
    // A file is watched as readily as a directory when nobody insists on one.
    assert!(w.add(&scratch.at("file"), ALL).is_ok());
    // Reads are something inotify reports and directory snapshots cannot show: a watch for nothing else is refused there.
    let reads = w.add(&scratch.dir("read"), ACCESS | ONLY_DIRECTORY);
    if cfg!(target_os = "linux") { assert!(reads.is_ok()); } else { assert_eq!(reads, Err(UNSUPPORTED)); }
    w.close();
}

#[test]
fn a_read_waits_no_longer_than_asked() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let id = w.add(&scratch.0, CREATE | DELETE).unwrap();
    let begin = Instant::now();
    assert_eq!(w.read(Duration::ZERO).err(), Some(TIMEOUT));
    assert_eq!(w.read_ns(1).err(), Some(TIMEOUT));
    assert!(begin.elapsed() < Duration::from_millis(500));
    let begin = Instant::now();
    assert_eq!(w.read(Duration::from_millis(150)).err(), Some(TIMEOUT));
    assert!(begin.elapsed() >= Duration::from_millis(150) && begin.elapsed() < Duration::from_secs(2), "waited {:?}", begin.elapsed());
    // What is queued is taken without a wait, whatever the limit; a limit near the end of the range is one too.
    for (file, limit) in [("zero", 0), ("one", 1), ("forever", FOREVER), ("nearly", FOREVER - 1)] {
        fs::write(scratch.at(file), b"").unwrap();
        thread::sleep(QUIET);
        let queued = Instant::now();
        assert!(is(&w.read_ns(limit).unwrap(), id, CREATE, file) && queued.elapsed() < Duration::from_millis(500));
    }
    w.close();
}

#[test]
fn remove_ends_a_watch() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let (a, b) = (scratch.dir("a"), scratch.dir("b"));
    let (ia, ib) = (w.add(&a, ALL).unwrap(), w.add(&b, ALL).unwrap());
    assert_eq!(w.remove(ia), OK);
    let seen = w.until("removed", |e| is(e, ia, REMOVED, ""));
    assert_eq!(seen.len(), 1);
    // The id names nothing any more, the directory is silent, and the other watch is not.
    assert_eq!(w.remove(ia), INVALID_ARGUMENT);
    assert_eq!(w.remove(ia.max(ib) + 1000), INVALID_ARGUMENT);
    assert_eq!(w.remove(u32::MAX), INVALID_ARGUMENT);
    fs::write(a.join("silent"), b"").unwrap();
    fs::write(b.join("heard"), b"").unwrap();
    let mut seen = w.until("create heard", |e| is(e, ib, CREATE, "heard"));
    seen.extend(w.rest());
    only(&seen, &[ib], &["heard"]);
    // Watched again, the directory reports again.
    let again = w.add(&a, ALL).unwrap();
    fs::write(a.join("second"), b"").unwrap();
    only(&w.until("create second", |e| is(e, again, CREATE, "second")), &[again], &["second"]);
    w.close();
}

#[test]
fn a_deleted_directory_ends_its_watch() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let sub = scratch.dir("sub");
    fs::write(sub.join("child"), b"").unwrap();
    let (parent, id) = (w.add(&scratch.0, ALL).unwrap(), w.add(&sub, ALL).unwrap());
    fs::remove_file(sub.join("child")).unwrap();
    fs::remove_dir(&sub).unwrap();
    // The parent reports the entry and the watch of the directory ends, in the order the provider learns of them.
    let mut seen = w.until("first of two", |e| is(e, id, REMOVED, "") || is(e, parent, DELETE | DIRECTORY, "sub"));
    let first_was_end = seen.last().unwrap().events == REMOVED;
    seen.extend(w.until("second of two", |e| if first_was_end { is(e, parent, DELETE | DIRECTORY, "sub") } else { is(e, id, REMOVED, "") }));
    only(&seen, &[parent, id], &["", "sub", "child"]);
    assert!(seen.iter().any(|e| is(e, id, DELETE, "child")), "the entry that went first is reported to the watch that still stood: {:?}", describe(&seen));
    let end = seen.iter().position(|e| is(e, id, REMOVED, "")).unwrap();
    assert!(seen[end + 1..].iter().all(|e| e.watch != id), "nothing follows REMOVED: {:?}", describe(&seen));
    assert_eq!(w.remove(id), INVALID_ARGUMENT);
    assert_eq!(w.remove(parent), OK);
    w.close();
}

/// The two kinds of provider part ways here, and each says so in its documentation: inotify watches the node and goes
/// with it, the polling provider watches the path and ends the watch when the node has left it.
#[test]
fn a_renamed_directory_keeps_or_ends_its_watch() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let dir = scratch.dir("before");
    let id = w.add(&dir, CREATE | DELETE).unwrap();
    fs::rename(&dir, scratch.at("after")).unwrap();
    thread::sleep(QUIET);
    fs::write(scratch.at("after").join("x"), b"").unwrap();
    let seen = w.until("an event of the watch", |e| e.watch == id);
    if cfg!(any(target_os = "linux", target_os = "android")) { assert!(seen.len() == 1 && is(&seen[0], id, CREATE, "x"), "{:?}", describe(&seen)); assert_eq!(w.remove(id), OK); }
    else { assert!(seen.len() == 1 && is(&seen[0], id, REMOVED, ""), "{:?}", describe(&seen)); assert_eq!(w.remove(id), INVALID_ARGUMENT); }
    w.close();
}

#[test]
fn remove_releases_a_reader_that_waits() {
    let _serial = serial();
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let (a, b, c) = (scratch.dir("a"), scratch.dir("b"), scratch.dir("c"));
    let ia = w.add(&a, ALL).unwrap();
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        // The consumer's way to end a wait without a limit: another thread removes the watch.
        let reader = scope.spawn(|| { let event = w.read_ns(FOREVER); done.store(true, Ordering::Release); event });
        thread::sleep(Duration::from_millis(300));
        assert!(!done.load(Ordering::Acquire));
        let begin = Instant::now();
        assert_eq!(w.remove(ia), OK);
        let event = reader.join().unwrap().unwrap();
        assert!(is(&event, ia, REMOVED, "") && begin.elapsed() < Duration::from_secs(2));
    });
    thread::scope(|scope| {
        // A watch added beside the wait reports to it.
        done.store(false, Ordering::Release);
        let reader = scope.spawn(|| { let event = w.read_ns(FOREVER); done.store(true, Ordering::Release); event });
        thread::sleep(Duration::from_millis(200));
        let ib = w.add(&b, CREATE).unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!(!done.load(Ordering::Acquire));
        fs::write(b.join("woken"), b"").unwrap();
        assert!(is(&reader.join().unwrap().unwrap(), ib, CREATE, "woken"));
    });
    thread::scope(|scope| {
        // The consumer's shutdown: a reader that goes on until every watch has ended, and another thread that ends them.
        let ids = [w.add(&a, ALL).unwrap(), w.add(&b, ALL).unwrap(), w.add(&c, ALL).unwrap()];
        let reader = scope.spawn(|| { let mut ended = Vec::new(); while ended.len() < 3 { let event = w.read_ns(FOREVER).unwrap(); if event.events & REMOVED != 0 { ended.push(event.watch); } } ended });
        thread::sleep(Duration::from_millis(200));
        let begin = Instant::now();
        for id in ids { assert_eq!(w.remove(id), OK); }
        let mut ended = reader.join().unwrap();
        ended.sort_unstable();
        let mut expected = ids.to_vec();
        expected.sort_unstable();
        assert!(ended == expected && begin.elapsed() < Duration::from_secs(2));
    });
    w.close();
}

#[test]
fn a_reader_that_does_not_keep_up_is_told() {
    let _serial = serial();
    // The queue of the polling provider holds 4096 events; the kernel says what inotify's holds.
    let limit: usize = if cfg!(any(target_os = "linux", target_os = "android")) { fs::read_to_string("/proc/sys/fs/inotify/max_queued_events").unwrap().trim().parse().unwrap() } else { 4096 };
    if limit > 100_000 { eprintln!("watches: a queue of {limit} events is not cheap to fill, OVERFLOW not checked"); return; }
    let (scratch, w) = (Scratch::new(), Watcher::open());
    let id = w.add(&scratch.0, CREATE).unwrap();
    for n in 0..limit + 300 { fs::write(scratch.at(&format!("o{n}")), b"").unwrap(); }
    // Nothing is read before the provider has seen it all: the queue has to fill, not the reader to fall behind by chance.
    if !cfg!(any(target_os = "linux", target_os = "android")) { thread::sleep(Duration::from_millis(1500)); }
    let mut taken = 0;
    let lost = loop {
        let event = w.read(PATIENCE).unwrap();
        if event.events & OVERFLOW != 0 { break event; }
        assert!(event.watch == id && event.events == CREATE && name(&event).starts_with('o'));
        taken += 1;
    };
    // The report names no watch and no entry, and it comes where the loss happened: after exactly what the queue holds.
    assert!(lost.watch == 0 && lost.events == OVERFLOW && lost.cookie == 0 && lost.name_length == 0 && taken == limit, "taken {taken} of {limit}");
    // The watch has not gone anywhere.
    thread::sleep(QUIET);
    fs::write(scratch.at("after"), b"").unwrap();
    w.until("create after", |e| is(e, id, CREATE, "after"));
    w.close();
}

#[test]
fn two_watchers_are_independent() {
    let _serial = serial();
    let (scratch, one, two) = (Scratch::new(), Watcher::open(), Watcher::open());
    assert_ne!(one.0, two.0);
    let (a, b) = (scratch.dir("a"), scratch.dir("b"));
    let (a1, b1, a2) = (one.add(&a, ALL).unwrap(), one.add(&b, CREATE).unwrap(), two.add(&a, DELETE).unwrap());
    fs::write(a.join("p"), b"").unwrap();
    only(&one.until("create p", |e| is(e, a1, CREATE, "p")), &[a1], &["p"]);
    // Each polling watcher looks on its own schedule: the other one has to have seen the file before it can miss it.
    thread::sleep(QUIET);
    fs::remove_file(a.join("p")).unwrap();
    only(&one.until("delete p", |e| is(e, a1, DELETE, "p")), &[a1], &["p"]);
    assert_eq!(describe(&two.until("delete p", |e| is(e, a2, DELETE, "p"))).len(), 1);
    // An id belongs to its watcher, and ending a watch or closing a watcher leaves the other watcher alone.
    if b1 != a2 { assert_eq!(two.remove(b1), INVALID_ARGUMENT); }
    assert_eq!(one.remove(a1), OK);
    assert!(is(&one.until("removed", |e| e.events == REMOVED)[0], a1, REMOVED, ""));
    fs::write(a.join("r"), b"").unwrap();
    thread::sleep(QUIET);
    fs::remove_file(a.join("r")).unwrap();
    assert_eq!(describe(&two.until("delete r", |e| is(e, a2, DELETE, "r"))).len(), 1);
    assert!(one.rest().is_empty());
    one.close();
    fs::write(a.join("s"), b"").unwrap();
    thread::sleep(QUIET);
    fs::remove_file(a.join("s")).unwrap();
    assert_eq!(describe(&two.until("delete s", |e| is(e, a2, DELETE, "s"))).len(), 1);
    two.close();
}

#[test]
fn watchers_come_and_go() {
    let _serial = serial();
    let scratch = Scratch::new();
    let before = counts(&stats());
    for round in 0..40 {
        let w = Watcher::open();
        if round % 2 == 0 { w.add(&scratch.0, ALL).unwrap(); fs::write(scratch.at(&format!("f{round}")), b"").unwrap(); }
        // Closed with a watch in place, with events nobody read, and while the polling provider is at work.
        if round % 4 == 0 { thread::sleep(Duration::from_millis(120)); }
        let begin = Instant::now();
        w.close();
        assert!(begin.elapsed() < Duration::from_secs(2));
    }
    let after = counts(&stats());
    assert_eq!([after[0] - before[0], after[1] - before[1], after[2] - before[2], after[5] - before[5]], [40, 40, 20, 0]);
}

#[test]
fn invalid_arguments_are_rejected_and_counted() {
    let _serial = serial();
    let (scratch, w, o) = (Scratch::new(), Watcher::open(), ops());
    let id = w.add(&scratch.0, ALL).unwrap();
    let path = raw(&scratch.0);
    let before = counts(&stats());
    let mut handles = [ptr::null_mut::<c_void>(); 2];
    let mut events = [Event::EMPTY; 2];
    let mut out = 77u32;
    unsafe {
        assert_eq!(o.open.unwrap()(ptr::null_mut()), INVALID_ARGUMENT);
        assert_eq!(o.open.unwrap()(handles.as_mut_ptr().cast::<u8>().add(1).cast()), INVALID_ARGUMENT);
        assert_eq!(o.close.unwrap()(ptr::null_mut()), INVALID_ARGUMENT);
        let add = o.add.unwrap();
        let lengthy = vec![b'n'; 4096];
        let mut refused = |watcher: *mut c_void, data: *const u8, length: usize, kinds: u32| { out = 77; assert_eq!(add(watcher, data, length, kinds, &mut out), INVALID_ARGUMENT); assert_eq!(out, 0); };
        refused(ptr::null_mut(), path.as_ptr(), path.len(), ALL);
        refused(w.0, ptr::null(), path.len(), ALL);
        refused(w.0, path.as_ptr(), 0, ALL);
        refused(w.0, lengthy.as_ptr(), lengthy.len(), ALL);
        refused(w.0, b"/tmp\0x".as_ptr(), 6, ALL);
        // Nothing to report, only a way to look, and the kinds that are reported but never asked for.
        for kinds in [0, ONLY_DIRECTORY, NO_FOLLOW, ONLY_DIRECTORY | NO_FOLLOW, ALL | OVERFLOW, ALL | REMOVED, ALL | DIRECTORY, ALL | 4096, ALL | 0x8000_0000] { refused(w.0, path.as_ptr(), path.len(), kinds); }
        assert_eq!(add(w.0, path.as_ptr(), path.len(), ALL, ptr::null_mut()), INVALID_ARGUMENT);
        assert_eq!(add(w.0, path.as_ptr(), path.len(), ALL, events.as_mut_ptr().cast::<u8>().add(1).cast()), INVALID_ARGUMENT);
        assert_eq!(o.remove.unwrap()(ptr::null_mut(), id), INVALID_ARGUMENT);
        assert_eq!(o.remove.unwrap()(w.0, 0), INVALID_ARGUMENT);
        let read = o.read.unwrap();
        events[0] = Event { watch: 5, events: 5, cookie: 5, name_length: 5, name: [5; 256] };
        assert_eq!(read(ptr::null_mut(), 0, events.as_mut_ptr(), size_of::<Event>()), INVALID_ARGUMENT);
        assert!(blank(&events[0]));
        assert_eq!(read(w.0, 0, ptr::null_mut(), size_of::<Event>()), INVALID_ARGUMENT);
        assert_eq!(read(w.0, 0, events.as_mut_ptr().cast::<u8>().add(1).cast(), size_of::<Event>()), INVALID_ARGUMENT);
        events[0].watch = 5;
        assert_eq!(read(w.0, 0, events.as_mut_ptr(), size_of::<Event>() - 1), INVALID_ARGUMENT);
        assert_eq!(events[0].watch, 5, "too small to write to");
        let mut s = watches::Stats::default();
        assert_eq!(o.read_stats.unwrap()(ptr::null_mut(), size_of::<watches::Stats>()), INVALID_ARGUMENT);
        assert_eq!(o.read_stats.unwrap()(&mut s, size_of::<watches::Stats>() - 1), INVALID_ARGUMENT);
    }
    let after = counts(&stats());
    assert_eq!(after[5] - before[5], 25);
    assert_eq!(after[..5], before[..5]);
    // None of it reached the watcher: it has its one watch and nothing queued.
    assert!(w.rest().is_empty());
    assert_eq!(w.remove(id), OK);
    w.close();
}
