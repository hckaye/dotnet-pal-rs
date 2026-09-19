//! The services a .NET NativeAOT runtime asks of the machine beyond memory,
//! time and threads: topology, process lifetime, image inspection, the standard
//! streams and a module view of the one static image. Plus the two hooks the
//! freestanding C runtime (native/freestanding) calls to leave the machine, and
//! the C math library exported from the pure-Rust `libm` crate.
//!
//! Honest limits: one CPU, no entropy, no debugger, no crash dump utility, no
//! build id, no input stream (reads report end of input), no environment.
use crate::{exit, memory, uart, Baremetal};
use core::ffi::c_void;
use dotnet_pal_rs::image::UnwindInfo;
use dotnet_pal_rs::port::{self, Error, Result};
use dotnet_pal_rs::runtime::ModuleInfo;

extern "C" {
    static __image_start: u8;
    static __bss_start: u8;
    static __eh_frame_hdr_start: u8;
    static __eh_frame_hdr_end: u8;
    static __eh_frame_start: u8;
    static __eh_frame_end: u8;
}
fn symbol(reference: &u8) -> usize { reference as *const u8 as usize }
/// RAM as QEMU's virt machine maps it with `-m 1024`; everything below is device space.
const RAM: core::ops::Range<usize> = 0x4000_0000..0x8000_0000;

// Linux AT_HWCAP bits, the feature words the runtime's CPU detection reads.
const HWCAP_FP: u64 = 1 << 0;
const HWCAP_ASIMD: u64 = 1 << 1;
const HWCAP_AES: u64 = 1 << 3;
const HWCAP_PMULL: u64 = 1 << 4;
const HWCAP_SHA1: u64 = 1 << 5;
const HWCAP_SHA2: u64 = 1 << 6;
const HWCAP_CRC32: u64 = 1 << 7;
const HWCAP_ATOMICS: u64 = 1 << 8;
const HWCAP_ASIMDRDM: u64 = 1 << 12;
const HWCAP_LRCPC: u64 = 1 << 15;
const HWCAP_ASIMDDP: u64 = 1 << 20;
const HWCAP_SVE: u64 = 1 << 22;
const HWCAP_ILRCPC: u64 = 1 << 26;
const HWCAP2_SVE2: u64 = 1 << 1;
fn field(register: u64, shift: u32) -> u64 { (register >> shift) & 0xF }
/// The ID registers translated into the HWCAP words a Linux kernel would report.
fn hwcap() -> (u64, u64) {
    let (isar0, isar1, pfr0, zfr0): (u64, u64, u64, u64);
    // SAFETY: reading ID registers at EL1 has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, ID_AA64ISAR0_EL1", out(reg) isar0, options(nomem, nostack));
        core::arch::asm!("mrs {}, ID_AA64ISAR1_EL1", out(reg) isar1, options(nomem, nostack));
        core::arch::asm!("mrs {}, ID_AA64PFR0_EL1", out(reg) pfr0, options(nomem, nostack));
    }
    let mut first = 0;
    if field(pfr0, 16) != 0xF { first |= HWCAP_FP; }
    if field(pfr0, 20) != 0xF { first |= HWCAP_ASIMD; }
    let aes = field(isar0, 4);
    if aes >= 1 { first |= HWCAP_AES; }
    if aes >= 2 { first |= HWCAP_PMULL; }
    if field(isar0, 8) >= 1 { first |= HWCAP_SHA1; }
    if field(isar0, 12) >= 1 { first |= HWCAP_SHA2; }
    if field(isar0, 16) >= 1 { first |= HWCAP_CRC32; }
    if field(isar0, 20) >= 2 { first |= HWCAP_ATOMICS; }
    if field(isar0, 28) >= 1 { first |= HWCAP_ASIMDRDM; }
    if field(isar0, 44) >= 1 { first |= HWCAP_ASIMDDP; }
    let rcpc = field(isar1, 20);
    if rcpc >= 1 { first |= HWCAP_LRCPC; }
    if rcpc >= 2 { first |= HWCAP_ILRCPC; }
    let mut second = 0;
    if field(pfr0, 32) >= 1 {
        first |= HWCAP_SVE;
        // SAFETY: ID_AA64ZFR0_EL1 exists once SVE is implemented.
        unsafe { core::arch::asm!("mrs {}, S3_0_C0_C4_4", out(reg) zfr0, options(nomem, nostack)) };
        if field(zfr0, 0) >= 1 { second |= HWCAP2_SVE2; }
    }
    (first, second)
}

impl port::Topology for Baremetal {
    fn cpu_max() -> Result<u32> { Ok(1) }
    fn cpu_count() -> Result<u32> { Ok(1) }
    fn current_cpu() -> Result<u32> { Ok(0) }
    unsafe fn process_affinity(mask: *mut u8, capacity: usize) -> Result<usize> {
        // SAFETY: the front end validated the buffer for `capacity` bytes.
        if capacity > 0 { unsafe { mask.write(1) }; }
        Ok(1)
    }
    fn set_thread_affinity(cpu: u32) -> Result<()> { if cpu == 0 { Ok(()) } else { Err(Error::InvalidArgument) } }
    fn physical_memory() -> Result<(u64, u64)> {
        let (start, end) = memory::bounds();
        Ok(((end - start) as u64, memory::available() as u64))
    }
    fn memory_limit() -> Result<u64> { Ok(0) }
    fn virtual_limit() -> Result<u64> { Ok(0) }
    /// The machine has no swap device, which is an answer rather than a gap.
    fn swap_memory() -> Result<(u64, u64)> { Ok((0, 0)) }
    fn cache_size() -> Result<usize> { Ok(0) }
    fn cpu_features() -> Result<(u64, u64)> { Ok(hwcap()) }
}

impl port::Process for Baremetal {
    fn exit(code: i32) -> ! { crate::trace(b"process exit", code as u64); exit::exit(code) }
    fn debugger_present() -> Result<bool> { Ok(false) }
    unsafe fn crash_dump(_: &[*const u8], _: *mut u8, _: usize) -> Result<()> { Err(Error::Unsupported) }
}

impl port::Image for Baremetal {
    unsafe fn unwind_info(address: usize) -> Result<UnwindInfo> {
        crate::trace(b"unwind info", address as u64);
        // The whole loaded image is the code range: the runtime's own text, the managed
        // code section and the read-only data between them all belong to this one image.
        // SAFETY: the linker script defines every symbol; only their addresses are taken.
        let (text_start, text_end) = unsafe { (symbol(&__image_start), symbol(&__bss_start)) };
        if !(text_start..text_end).contains(&address) { return Err(Error::NotFound); }
        let (hdr_start, hdr_end, frame_start, frame_end, base) = unsafe {
            (symbol(&__eh_frame_hdr_start), symbol(&__eh_frame_hdr_end), symbol(&__eh_frame_start), symbol(&__eh_frame_end), symbol(&__image_start))
        };
        Ok(UnwindInfo {
            base, text_start, text_length: text_end - text_start,
            eh_frame_hdr: if hdr_end > hdr_start { hdr_start } else { 0 }, eh_frame_hdr_length: hdr_end.saturating_sub(hdr_start),
            eh_frame: if frame_end > frame_start { frame_start } else { 0 }, eh_frame_length: frame_end.saturating_sub(frame_start),
        })
    }
    unsafe fn readable(address: usize, size: usize) -> Result<bool> {
        memory::readable(address, size)
    }
    unsafe fn build_id(_: usize, _: *mut u8, _: usize) -> Result<usize> { Err(Error::NotFound) }
}

/// The one image is the only module: it opens by the anonymous name, its base is
/// the load address, and it has no symbol table to look into.
impl port::Modules for Baremetal {
    unsafe fn open(name: Option<&[u8]>) -> Result<*mut c_void> {
        if name.is_some() { return Err(Error::NotFound); }
        Ok(unsafe { symbol(&__image_start) } as *mut c_void)
    }
    unsafe fn symbol(_: *mut c_void, _: &[u8]) -> Result<*mut c_void> { Err(Error::NotFound) }
    unsafe fn close(_: *mut c_void) -> Result<()> { Ok(()) }
    unsafe fn info(address: *mut c_void) -> Result<ModuleInfo> {
        crate::trace(b"module info", address as u64);
        let (start, end) = unsafe { (symbol(&__image_start), memory::bounds().0) };
        if !(start..end).contains(&(address as usize)) { return Err(Error::NotFound); }
        Ok(ModuleInfo { base: start as *mut c_void, name: b"app\0".as_ptr(), name_length: 3 })
    }
}

/// What this machine can say about itself. There is no environment, no user and
/// no process accounting: those questions are `Unsupported`, or an enumeration
/// that ends at once, rather than invented values.
impl port::SystemInfo for Baremetal {
    unsafe fn environment_entry(_: usize, _: *mut u8, _: usize) -> Result<usize> { Err(Error::NotFound) }
    unsafe fn text(what: u32, out: *mut u8, capacity: usize) -> Result<usize> {
        let text: &[u8] = match what {
            dotnet_pal_rs::system::TEXT_EXECUTABLE_PATH => b"/app\0",
            dotnet_pal_rs::system::TEXT_OS_NAME => b"baremetal-aarch64\0",
            dotnet_pal_rs::system::TEXT_OS_RELEASE => concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes(),
            dotnet_pal_rs::system::TEXT_OS_VERSION => b"QEMU virt, one core, no operating system\0",
            _ => return Err(Error::Unsupported),
        };
        // SAFETY: the front end checked that `out` holds `capacity` bytes.
        if text.len() <= capacity { unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()) }; }
        Ok(text.len())
    }
    fn process_times() -> Result<(u64, u64)> { Err(Error::Unsupported) }
    /// The counter starts with the machine.
    fn uptime_ns() -> Result<u64> { crate::clock::monotonic_ns().ok_or(Error::Os) }
    fn user_ids() -> Result<(u32, u32)> { Err(Error::Unsupported) }
}
impl port::Streams for Baremetal {
    unsafe fn write(_: u32, data: *const u8, size: usize) -> Result<usize> {
        // SAFETY: the front end validated the buffer; both output streams share the serial port.
        uart::write(unsafe { core::slice::from_raw_parts(data, size) });
        Ok(size)
    }
    unsafe fn read(_: u32, _: *mut u8, _: usize) -> Result<usize> { Ok(0) }
    fn is_terminal(_: u32) -> Result<bool> { Ok(true) }
}

/// The freestanding C runtime's exit hooks (native/freestanding/crt.c).
#[no_mangle]
pub extern "C" fn dotnet_pal_freestanding_abort() -> ! {
    uart::write(b"ABORT\n");
    backtrace();
    exit::exit(134)
}
/// Return addresses along the frame-pointer chain, for offline symbolizing.
pub fn backtrace() {
    let mut frame: usize;
    // SAFETY: reading the frame pointer register has no side effects.
    unsafe { core::arch::asm!("mov {}, x29", out(reg) frame, options(nomem, nostack)) };
    uart::write(b"[pal] backtrace:");
    for _ in 0..24 {
        if !RAM.contains(&frame) || frame % 8 != 0
            || !memory::readable(frame, 2 * core::mem::size_of::<usize>()).unwrap_or(false) { break; }
        // SAFETY: the checked pages hold a readable, aligned (fp, lr) pair.
        let (next, link) = unsafe { ((frame as *const usize).read(), (frame as *const usize).add(1).read()) };
        uart::write(b" ");
        uart::write_hex(link as u64);
        if next <= frame { break; }
        frame = next;
    }
    uart::write(b"\n");
}
#[no_mangle]
pub extern "C" fn dotnet_pal_freestanding_exit(code: i32) -> ! { exit::exit(code) }

/// The C math library, from the pure-Rust `libm` port of musl's implementation.
macro_rules! math {
    ($($name:ident($($arg:ident: $t:ty),*) -> $r:ty;)*) => { $(
        #[no_mangle]
        pub extern "C" fn $name($($arg: $t),*) -> $r { libm::$name($($arg),*) }
    )* };
}
math! {
    acos(x: f64) -> f64; acosh(x: f64) -> f64; asin(x: f64) -> f64; asinh(x: f64) -> f64; atan(x: f64) -> f64;
    atan2(y: f64, x: f64) -> f64; atanh(x: f64) -> f64; cbrt(x: f64) -> f64; ceil(x: f64) -> f64; cos(x: f64) -> f64;
    cosh(x: f64) -> f64; exp(x: f64) -> f64; exp2(x: f64) -> f64; fabs(x: f64) -> f64; floor(x: f64) -> f64;
    fma(x: f64, y: f64, z: f64) -> f64; fmod(x: f64, y: f64) -> f64; log(x: f64) -> f64; log10(x: f64) -> f64;
    log2(x: f64) -> f64; pow(x: f64, y: f64) -> f64; round(x: f64) -> f64; sin(x: f64) -> f64; sinh(x: f64) -> f64;
    sqrt(x: f64) -> f64; tan(x: f64) -> f64; tanh(x: f64) -> f64; trunc(x: f64) -> f64;
    acosf(x: f32) -> f32; acoshf(x: f32) -> f32; asinf(x: f32) -> f32; asinhf(x: f32) -> f32; atanf(x: f32) -> f32;
    atan2f(y: f32, x: f32) -> f32; atanhf(x: f32) -> f32; cbrtf(x: f32) -> f32; ceilf(x: f32) -> f32; cosf(x: f32) -> f32;
    coshf(x: f32) -> f32; expf(x: f32) -> f32; exp2f(x: f32) -> f32; fabsf(x: f32) -> f32; floorf(x: f32) -> f32;
    fmaf(x: f32, y: f32, z: f32) -> f32; fmodf(x: f32, y: f32) -> f32; logf(x: f32) -> f32; log10f(x: f32) -> f32;
    log2f(x: f32) -> f32; powf(x: f32, y: f32) -> f32; roundf(x: f32) -> f32; sinf(x: f32) -> f32; sinhf(x: f32) -> f32;
    sqrtf(x: f32) -> f32; tanf(x: f32) -> f32; tanhf(x: f32) -> f32; truncf(x: f32) -> f32;
}
#[no_mangle]
pub extern "C" fn modf(x: f64, integral: *mut f64) -> f64 {
    let (fraction, whole) = libm::modf(x);
    // SAFETY: the C contract passes a writable double.
    unsafe { integral.write(whole) };
    fraction
}
#[no_mangle]
pub extern "C" fn modff(x: f32, integral: *mut f32) -> f32 {
    let (fraction, whole) = libm::modff(x);
    unsafe { integral.write(whole) };
    fraction
}
#[no_mangle]
pub extern "C" fn ldexp(x: f64, exponent: i32) -> f64 { libm::ldexp(x, exponent) }
#[no_mangle]
pub extern "C" fn ldexpf(x: f32, exponent: i32) -> f32 { libm::ldexpf(x, exponent) }
#[no_mangle]
pub extern "C" fn scalbn(x: f64, exponent: i32) -> f64 { libm::scalbn(x, exponent) }
#[no_mangle]
pub extern "C" fn scalbnf(x: f32, exponent: i32) -> f32 { libm::scalbnf(x, exponent) }
#[no_mangle]
pub extern "C" fn frexp(x: f64, exponent: *mut i32) -> f64 {
    let (mantissa, e) = libm::frexp(x);
    unsafe { exponent.write(e) };
    mantissa
}
#[no_mangle]
pub extern "C" fn frexpf(x: f32, exponent: *mut i32) -> f32 {
    let (mantissa, e) = libm::frexpf(x);
    unsafe { exponent.write(e) };
    mantissa
}
#[no_mangle]
pub extern "C" fn ilogb(x: f64) -> i32 { libm::ilogb(x) }
#[no_mangle]
pub extern "C" fn ilogbf(x: f32) -> i32 { libm::ilogbf(x) }
