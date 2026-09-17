//! Pointer-width, saturating diagnostic counters; no 64-bit atomic dependency.
use core::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Counter(AtomicUsize);
impl Counter {
    pub const fn new() -> Self { Self(AtomicUsize::new(0)) }
    pub fn increment(&self) {
        let mut current = self.0.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_add(1);
            match self.0.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
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
