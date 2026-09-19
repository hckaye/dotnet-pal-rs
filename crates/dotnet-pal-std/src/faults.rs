//! Fault reporting of the desktop port, on POSIX signals: Linux and macOS, AArch64 and x86-64.
//!
//! `enable` takes SIGSEGV, SIGBUS, SIGFPE, SIGILL and SIGTRAP with `sigaction` and keeps
//! the actions it replaced. The handler converts the interrupted `ucontext_t` into the
//! boundary's frame, reports the fault and, on `Resume`, writes every register back so
//! the kernel continues from the edited frame. Otherwise it puts the previous action back
//! and returns: the faulting instruction runs again and the process ends the way it would
//! have without this provider (the default action, Rust's stack-overflow handler, ...).
//!
//! The handler runs on the faulting stack, never an alternate one, because the consumer
//! resumes on that stack. A stack overflow therefore ends the process before it can be
//! reported, and `STACK_OVERFLOW` is never produced. What the port keeps under the
//! interrupted `sp` is the kernel's signal frame, which starts below the red zone where
//! the ABI has one. Everything on the handler's path is async-signal-safe: atomics,
//! `sigaction`, `raise`, `pthread_self`, no allocation, no lock, nothing that can panic.
//!
//! The kernels differ in what they tell a handler. Seen on Linux 6.12 (AArch64) and on
//! Darwin 25.5 (Apple silicon, x86-64 under Rosetta):
//!
//! | | Linux | macOS |
//! | --- | --- | --- |
//! | A signal somebody sent | `si_code <= 0` | `si_code` and `si_addr` are those of a fault (`kill -SEGV` reads as an invalid access to address 0); only the exception state in the context differs, see below |
//! | An alignment fault | SIGBUS with `BUS_ADRALN` | AArch64: every SIGBUS says `BUS_ADRALN`, a protection fault too, so the exception syndrome decides. x86-64: as Linux |
//! | An invalid access | SIGSEGV | SIGSEGV for an unmapped address, SIGBUS for a protected one |
//! | A fault whose signal is blocked | ends the process | runs the instruction again, forever |
//!
//! So on Linux the five signals stay blocked while the handler runs, and the kernel ends
//! the process on a fault inside it. On macOS they stay deliverable, and the handler
//! recognizes a fault of a thread that is already inside it by a table of thread ids.
//!
//! A signal somebody sent is not reported. It has no faulting instruction to run again,
//! and a previous handler written for faults would swallow it (Rust's resets the action
//! and returns), so it takes its default action. macOS identifies one by the exception the
//! thread took last: a system call (`ESR_EL1` class 0x15) on AArch64, a trap number that is
//! no CPU vector on x86-64. That state is not always the signal's cause. A signal sent
//! to a thread whose last exception was a page fault the kernel resolved carries that
//! fault's syndrome and address and is reported as `ACCESS` there; under Rosetta the state
//! is that of the last fault the thread was signalled for, however long ago. Signals cannot
//! close this gap; a Mach exception port, which sees CPU faults only, could.
use super::Std;
use dotnet_pal_rs::faults::{self, Disposition, Frame};
use dotnet_pal_rs::port::{self, Error, Result};
use std::{cell::UnsafeCell, ffi::c_void, mem::MaybeUninit, ptr, sync::{atomic::{AtomicUsize, Ordering}, Mutex}};

/// What the port declaration names; `lib.rs` has the absent counterpart.
pub type Provider = Std;
const SIGNALS: [libc::c_int; 5] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGFPE, libc::SIGILL, libc::SIGTRAP];
/// The actions `enable` replaced, in the order of `SIGNALS`. A slot is written before the
/// handler of its signal is installed and never afterwards, so the handler reads it
/// unlocked. All zero is the default action on both systems.
struct Previous([UnsafeCell<MaybeUninit<libc::sigaction>>; 5]);
unsafe impl Sync for Previous {}
static PREVIOUS: Previous = Previous([const { UnsafeCell::new(MaybeUninit::zeroed()) }; 5]);
static ARMED: Mutex<bool> = Mutex::new(false);

/// `FPE_INTDIV` and `FPE_INTOVF` of <asm-generic/siginfo.h> and of <sys/signal.h>; the libc crate has neither.
const FPE: [libc::c_int; 2] = if cfg!(target_os = "linux") { [1, 2] } else { [7, 8] };

#[cfg(target_os = "linux")]
type Machine = libc::mcontext_t;
#[cfg(target_os = "macos")]
type Machine = libc::__darwin_mcontext64;
/// The register state inside a context the kernel built: part of it on Linux, behind a pointer on macOS.
#[cfg(target_os = "linux")]
unsafe fn machine(context: &mut libc::ucontext_t) -> Option<&mut Machine> { Some(&mut context.uc_mcontext) }
#[cfg(target_os = "macos")]
unsafe fn machine(context: &mut libc::ucontext_t) -> Option<&mut Machine> { unsafe { context.uc_mcontext.as_mut() } }

/// `None` when the CPU raised nothing (somebody sent the signal), else whether a SIGBUS is an alignment fault.
#[cfg(target_os = "linux")]
fn raised(info: &libc::siginfo_t, _: &Machine) -> Option<bool> { (info.si_code > 0).then_some(info.si_code == libc::BUS_ADRALN) }
/// Exception classes 0x22 and 0x26 are PC and SP alignment, 0x24 a data abort whose status 0x21 is alignment.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn raised(_: &libc::siginfo_t, machine: &Machine) -> Option<bool> {
    let (class, status) = (machine.__es.__esr >> 26, machine.__es.__esr & 0x3f);
    (class != 0x15).then_some(class == 0x22 || class == 0x26 || (class == 0x24 && status == 0x21))
}
/// Vectors below 32 are the CPU's exceptions; Rosetta shows 222 until the thread's first fault.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn raised(info: &libc::siginfo_t, machine: &Machine) -> Option<bool> { (machine.__es.__trapno < 32).then_some(info.si_code == libc::BUS_ADRALN) }

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn capture(m: &Machine) -> Frame { Frame { x: m.regs, sp: m.sp, pc: m.pc, pstate: m.pstate } }
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn apply(m: &mut Machine, frame: &Frame) { m.regs = frame.x; m.sp = frame.sp; m.pc = frame.pc; m.pstate = frame.pstate; }
/// The frame is in instruction-encoding order; `gregs` is not. Every index is below its 23 entries.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const ORDER: [libc::c_int; 16] = [libc::REG_RAX, libc::REG_RCX, libc::REG_RDX, libc::REG_RBX, libc::REG_RSP, libc::REG_RBP, libc::REG_RSI, libc::REG_RDI,
    libc::REG_R8, libc::REG_R9, libc::REG_R10, libc::REG_R11, libc::REG_R12, libc::REG_R13, libc::REG_R14, libc::REG_R15];
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn capture(m: &Machine) -> Frame {
    let mut frame = Frame { rip: m.gregs[libc::REG_RIP as usize] as u64, rflags: m.gregs[libc::REG_EFL as usize] as u64, ..Frame::default() };
    for (register, index) in frame.registers.iter_mut().zip(ORDER) { *register = m.gregs[index as usize] as u64; }
    frame
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn apply(m: &mut Machine, frame: &Frame) {
    for (register, index) in frame.registers.iter().zip(ORDER) { m.gregs[index as usize] = *register as libc::greg_t; }
    m.gregs[libc::REG_RIP as usize] = frame.rip as libc::greg_t;
    m.gregs[libc::REG_EFL as usize] = frame.rflags as libc::greg_t;
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn capture(m: &Machine) -> Frame {
    let (s, mut frame) = (&m.__ss, Frame { sp: m.__ss.__sp, pc: m.__ss.__pc, pstate: m.__ss.__cpsr as u64, ..Frame::default() });
    frame.x[..29].copy_from_slice(&s.__x);
    (frame.x[29], frame.x[30]) = (s.__fp, s.__lr);
    frame
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn apply(m: &mut Machine, frame: &Frame) {
    let s = &mut m.__ss;
    s.__x.copy_from_slice(&frame.x[..29]);
    (s.__fp, s.__lr, s.__sp, s.__pc, s.__cpsr) = (frame.x[29], frame.x[30], frame.sp, frame.pc, frame.pstate as u32);
}
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn capture(m: &Machine) -> Frame {
    let s = &m.__ss;
    Frame { registers: [s.__rax, s.__rcx, s.__rdx, s.__rbx, s.__rsp, s.__rbp, s.__rsi, s.__rdi, s.__r8, s.__r9, s.__r10, s.__r11, s.__r12, s.__r13, s.__r14, s.__r15],
        rip: s.__rip, rflags: s.__rflags }
}
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn apply(m: &mut Machine, frame: &Frame) {
    let s = &mut m.__ss;
    [s.__rax, s.__rcx, s.__rdx, s.__rbx, s.__rsp, s.__rbp, s.__rsi, s.__rdi, s.__r8, s.__r9, s.__r10, s.__r11, s.__r12, s.__r13, s.__r14, s.__r15] = frame.registers;
    (s.__rip, s.__rflags) = (frame.rip, frame.rflags);
}

/// Threads inside the handler. With every entry taken a thread goes untracked, and a
/// fault it takes inside the handler is reported like any other.
#[cfg(target_os = "macos")]
static INSIDE: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
/// `None` for a fault of a thread that is already inside the handler.
#[cfg(target_os = "macos")]
fn enter() -> Option<Option<&'static AtomicUsize>> {
    let me = unsafe { libc::pthread_self() };
    if INSIDE.iter().any(|thread| thread.load(Ordering::Acquire) == me) { return None; }
    Some(INSIDE.iter().find(|thread| thread.compare_exchange(0, me, Ordering::AcqRel, Ordering::Acquire).is_ok()))
}
/// The kernel ends the process first: the fault signals are blocked inside the handler.
#[cfg(target_os = "linux")]
fn enter() -> Option<Option<&'static AtomicUsize>> { Some(None) }

#[cfg(target_os = "linux")]
unsafe fn errno() -> *mut libc::c_int { unsafe { libc::__errno_location() } }
#[cfg(target_os = "macos")]
unsafe fn errno() -> *mut libc::c_int { unsafe { libc::__error() } }

enum Outcome { Resumed, Declined, Sent }
/// # Safety
/// `info` and `context` are what the kernel handed the signal handler.
unsafe fn report(signal: libc::c_int, info: *const libc::siginfo_t, context: *mut libc::ucontext_t) -> Outcome {
    let (Some(info), Some(context)) = (unsafe { info.as_ref() }, unsafe { context.as_mut() }) else { return Outcome::Declined; };
    let Some(machine) = (unsafe { machine(context) }) else { return Outcome::Declined; };
    let Some(aligned) = raised(info, machine) else { return Outcome::Sent; };
    let Some(inside) = enter() else { return Outcome::Declined; };
    let kind = match signal {
        libc::SIGSEGV => faults::ACCESS,
        libc::SIGBUS => if aligned { faults::ALIGNMENT } else { faults::ACCESS },
        libc::SIGFPE => if info.si_code == FPE[0] { faults::INTEGER_DIVIDE } else if info.si_code == FPE[1] { faults::INTEGER_OVERFLOW } else { faults::FLOATING_POINT },
        libc::SIGILL => faults::ILLEGAL_INSTRUCTION,
        _ => faults::BREAKPOINT,
    };
    let address = if kind == faults::ACCESS || kind == faults::ALIGNMENT { unsafe { info.si_addr() as usize } } else { 0 };
    let mut frame = capture(machine);
    let disposition = faults::deliver(kind, address, &mut frame);
    if let Some(thread) = inside { thread.store(0, Ordering::Release); }
    if disposition != Disposition::Resume { return Outcome::Declined; }
    // The kernel resumes from the context it is handed back, so every register goes back.
    apply(machine, &frame);
    Outcome::Resumed
}
extern "C" fn on_signal(signal: libc::c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    let saved = unsafe { *errno() };
    match unsafe { report(signal, info, context.cast()) } {
        Outcome::Resumed => {}
        // With the previous action back, returning runs the faulting instruction again and the run ends as it would have without this provider.
        Outcome::Declined => if let Some((_, previous)) = SIGNALS.iter().zip(&PREVIOUS.0).find(|(s, _)| **s == signal) {
            unsafe { libc::sigaction(signal, previous.get().cast(), ptr::null_mut()) };
        },
        // A sent signal has no faulting instruction to run again, and a previous handler written for faults (Rust's resets the
        // action and returns) would swallow it: it takes its default action.
        Outcome::Sent => unsafe {
            let mut standard: libc::sigaction = std::mem::zeroed();
            standard.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut standard.sa_mask);
            libc::sigaction(signal, &standard, ptr::null_mut());
            libc::raise(signal);
        },
    }
    unsafe { *errno() = saved };
}

impl port::Faults for Std {
    fn enable() -> Result<()> {
        let mut armed = ARMED.lock().map_err(|_| Error::Os)?;
        if *armed { return Ok(()); }
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut c_void) as usize;
        // No SA_ONSTACK: the consumer resumes on the faulting stack.
        action.sa_flags = libc::SA_SIGINFO | if cfg!(target_os = "macos") { libc::SA_NODEFER } else { 0 };
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        if cfg!(target_os = "linux") { for signal in SIGNALS { unsafe { libc::sigaddset(&mut action.sa_mask, signal) }; } }
        for (index, (signal, previous)) in SIGNALS.iter().zip(&PREVIOUS.0).enumerate() {
            let installed = unsafe { libc::sigaction(*signal, ptr::null(), previous.get().cast()) == 0 && libc::sigaction(*signal, &action, ptr::null_mut()) == 0 };
            if !installed {
                for (signal, previous) in SIGNALS.iter().zip(&PREVIOUS.0).take(index) { unsafe { libc::sigaction(*signal, previous.get().cast(), ptr::null_mut()) }; }
                return Err(Error::Os);
            }
        }
        *armed = true;
        Ok(())
    }
}
