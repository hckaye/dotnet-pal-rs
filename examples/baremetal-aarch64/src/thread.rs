//! Cooperative threads, thread identity and dynamic thread-local slots.
//!
//! One core, no timer interrupt, run to block: a thread keeps the CPU until it
//! calls something that waits (an event, a mutex, a sleep, a join) or yields.
//! Waiting is implemented as polling the condition and handing the CPU to the
//! next live thread; when nothing else can run, the waiter spins on the counter.
//! That is honest for this machine and it means a thread that waits forever on a
//! condition nobody can satisfy hangs the run instead of deadlock-detecting.
//!
//! Thread state lives in a fixed table of [`MAX_THREADS`] entries. A handle is
//! the address of its entry, so no allocation is involved in creating a thread
//! beyond its stack and its TLS block. Running out of entries is
//! `Error::OutOfMemory`, which is what the boundary tells the runtime.
use crate::sync::Single;
use crate::{clock, memory, tls, Baremetal};
use core::{ffi::c_void, mem, ptr};
use dotnet_pal_rs::kernel::{Destructor, Entry};
use dotnet_pal_rs::port::{self, Error, Result};

pub const MAX_THREADS: usize = 16;
pub const MAX_TLS_SLOTS: usize = 64;
const DEFAULT_STACK: usize = 128 * 1024;
const MIN_STACK: usize = 32 * 1024;
/// Size of the register frame `pal_context_switch` writes; see switch.S.
const FRAME: usize = 176;

extern "C" {
    static __boot_stack_bottom: u8;
    static __boot_stack_top: u8;
    fn pal_context_switch(save: *mut usize, next: usize);
    fn pal_thread_trampoline();
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The entry is unused.
    Free,
    /// Created and not finished: it is runnable or polling a condition.
    Live,
    /// The entry function returned; the stack is idle and waits for a join.
    Finished,
}

pub struct Thread {
    pub state: State,
    /// Stack pointer of the saved register frame while this thread is not running.
    sp: usize,
    entry: Option<Entry>,
    argument: *mut c_void,
    /// Region allocation of the stack; zero for the boot thread, which uses the
    /// stack reserved by the linker script.
    stack: usize,
    stack_size: usize,
    stack_low: usize,
    stack_high: usize,
    tls: tls::Block,
    slots: [*mut c_void; MAX_TLS_SLOTS],
    name: [u8; 15],
    name_length: usize,
    detached: bool,
    /// Finished with nobody left to join: another thread may free its memory.
    reclaim: bool,
}
impl Thread {
    const EMPTY: Thread = Thread {
        state: State::Free,
        sp: 0,
        entry: None,
        argument: ptr::null_mut(),
        stack: 0,
        stack_size: 0,
        stack_low: 0,
        stack_high: 0,
        tls: tls::Block { base: 0, size: 0, pointer: 0 },
        slots: [ptr::null_mut(); MAX_TLS_SLOTS],
        name: [0; 15],
        name_length: 0,
        detached: false,
        reclaim: false,
    };
}

static THREADS: Single<[Thread; MAX_THREADS]> = Single::new([const { Thread::EMPTY }; MAX_THREADS]);
static CURRENT: Single<usize> = Single::new(0);
/// Where a finished thread's register frame goes: it is written to the dying
/// stack and never read again.
static DISCARDED: Single<usize> = Single::new(0);

// Borrowing the tables. Sound only under the single-core, non-preemptive rule in
// `sync`: no caller keeps one of these borrows across a context switch.
fn table() -> &'static mut [Thread; MAX_THREADS] {
    // SAFETY: see the module and `sync` comments.
    unsafe { THREADS.get() }
}
pub fn current() -> usize {
    // SAFETY: as above.
    *unsafe { CURRENT.get() }
}
fn set_current(index: usize) {
    // SAFETY: as above.
    *unsafe { CURRENT.get() } = index;
}

/// Frees the memory of threads that finished and will not be joined. Never the
/// running thread: it is still standing on the stack that would be freed.
fn reclaim_finished() {
    let running = current();
    for index in 0..MAX_THREADS {
        let thread = &mut table()[index];
        if index == running || thread.state != State::Finished || !thread.reclaim {
            continue;
        }
        let (stack, stack_size, block) = (thread.stack, thread.stack_size, thread.tls);
        *thread = Thread::EMPTY;
        if stack_size != 0 {
            memory::give(stack, stack_size);
        }
        tls::release(block);
    }
}
fn next_live(after: usize) -> Option<usize> {
    for step in 1..=MAX_THREADS {
        let index = (after + step) % MAX_THREADS;
        if index != after && table()[index].state == State::Live {
            return Some(index);
        }
    }
    None
}
/// Hands the CPU to the next live thread. `false` means this thread is the only
/// one that can run, so the caller keeps going (spinning, if it is waiting).
pub fn schedule() -> bool {
    reclaim_finished();
    let running = current();
    let Some(next) = next_live(running) else { return false };
    let save = ptr::addr_of_mut!(table()[running].sp);
    let resume = table()[next].sp;
    set_current(next);
    // SAFETY: `save` points at this thread's saved-stack-pointer slot and
    // `resume` is a frame this port built or saved for the incoming thread.
    unsafe { pal_context_switch(save, resume) };
    true
}
/// Runs other threads until `ready` holds, or until the deadline passes.
/// `deadline` is a monotonic nanosecond value; `None` waits forever.
pub fn block_until(deadline: Option<u64>, mut ready: impl FnMut() -> bool) -> Result<()> {
    loop {
        if ready() {
            return Ok(());
        }
        if let Some(deadline) = deadline {
            if clock::monotonic_ns().unwrap_or(u64::MAX) >= deadline {
                return if ready() { Ok(()) } else { Err(Error::Timeout) };
            }
        }
        if !schedule() {
            core::hint::spin_loop();
        }
    }
}

/// Prepares the boot thread: its TLS block, its stack bounds and its table entry.
/// Runs before `.init_array` and `main`, so C++ constructors and `main` itself
/// already see working thread-local variables.
pub fn init_boot_thread() {
    let Some(block) = tls::allocate() else {
        crate::uart::write(b"FATAL no memory for the boot thread TLS block\n");
        crate::exit::exit(134);
    };
    // SAFETY: the block belongs to this thread for the rest of the run.
    unsafe { tls::install(block.pointer) };
    let thread = &mut table()[0];
    *thread = Thread::EMPTY;
    thread.state = State::Live;
    thread.tls = block;
    thread.stack_low = ptr::addr_of!(__boot_stack_bottom) as usize;
    thread.stack_high = ptr::addr_of!(__boot_stack_top) as usize;
    set_current(0);
}

fn index_of(handle: *mut c_void) -> Option<usize> {
    let base = table().as_ptr() as usize;
    let address = handle as usize;
    let size = mem::size_of::<Thread>();
    if address < base || (address - base) % size != 0 {
        return None;
    }
    let index = (address - base) / size;
    (index < MAX_THREADS).then_some(index)
}
fn handle_of(index: usize) -> *mut c_void {
    ptr::addr_of_mut!(table()[index]).cast()
}

/// Entered from `pal_thread_trampoline` with the thread's table index. Runs the
/// entry point, then the thread-local destructors, then leaves the CPU for good.
#[no_mangle]
pub extern "C" fn pal_thread_main(index: usize) -> ! {
    let thread = &table()[index];
    let (entry, argument) = (thread.entry, thread.argument);
    if let Some(entry) = entry {
        // SAFETY: the entry and its argument come from a `thread_create` caller
        // that owns their validity for the life of the thread.
        unsafe { entry(argument) };
    }
    run_destructors(index);
    let thread = &mut table()[index];
    thread.state = State::Finished;
    thread.reclaim = thread.detached;
    leave()
}
/// Switches away from a finished thread. The outgoing register frame is written
/// to the dying stack and dropped, because nothing will resume this thread.
fn leave() -> ! {
    loop {
        reclaim_finished();
        let Some(next) = next_live(current()) else {
            crate::uart::write(b"FATAL the last thread finished without exiting\n");
            crate::exit::exit(134);
        };
        let resume = table()[next].sp;
        set_current(next);
        // SAFETY: the save slot is scratch; `resume` is the incoming thread's frame.
        unsafe { pal_context_switch(DISCARDED.get(), resume) };
    }
}

impl port::Threads for Baremetal {
    unsafe fn create(entry: Entry, argument: *mut c_void, stack_size: usize) -> Result<*mut c_void> {
        crate::trace(b"thread create stack", stack_size as u64);
        let Some(index) = (0..MAX_THREADS).find(|&i| table()[i].state == State::Free) else {
            return Err(Error::OutOfMemory);
        };
        let size = memory::round_up(
            if stack_size == 0 { DEFAULT_STACK } else { stack_size.max(MIN_STACK) },
            memory::PAGE,
        );
        let stack = memory::take(size, memory::PAGE).ok_or(Error::OutOfMemory)?;
        let Some(block) = tls::allocate() else {
            memory::give(stack, size);
            return Err(Error::OutOfMemory);
        };
        // The frame `pal_context_switch` will restore: the trampoline in x30, the
        // table index in x19 and the thread pointer in the TPIDR_EL0 slot.
        let frame = (stack + size - FRAME) & !15;
        // SAFETY: the stack is this port's fresh, exclusively owned allocation.
        unsafe {
            ptr::write_bytes(frame as *mut u8, 0, FRAME);
            (frame as *mut usize).write(index);
            ((frame + 88) as *mut usize).write(pal_thread_trampoline as *const () as usize);
            ((frame + 96) as *mut usize).write(block.pointer);
        }
        let thread = &mut table()[index];
        *thread = Thread::EMPTY;
        thread.state = State::Live;
        thread.sp = frame;
        thread.entry = Some(entry);
        thread.argument = argument;
        thread.stack = stack;
        thread.stack_size = size;
        thread.stack_low = stack;
        thread.stack_high = stack + size;
        thread.tls = block;
        Ok(handle_of(index))
    }
    /// Waits for the thread to return and then frees its stack and TLS block.
    unsafe fn join(handle: *mut c_void) -> Result<()> {
        let index = index_of(handle).ok_or(Error::InvalidArgument)?;
        let thread = &table()[index];
        if thread.state == State::Free || thread.detached || index == current() {
            return Err(Error::InvalidArgument);
        }
        block_until(None, || table()[index].state == State::Finished)?;
        let thread = &mut table()[index];
        thread.reclaim = true;
        reclaim_finished();
        Ok(())
    }
    unsafe fn detach(handle: *mut c_void) -> Result<()> {
        let index = index_of(handle).ok_or(Error::InvalidArgument)?;
        let thread = &mut table()[index];
        if thread.state == State::Free || thread.detached {
            return Err(Error::InvalidArgument);
        }
        thread.detached = true;
        if thread.state == State::Finished {
            thread.reclaim = true;
            reclaim_finished();
        }
        Ok(())
    }
}

impl port::StackBounds for Baremetal {
    fn current() -> Result<(*mut c_void, *mut c_void)> {
        let thread = &table()[current()];
        if thread.stack_low == 0 {
            return Err(Error::Os);
        }
        Ok((thread.stack_low as *mut c_void, thread.stack_high as *mut c_void))
    }
}
impl port::Identity for Baremetal {
    /// There is one process on this machine and it is this image.
    fn process_id() -> Result<u64> {
        Ok(1)
    }
    /// The 1-based index of the thread's table entry. Identities are reused when
    /// a finished thread's entry is taken by a new thread.
    fn thread_id() -> Result<u64> {
        Ok(current() as u64 + 1)
    }
}
impl port::ThreadName for Baremetal {
    /// Kept for the thread's lifetime, truncated to the 15 bytes a Linux thread
    /// name holds. Nothing on this machine displays it.
    fn set(name: &[u8]) -> Result<()> {
        if name.len() > 15 {
            return Err(Error::InvalidArgument);
        }
        let thread = &mut table()[current()];
        thread.name[..name.len()].copy_from_slice(name);
        thread.name_length = name.len();
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Key {
    used: bool,
    destructor: Option<Destructor>,
}
static KEYS: Single<[Key; MAX_TLS_SLOTS]> =
    Single::new([Key { used: false, destructor: None }; MAX_TLS_SLOTS]);
fn keys() -> &'static mut [Key; MAX_TLS_SLOTS] {
    // SAFETY: see the module comment.
    unsafe { KEYS.get() }
}
fn key_index(handle: *mut c_void) -> Option<usize> {
    let base = keys().as_ptr() as usize;
    let address = handle as usize;
    let size = mem::size_of::<Key>();
    if address < base || (address - base) % size != 0 {
        return None;
    }
    let index = (address - base) / size;
    (index < MAX_TLS_SLOTS && keys()[index].used).then_some(index)
}
/// Runs the destructors of the exiting thread's slots, on that thread, with the
/// slot value, like a pthread key. A destructor may set slots again; the sweep
/// repeats a bounded number of times and then gives up.
fn run_destructors(index: usize) {
    for _ in 0..4 {
        let mut again = false;
        for slot in 0..MAX_TLS_SLOTS {
            let value = mem::replace(&mut table()[index].slots[slot], ptr::null_mut());
            if value.is_null() {
                continue;
            }
            let key = keys()[slot];
            if let (true, Some(destructor)) = (key.used, key.destructor) {
                again = true;
                // SAFETY: the destructor was supplied by the key's creator and
                // receives the value that creator's thread stored.
                unsafe { destructor(value) };
            }
        }
        if !again {
            break;
        }
    }
}
impl port::ThreadLocal for Baremetal {
    unsafe fn create(destructor: Option<Destructor>) -> Result<*mut c_void> {
        crate::trace(b"tls create", destructor.is_some() as u64);
        crate::trace(b"tls create", destructor.is_some() as u64);
        let Some(index) = (0..MAX_TLS_SLOTS).find(|&i| !keys()[i].used) else {
            return Err(Error::OutOfMemory);
        };
        keys()[index] = Key { used: true, destructor };
        for thread in table().iter_mut() {
            thread.slots[index] = ptr::null_mut();
        }
        Ok(ptr::addr_of_mut!(keys()[index]).cast())
    }
    /// Frees the key. Destructors are not run: deleting a key is not a thread
    /// exit, exactly as with `pthread_key_delete`.
    unsafe fn destroy(handle: *mut c_void) -> Result<()> {
        let index = key_index(handle).ok_or(Error::InvalidArgument)?;
        keys()[index] = Key { used: false, destructor: None };
        Ok(())
    }
    unsafe fn get(handle: *mut c_void) -> Result<*mut c_void> {
        let index = key_index(handle).ok_or(Error::InvalidArgument)?;
        Ok(table()[current()].slots[index])
    }
    unsafe fn set(handle: *mut c_void, value: *mut c_void) -> Result<()> {
        let index = key_index(handle).ok_or(Error::InvalidArgument)?;
        table()[current()].slots[index] = value;
        Ok(())
    }
}
