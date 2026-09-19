//! Synchronous CPU faults without POSIX signals (`CAP_FAULTS`).
//!
//! The native context group hands a consumer raw signal machinery, which only a
//! POSIX target has. This group is the other direction: the port owns its trap
//! path and *reports* each fault with the interrupted registers in a layout the
//! boundary defines per architecture. The consumer's handler may edit the frame
//! and ask the port to resume from it, which is how a runtime turns a null
//! dereference in compiled managed code into a managed exception.
//!
//! A port calls [`deliver`] from its trap handler. Everything on that path is
//! lock-free and allocation-free; the handler may not block, unwind or call the
//! boundary. A fault taken while a handler is running on the same thread is the
//! port's to end the run with: only the port can tell a nested fault from a
//! concurrent one on another thread.
use crate::kernel::BUSY;
use crate::port::{Faults, Port};
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, UNSUPPORTED};
use core::{ffi::c_void, mem, ptr, sync::atomic::{AtomicPtr, AtomicU8, Ordering}};

pub const CAP: u64 = 536870912;
/// Invalid memory access; `address` is the address that faulted.
pub const ACCESS: u32 = 1;
pub const ALIGNMENT: u32 = 2;
pub const INTEGER_DIVIDE: u32 = 3;
pub const INTEGER_OVERFLOW: u32 = 4;
pub const FLOATING_POINT: u32 = 5;
pub const ILLEGAL_INSTRUCTION: u32 = 6;
pub const BREAKPOINT: u32 = 7;
pub const STACK_OVERFLOW: u32 = 8;
pub const UNHANDLED: u32 = 0;
pub const RESUME: u32 = 1;
pub const FRAME_ARM64: u64 = 0x4652_414d_4500_0001;
pub const FRAME_X64: u64 = 0x4652_414d_4500_0002;

/// AArch64: `x[30]` is the link register.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameArm64 { pub x: [u64; 31], pub sp: u64, pub pc: u64, pub pstate: u64 }
/// x86-64: general registers in instruction-encoding order (rax, rcx, rdx, rbx,
/// rsp, rbp, rsi, rdi, r8..r15).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameX64 { pub registers: [u64; 16], pub rip: u64, pub rflags: u64 }
/// Placeholder on architectures the boundary defines no frame for.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoFrame { _private: u64 }

#[cfg(target_arch = "aarch64")]
pub type Frame = FrameArm64;
#[cfg(target_arch = "aarch64")]
pub const FRAME_TAG: u64 = FRAME_ARM64;
#[cfg(target_arch = "x86_64")]
pub type Frame = FrameX64;
#[cfg(target_arch = "x86_64")]
pub const FRAME_TAG: u64 = FRAME_X64;
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub type Frame = NoFrame;
/// Zero: no frame layout, so a port cannot provide the capability here.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub const FRAME_TAG: u64 = 0;

pub type Handler = unsafe extern "C" fn(u32, usize, *mut c_void, usize, *mut c_void) -> u32;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub frame_tag: Option<extern "C" fn() -> u64>,
    pub frame_size: Option<extern "C" fn() -> usize>,
    pub install: Option<unsafe extern "C" fn(Option<Handler>, *mut c_void) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
/// What a C host supplies: it delivers a fault by calling the callback `enable` received.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostOps {
    pub frame_tag: Option<extern "C" fn() -> u64>,
    pub frame_size: Option<extern "C" fn() -> usize>,
    pub enable: Option<unsafe extern "C" fn(Option<Handler>) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: HostOps }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub installs: u64, pub delivered: u64, pub resumed: u64, pub unhandled: u64, pub rejected: u64 }
static COUNTERS: [Counter; 5] = [const { Counter::new() }; 5];

const EMPTY_STATE: u8 = 0;
const INSTALLING: u8 = 1;
const INSTALLED: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(EMPTY_STATE);
static HANDLER: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
static DATA: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// What the port does after [`deliver`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Nobody claimed the fault: end the run as the port would without a handler.
    Unhandled,
    /// Continue from the frame, which the handler has edited.
    Resume,
}
/// Reports one fault to the installed handler. Callable from a trap handler.
pub fn deliver(kind: u32, address: usize, frame: &mut Frame) -> Disposition {
    let handler = HANDLER.load(Ordering::Acquire);
    if handler.is_null() || !(ACCESS..=STACK_OVERFLOW).contains(&kind) {
        COUNTERS[if handler.is_null() { 3 } else { 4 }].increment();
        return Disposition::Unhandled;
    }
    COUNTERS[1].increment();
    // SAFETY: only `install` stores here, and it stores a `Handler`.
    let handler: Handler = unsafe { mem::transmute::<*mut (), Handler>(handler) };
    let answer = unsafe { handler(kind, address, (frame as *mut Frame).cast(), mem::size_of::<Frame>(), DATA.load(Ordering::Acquire)) };
    if answer == RESUME { COUNTERS[2].increment(); Disposition::Resume } else { COUNTERS[3].increment(); Disposition::Unhandled }
}
/// [`deliver`] with the C handler signature, for a host table to call. The last
/// argument is unused: the consumer's own data pointer is the boundary's to pass.
///
/// # Safety
/// `frame` is null or points to storage the caller lends exclusively for the call.
pub unsafe extern "C" fn deliver_raw(kind: u32, address: usize, frame: *mut c_void, size: usize, _: *mut c_void) -> u32 {
    if frame.is_null() || !(frame as usize).is_multiple_of(mem::align_of::<Frame>()) || size != mem::size_of::<Frame>() || FRAME_TAG == 0 {
        COUNTERS[4].increment();
        return UNHANDLED;
    }
    // SAFETY: checked non-null, aligned and exactly one frame long; the host lends it for the call.
    match deliver(kind, address, unsafe { &mut *frame.cast::<Frame>() }) { Disposition::Resume => RESUME, Disposition::Unhandled => UNHANDLED }
}
extern "C" fn frame_tag() -> u64 { FRAME_TAG }
extern "C" fn frame_size() -> usize { mem::size_of::<Frame>() }
unsafe extern "C" fn install<F: Faults>(handler: Option<Handler>, data: *mut c_void) -> u32 {
    let Some(handler) = handler else { COUNTERS[4].increment(); return INVALID_ARGUMENT; };
    if STATE.compare_exchange(EMPTY_STATE, INSTALLING, Ordering::AcqRel, Ordering::Acquire).is_err() { COUNTERS[4].increment(); return BUSY; }
    // The data pointer is published before the handler, so a fault never sees one without the other.
    DATA.store(data, Ordering::Release);
    HANDLER.store(handler as *mut (), Ordering::Release);
    let status = match F::enable() { Ok(()) => OK, Err(e) => match e.status() { UNSUPPORTED => UNSUPPORTED, _ => OS_ERROR } };
    if status != OK {
        HANDLER.store(ptr::null_mut(), Ordering::Release);
        STATE.store(EMPTY_STATE, Ordering::Release);
        COUNTERS[4].increment();
        return status;
    }
    STATE.store(INSTALLED, Ordering::Release);
    COUNTERS[0].increment();
    OK
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { installs: COUNTERS[0].load(), delivered: COUNTERS[1].load(), resumed: COUNTERS[2].load(), unhandled: COUNTERS[3].load(), rejected: COUNTERS[4].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { frame_tag: None, frame_size: None, install: None, read_stats: Some(read_stats) };
/// A port that claims fault reporting on an architecture without a frame layout
/// rejects negotiation: a consumer must never install against a frame it cannot read.
pub fn negotiate<P: Port>() -> Option<(u64, Ops)> {
    type T<P> = <P as Port>::Faults;
    if !T::<P>::PROVIDED { return Some((0, EMPTY)); }
    if FRAME_TAG == 0 { return None; }
    Some((CAP, Ops { frame_tag: Some(frame_tag), frame_size: Some(frame_size), install: Some(install::<T<P>>), read_stats: Some(read_stats) }))
}
