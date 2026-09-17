//! A complete WASIp1 raw-import transport for an explicitly wasm32 profile.
//! The libc adapter enters the same API table as the GC. Only one host import
//! remains. This is not WASIp2, native POSIX, or a claim about arbitrary targets.
use crate::port::{Port, WasiTransport};
use crate::{aligned_output, Counter, INVALID_ARGUMENT, OK};
use core::mem;
#[path="wasi_schema.rs"]
pub mod schema;
use schema::{ARG_COUNTS, OP_COUNT, WIDE_MASKS};
pub const CAP: u64 = 65536;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    pub invoke: Option<unsafe extern "C" fn(u32, *const u64, u32) -> u32>,
    pub call_count: Option<extern "C" fn(u32) -> u64>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Stats { pub calls: u64, pub rejected: u64, pub host_errors: u64 }
/// Wire request handed to the single host import. Fixed 88-byte layout.
#[repr(C)]
pub struct Request { pub version: u32, pub size: u32, pub opcode: u32, pub argc: u32, pub args: [u64;9] }
const _: () = assert!(mem::size_of::<Request>() == 88);
static COUNTERS: [Counter;OP_COUNT] = [const { Counter::new() }; OP_COUNT];
static REJECTED: Counter = Counter::new();
static ERRORS: Counter = Counter::new();
unsafe extern "C" fn invoke<T: WasiTransport>(opcode: u32, args: *const u64, argc: u32) -> u32 {
    if opcode as usize >= OP_COUNT || argc as usize != ARG_COUNTS[opcode as usize]
        || (argc != 0 && (!aligned_output(args.cast_mut()) || (args as usize).checked_add(argc as usize*8).is_none())) {
        REJECTED.increment(); return 28; // __WASI_ERRNO_INVAL
    }
    let mut request = Request { version:1, size:88, opcode, argc, args:[0;9] };
    for i in 0..argc as usize {
        let value=unsafe { args.add(i).read() };
        if WIDE_MASKS[opcode as usize] & (1<<i) == 0 && value > u32::MAX as u64 {
            REJECTED.increment(); return 28;
        }
        request.args[i]=value;
    }
    COUNTERS[opcode as usize].increment();
    let status=unsafe { T::dispatch(&request) };
    let status=if status <= 76 { status } else { 29 }; // Unknown statuses must not truncate into success.
    if status != 0 { ERRORS.increment(); }
    status
}
extern "C" fn count(op: u32) -> u64 { if (op as usize) < OP_COUNT { COUNTERS[op as usize].load() } else { 0 } }
unsafe extern "C" fn stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    let calls=COUNTERS.iter().fold(0u64,|n,c| n.saturating_add(c.load()));
    unsafe { out.write(Stats { calls, rejected:REJECTED.load(), host_errors:ERRORS.load() }) }; OK
}
pub const EMPTY: Ops = Ops { invoke:None, call_count:None, read_stats:None };
pub fn negotiate<P: Port>() -> (u64, Ops) {
    if !P::Wasi::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { invoke:Some(invoke::<P::Wasi>), call_count:Some(count), read_stats:Some(stats) })
}
