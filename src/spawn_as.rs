//! A child started as another user (`CAP_SPAWN_AS`). The request is the one of
//! [`crate::processes`] with the identity the child is to run under; the result is a
//! child of the processes group, so the type that provides [`crate::port::Processes`]
//! provides this one.
use crate::io::{ACCESS_DENIED, IS_DIRECTORY, NAME_TOO_LONG, NOT_DIRECTORY, TOO_MANY_HANDLES};
use crate::port::{Port, SpawnAs};
use crate::processes::{request, Spawned, Texts};
use crate::runtime::NOT_FOUND;
use crate::{aligned_output, Counter, Header, INVALID_ARGUMENT, OK, OS_ERROR, OUT_OF_MEMORY, UNSUPPORTED};
use core::mem;

pub const CAP: u64 = 4398046511104;
/// Most supplementary groups a request names.
pub const MAX_GROUPS: usize = 65536;

/// The identity a child runs under: its user, its primary group and its supplementary groups.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Identity { pub user_id: u32, pub group_id: u32, pub groups: *const u32, pub group_count: usize }
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ops {
    #[allow(clippy::type_complexity)]
    pub spawn_as: Option<unsafe extern "C" fn(*const u8, usize, Texts, usize, Texts, usize, *const u8, usize, u32, *const Identity, *mut Spawned, usize) -> u32>,
    pub read_stats: Option<unsafe extern "C" fn(*mut Stats, usize) -> u32>,
}
#[repr(C)]
pub struct Host { pub header: Header, pub ops: Ops }
#[repr(C)]
#[derive(Default)]
pub struct Stats { pub spawn_ok: u64, pub rejected_or_failed: u64 }
static COUNTERS: [Counter; 2] = [const { Counter::new() }; 2];

fn record(status: u32) -> u32 {
    let status = match status {
        OK | UNSUPPORTED | INVALID_ARGUMENT | OS_ERROR | OUT_OF_MEMORY | ACCESS_DENIED | TOO_MANY_HANDLES | NOT_FOUND | IS_DIRECTORY | NOT_DIRECTORY | NAME_TOO_LONG => status,
        _ => OS_ERROR,
    };
    COUNTERS[if status == OK { 0 } else { 1 }].increment();
    status
}
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn spawn_as<S: SpawnAs>(program: *const u8, program_length: usize, arguments: Texts, argument_count: usize, environment: Texts,
    environment_count: usize, directory: *const u8, directory_length: usize, pipes: u32, identity: *const Identity, out: *mut Spawned, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Spawned>() { return record(INVALID_ARGUMENT); }
    unsafe { out.write(Spawned::EMPTY) };
    let Some((program, arguments, environment, directory)) = (unsafe { request(program, program_length, arguments, argument_count, environment, environment_count,
        directory, directory_length, pipes) }) else { return record(INVALID_ARGUMENT); };
    if identity.is_null() || !(identity as usize).is_multiple_of(mem::align_of::<Identity>()) { return record(INVALID_ARGUMENT); }
    // SAFETY: a readable identity is the caller's contract.
    let identity = unsafe { identity.read() };
    if identity.group_count > MAX_GROUPS || (identity.group_count != 0 && (identity.groups.is_null() || !(identity.groups as usize).is_multiple_of(mem::align_of::<u32>()))) {
        return record(INVALID_ARGUMENT);
    }
    // SAFETY: `group_count` readable ids are the caller's contract; an empty list needs no pointer.
    let groups = if identity.group_count == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(identity.groups, identity.group_count) } };
    let status = match unsafe { S::spawn_as(program, arguments, environment, directory, pipes, identity.user_id, identity.group_id, groups) } {
        Ok(spawned) if spawned.consistent(pipes) => { unsafe { out.write(spawned) }; OK }
        Ok(_) => OS_ERROR,
        Err(e) => e.status(),
    };
    record(status)
}
unsafe extern "C" fn read_stats(out: *mut Stats, size: usize) -> u32 {
    if !aligned_output(out) || size < mem::size_of::<Stats>() { return INVALID_ARGUMENT; }
    unsafe { out.write(Stats { spawn_ok: COUNTERS[0].load(), rejected_or_failed: COUNTERS[1].load() }) };
    OK
}
pub const EMPTY: Ops = Ops { spawn_as: None, read_stats: Some(read_stats) };
/// Capability bit and callbacks for the port's provider of children under another identity.
pub fn negotiate<P: Port>() -> (u64, Ops) {
    type T<P> = <P as Port>::SpawnAs;
    if !T::<P>::PROVIDED { return (0, EMPTY); }
    (CAP, Ops { spawn_as: Some(spawn_as::<T<P>>), read_stats: Some(read_stats) })
}
