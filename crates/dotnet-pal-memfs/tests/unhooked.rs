//! What a reading watcher does on a port that set no wait function. A wait function cannot be taken back, and
//! neither can a clock, so these steps have a process of their own and are one test: each builds on the hooks of
//! the one before.
use dotnet_pal_memfs::{set_clock, set_yield, MemFs};
use dotnet_pal_rs::files::{CREATE, WRITE};
use dotnet_pal_rs::port::{Error, Files, Result, Watches};
use dotnet_pal_rs::watches::{self, Event, FOREVER, REMOVED};
use std::{ffi::c_void, sync::atomic::{AtomicU64, Ordering}, thread};

static YIELDS: AtomicU64 = AtomicU64::new(0);
static NOW: AtomicU64 = AtomicU64::new(1_000_000_000);
const HOUR: u64 = 3_600_000_000_000;

fn touch(path: &str) {
    let file = unsafe { <MemFs as Files>::open(path.as_bytes(), WRITE | CREATE, 0o644) }.unwrap();
    assert_eq!(unsafe { <MemFs as Files>::close(file) }, Ok(()));
}
fn await_event(watcher: *mut c_void, timeout_ns: u64) -> Result<(u32, u32, String)> {
    let mut event = Event::EMPTY;
    unsafe { <MemFs as Watches>::read(watcher, timeout_ns, &mut event) }?;
    Ok((event.watch, event.events, String::from_utf8(event.name().to_vec()).unwrap()))
}
struct Sent(*mut c_void);
// SAFETY: a watcher is read by one thread while others add and remove, which is what the test does.
unsafe impl Send for Sent {}
impl Sent { fn handle(&self) -> *mut c_void { self.0 } }
/// Reads on a thread of its own and returns once that thread has yielded: the read found nothing and waits.
fn reader(watcher: *mut c_void, timeout_ns: u64) -> thread::JoinHandle<Result<(u32, u32, String)>> {
    let (sent, before) = (Sent(watcher), YIELDS.load(Ordering::SeqCst));
    let reader = thread::spawn(move || await_event(sent.handle(), timeout_ns));
    while YIELDS.load(Ordering::SeqCst) == before { thread::yield_now(); }
    reader
}

#[test]
fn a_read_without_a_wait_function_yields_and_needs_a_clock_to_time_out() {
    set_yield(|| { YIELDS.fetch_add(1, Ordering::SeqCst); thread::yield_now(); });
    assert_eq!(unsafe { MemFs::create_directory(b"/unhooked", 0o755) }, Ok(()));
    let watcher = <MemFs as Watches>::open().unwrap();
    let id = unsafe { <MemFs as Watches>::add(watcher, b"/unhooked", watches::CREATE) }.unwrap();

    // Without a clock there is no telling when the time is up: a timed read takes what is there and does not wait.
    for timeout_ns in [0, 1, HOUR, FOREVER - 1] { assert_eq!(await_event(watcher, timeout_ns), Err(Error::Timeout)); }
    touch("/unhooked/first");
    assert_eq!(await_event(watcher, HOUR), Ok((id, watches::CREATE, "first".to_string())));
    assert_eq!(YIELDS.load(Ordering::SeqCst), 0);

    // A read without limit needs no clock. It yields until another thread causes an event or ends the watch.
    let waiting = reader(watcher, FOREVER);
    touch("/unhooked/second");
    assert_eq!(waiting.join().unwrap(), Ok((id, watches::CREATE, "second".to_string())));
    let waiting = reader(watcher, FOREVER);
    assert_eq!(unsafe { <MemFs as Watches>::remove(watcher, id) }, Ok(()));
    assert_eq!(waiting.join().unwrap(), Ok((id, REMOVED, String::new())));

    // With a clock a timed read yields until the clock says the time is up. This one moves 1 ms every time it is
    // asked: a read of 5 ms asks it six times and yields five times.
    set_clock(|| NOW.fetch_add(1_000_000, Ordering::SeqCst));
    let before = YIELDS.load(Ordering::SeqCst);
    assert_eq!(await_event(watcher, 5_000_000), Err(Error::Timeout));
    assert_eq!(YIELDS.load(Ordering::SeqCst) - before, 5);
    assert_eq!(await_event(watcher, 0), Err(Error::Timeout));
    assert_eq!(YIELDS.load(Ordering::SeqCst) - before, 5);
    let id = unsafe { <MemFs as Watches>::add(watcher, b"/unhooked", watches::CREATE) }.unwrap();
    let waiting = reader(watcher, 1000 * HOUR);
    touch("/unhooked/third");
    assert_eq!(waiting.join().unwrap(), Ok((id, watches::CREATE, "third".to_string())));
    assert_eq!(unsafe { <MemFs as Watches>::close(watcher) }, Ok(()));
}
