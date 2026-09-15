//! Pointer-width, saturating diagnostic counters; no 64-bit atomic dependency.
use core::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Counter(AtomicUsize);
impl Counter {
    pub const fn new() -> Self { Self(AtomicUsize::new(0)) }
    pub fn increment(&self) {
        let _ = self.0.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
            |n| Some(n.saturating_add(1)));
    }
    pub fn load(&self) -> u64 { self.0.load(Ordering::Relaxed) as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturates_instead_of_wrapping() {
        let c = Counter(AtomicUsize::new(usize::MAX - 1));
        c.increment(); c.increment();
        assert_eq!(c.load(), usize::MAX as u64);
    }
}
