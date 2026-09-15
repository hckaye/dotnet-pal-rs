//! Optional diagnostics must not require 64-bit atomics on all Rust targets.
#[cfg(target_has_atomic = "64")]
pub type Counter = core::sync::atomic::AtomicU64;

// Stateless fallback: no unsafe mutable globals and no pretend successful stats.
#[cfg(not(target_has_atomic = "64"))]
pub struct Counter;

#[cfg(not(target_has_atomic = "64"))]
impl Counter {
    pub const fn new(_: u64) -> Self { Self }
    pub fn fetch_add(&self, _: u64, _: core::sync::atomic::Ordering) -> u64 { 0 }
    pub fn load(&self, _: core::sync::atomic::Ordering) -> u64 { 0 }
}
