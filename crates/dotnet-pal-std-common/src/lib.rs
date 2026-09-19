#![no_std]
//! Internal source-sharing for the desktop ports. This crate has no OS APIs,
//! platform selection, dependencies or exported PAL entry point.
//! The macro implements portable services on the platform's own provider type.
#[doc(hidden)]
#[macro_export]
macro_rules! implement {
    ($provider:ident) => {
fn boxed<T>(value: T) -> *mut c_void { Box::into_raw(Box::new(value)).cast() }
unsafe fn borrow<'a, T>(handle: *mut c_void) -> Result<&'a T> {
    if handle.is_null() || handle as usize % std::mem::align_of::<T>() != 0 { return Err(Error::InvalidArgument); }
    Ok(unsafe { &*handle.cast::<T>() })
}
unsafe fn take<T>(handle: *mut c_void) -> Result<Box<T>> {
    if handle.is_null() || handle as usize % std::mem::align_of::<T>() != 0 { return Err(Error::InvalidArgument); }
    Ok(unsafe { Box::from_raw(handle.cast::<T>()) })
}

impl port::Abort for $provider { fn abort() -> ! { std::process::abort() } }

impl port::Clock for $provider {
    fn monotonic_ns() -> Result<u64> {
        static START: OnceLock<Instant> = OnceLock::new();
        let start = *START.get_or_init(Instant::now);
        u64::try_from(start.elapsed().as_nanos()).map_err(|_| Error::Os)
    }
}
impl port::Scheduler for $provider {
    fn sleep_ns(nanoseconds: u64) -> Result<()> { std::thread::sleep(Duration::from_nanos(nanoseconds)); Ok(()) }
    fn yield_now() -> Result<()> { std::thread::yield_now(); Ok(()) }
}
impl port::Realtime for $provider {
    fn realtime_ns() -> Result<u64> {
        let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_err(|_| Error::Os)?;
        u64::try_from(since.as_nanos()).map_err(|_| Error::Os)
    }
}
impl port::Entropy for $provider {
    unsafe fn fill(out: *mut u8, size: usize) -> Result<()> {
        let buffer = unsafe { std::slice::from_raw_parts_mut(out.cast::<std::mem::MaybeUninit<u8>>(), size) };
        getrandom::fill_uninit(buffer).map(|_| ()).map_err(|_| Error::Os)
    }
}
impl port::Environment for $provider {
    unsafe fn get(name: &[u8], out: *mut u8, capacity: usize) -> Result<Lookup> {
        let name = std::str::from_utf8(name).map_err(|_| Error::InvalidArgument)?;
        let value = std::env::var(name).map_err(|e| match e { std::env::VarError::NotPresent => Error::NotFound, _ => Error::Os })?;
        let needed = value.len() + 1;
        if capacity < needed { return Ok(Lookup::TooSmall(needed)); }
        unsafe { ptr::copy_nonoverlapping(value.as_ptr(), out, value.len()); out.add(value.len()).write(0); }
        Ok(Lookup::Copied(needed))
    }
}
impl port::Diagnostics for $provider {
    unsafe fn write_stderr(data: *const u8, size: usize) -> core::result::Result<(), (usize, Error)> {
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        let mut done = 0;
        let mut stderr = std::io::stderr().lock();
        while done < size {
            match stderr.write(&bytes[done..]) {
                Ok(0) => return Err((done, Error::Os)),
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err((done, Error::Os)),
            }
        }
        Ok(())
    }
}

/// Size-prefixed allocations so `release` needs no layout from the caller.
impl port::NativeHeap for $provider {
    unsafe fn allocate(size: usize, zero: bool) -> Result<*mut c_void> {
        let layout = std::alloc::Layout::from_size_align(size.checked_add(HEADER).ok_or(Error::InvalidArgument)?, HEADER).map_err(|_| Error::InvalidArgument)?;
        let raw = unsafe { if zero { std::alloc::alloc_zeroed(layout) } else { std::alloc::alloc(layout) } };
        if raw.is_null() { return Err(Error::OutOfMemory); }
        unsafe { raw.cast::<usize>().write(size); }
        Ok(unsafe { raw.add(HEADER) }.cast())
    }
    unsafe fn resize(address: *mut c_void, size: usize) -> Result<*mut c_void> {
        if address.is_null() { return unsafe { <Self as port::NativeHeap>::allocate(size, false) }; }
        let old = unsafe { address.cast::<u8>().sub(HEADER) };
        let old_size = unsafe { old.cast::<usize>().read() };
        let new = unsafe { <Self as port::NativeHeap>::allocate(size, false) }?;
        unsafe { ptr::copy_nonoverlapping(address.cast::<u8>(), new.cast::<u8>(), old_size.min(size)); }
        unsafe { <Self as port::NativeHeap>::release(address) }?;
        Ok(new)
    }
    unsafe fn release(address: *mut c_void) -> Result<()> {
        if address.is_null() { return Ok(()); }
        let raw = unsafe { address.cast::<u8>().sub(HEADER) };
        let size = unsafe { raw.cast::<usize>().read() };
        let layout = std::alloc::Layout::from_size_align(size + HEADER, HEADER).map_err(|_| Error::InvalidArgument)?;
        unsafe { std::alloc::dealloc(raw, layout) };
        Ok(())
    }
}
const HEADER: usize = 16;

struct Event { state: Mutex<EventState>, signal: Condvar }
struct EventState { manual: bool, signaled: bool, waiters: usize }
impl port::Events for $provider {
    unsafe fn create(manual_reset: bool, initially_set: bool) -> Result<*mut c_void> {
        Ok(boxed(Event { state: Mutex::new(EventState { manual: manual_reset, signaled: initially_set, waiters: 0 }), signal: Condvar::new() }))
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        if event.state.lock().map_err(|_| Error::Os)?.waiters != 0 { return Err(Error::Busy); }
        drop(unsafe { take::<Event>(handle) }?);
        Ok(())
    }
    unsafe fn set(handle: *mut c_void) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        let mut state = event.state.lock().map_err(|_| Error::Os)?;
        state.signaled = true;
        if state.manual { event.signal.notify_all(); } else { event.signal.notify_one(); }
        Ok(())
    }
    unsafe fn reset(handle: *mut c_void) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        event.state.lock().map_err(|_| Error::Os)?.signaled = false;
        Ok(())
    }
    unsafe fn wait(handle: *mut c_void, timeout_ns: u64) -> Result<()> {
        let event = unsafe { borrow::<Event>(handle) }?;
        let mut state = event.state.lock().map_err(|_| Error::Os)?;
        state.waiters += 1;
        let deadline = (timeout_ns != INFINITE).then(|| Instant::now() + Duration::from_nanos(timeout_ns));
        let mut outcome = Ok(());
        while !state.signaled {
            match deadline {
                None => state = event.signal.wait(state).map_err(|_| Error::Os)?,
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline { outcome = Err(Error::Timeout); break; }
                    // Spurious wakes recheck the state against the SAME deadline.
                    state = event.signal.wait_timeout(state, deadline - now).map_err(|_| Error::Os)?.0;
                }
            }
        }
        if state.signaled {
            outcome = Ok(());
            if !state.manual { state.signaled = false; }
        }
        state.waiters -= 1;
        outcome
    }
}

struct RecursiveMutex { state: Mutex<MutexState>, released: Condvar, recursive: bool }
struct MutexState { owner: Option<std::thread::ThreadId>, depth: usize, users: usize }
impl port::Mutexes for $provider {
    unsafe fn create(recursive: bool) -> Result<*mut c_void> {
        Ok(boxed(RecursiveMutex { state: Mutex::new(MutexState { owner: None, depth: 0, users: 0 }), released: Condvar::new(), recursive }))
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let mutex = unsafe { borrow::<RecursiveMutex>(handle) }?;
        if mutex.state.lock().map_err(|_| Error::Os)?.users != 0 { return Err(Error::Busy); }
        drop(unsafe { take::<RecursiveMutex>(handle) }?);
        Ok(())
    }
    unsafe fn lock(handle: *mut c_void) -> Result<()> {
        let mutex = unsafe { borrow::<RecursiveMutex>(handle) }?;
        let me = std::thread::current().id();
        let mut state = mutex.state.lock().map_err(|_| Error::Os)?;
        if state.owner == Some(me) {
            if !mutex.recursive || state.depth == usize::MAX { return Err(Error::Os); }
            state.depth += 1;
            state.users += 1;
            return Ok(());
        }
        state.users += 1;
        while state.owner.is_some() { state = mutex.released.wait(state).map_err(|_| Error::Os)?; }
        state.owner = Some(me);
        state.depth = 1;
        Ok(())
    }
    unsafe fn unlock(handle: *mut c_void) -> Result<()> {
        let mutex = unsafe { borrow::<RecursiveMutex>(handle) }?;
        let mut state = mutex.state.lock().map_err(|_| Error::Os)?;
        if state.owner != Some(std::thread::current().id()) { return Err(Error::Os); }
        state.depth -= 1;
        state.users -= 1;
        if state.depth == 0 { state.owner = None; mutex.released.notify_one(); }
        Ok(())
    }
}

struct Thread(Option<std::thread::JoinHandle<()>>);
struct Start(Entry, usize);
impl port::Threads for $provider {
    unsafe fn create(entry: Entry, argument: *mut c_void, stack_size: usize) -> Result<*mut c_void> {
        let mut builder = std::thread::Builder::new();
        if stack_size != 0 { builder = builder.stack_size(stack_size); }
        let start = Start(entry, argument as usize);
        let handle = builder.spawn(move || { let Start(entry, arg) = start; unsafe { entry(arg as *mut c_void) }; }).map_err(|_| Error::OutOfMemory)?;
        Ok(boxed(Thread(Some(handle))))
    }
    unsafe fn join(handle: *mut c_void) -> Result<()> {
        let mut thread = unsafe { take::<Thread>(handle) }?;
        match thread.0.take() { Some(h) => h.join().map_err(|_| Error::Os), None => Err(Error::InvalidArgument) }
    }
    unsafe fn detach(handle: *mut c_void) -> Result<()> {
        let thread = unsafe { take::<Thread>(handle) }?;
        drop(thread); // dropping a JoinHandle detaches the thread
        Ok(())
    }
}

/// Dynamic thread-local slots. Keys index a process-wide table of destructors;
/// each thread keeps its own value vector and runs destructors when it exits.
static KEYS: Mutex<Vec<Option<Option<Destructor>>>> = Mutex::new(Vec::new());
struct Slots(Vec<*mut c_void>);
impl Drop for Slots {
    fn drop(&mut self) {
        // Destructors may set other slots; loop until every value is NULL, bounded.
        for _ in 0..4 {
            let mut pending = false;
            for (index, slot) in self.0.iter_mut().enumerate() {
                let value = std::mem::replace(slot, ptr::null_mut());
                if value.is_null() { continue; }
                let destructor = KEYS.lock().ok().and_then(|keys| keys.get(index).copied().flatten().flatten());
                if let Some(destructor) = destructor { pending = true; unsafe { destructor(value) }; }
            }
            if !pending { break; }
        }
    }
}
thread_local! { static SLOTS: RefCell<Slots> = const { RefCell::new(Slots(Vec::new())) }; }
impl port::ThreadLocal for $provider {
    unsafe fn create(destructor: Option<Destructor>) -> Result<*mut c_void> {
        let mut keys = KEYS.lock().map_err(|_| Error::Os)?;
        let index = match keys.iter().position(|k| k.is_none()) {
            Some(index) => { keys[index] = Some(destructor); index }
            None => { keys.push(Some(destructor)); keys.len() - 1 }
        };
        Ok(boxed(index))
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let index = *unsafe { take::<usize>(handle) }?;
        let mut keys = KEYS.lock().map_err(|_| Error::Os)?;
        match keys.get_mut(index) { Some(slot) if slot.is_some() => { *slot = None; Ok(()) }, _ => Err(Error::InvalidArgument) }
    }
    unsafe fn get(handle: *mut c_void) -> Result<*mut c_void> {
        let index = *unsafe { borrow::<usize>(handle) }?;
        Ok(SLOTS.with(|slots| slots.borrow().0.get(index).copied().unwrap_or(ptr::null_mut())))
    }
    unsafe fn set(handle: *mut c_void, value: *mut c_void) -> Result<()> {
        let index = *unsafe { borrow::<usize>(handle) }?;
        if !KEYS.lock().map_err(|_| Error::Os)?.get(index).is_some_and(|k| k.is_some()) { return Err(Error::InvalidArgument); }
        SLOTS.with(|slots| {
            let mut slots = slots.borrow_mut();
            if slots.0.len() <= index { slots.0.resize(index + 1, ptr::null_mut()); }
            slots.0[index] = value;
        });
        Ok(())
    }
}

struct RwLock { state: Mutex<(usize, bool)>, changed: Condvar }
impl port::RwLocks for $provider {
    unsafe fn create() -> Result<*mut c_void> { Ok(boxed(RwLock { state: Mutex::new((0, false)), changed: Condvar::new() })) }
    unsafe fn read(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let mut state = lock.state.lock().map_err(|_| Error::Os)?;
        while state.1 { state = lock.changed.wait(state).map_err(|_| Error::Os)?; }
        state.0 += 1;
        Ok(())
    }
    unsafe fn write(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let mut state = lock.state.lock().map_err(|_| Error::Os)?;
        while state.1 || state.0 != 0 { state = lock.changed.wait(state).map_err(|_| Error::Os)?; }
        state.1 = true;
        Ok(())
    }
    unsafe fn unlock(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let mut state = lock.state.lock().map_err(|_| Error::Os)?;
        if state.1 { state.1 = false; } else if state.0 != 0 { state.0 -= 1; } else { return Err(Error::Os); }
        lock.changed.notify_all();
        Ok(())
    }
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let lock = unsafe { borrow::<RwLock>(handle) }?;
        let held = { let s = lock.state.lock().map_err(|_| Error::Os)?; s.1 || s.0 != 0 };
        if held { return Err(Error::Busy); }
        drop(unsafe { take::<RwLock>(handle) }?);
        Ok(())
    }
}

    };
}
