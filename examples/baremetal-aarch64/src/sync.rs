//! Interior mutability for the port's static tables.
//!
//! The machine has one core, the boot code never unmasks interrupts, and the
//! scheduler is cooperative: a thread switch happens only where this crate calls
//! [`crate::thread::schedule`]. A borrow taken here is therefore exclusive for as
//! long as the holder does not yield, which is the rule every caller in this
//! crate follows. This is not a lock and would not be sound on a second core.
use core::cell::UnsafeCell;

pub struct Single<T> {
    value: UnsafeCell<T>,
}
// SAFETY: one core, no interrupts, no preemption. See the module comment.
unsafe impl<T> Sync for Single<T> {}
impl<T> Single<T> {
    pub const fn new(value: T) -> Self {
        Self { value: UnsafeCell::new(value) }
    }
    /// Borrows the value mutably.
    ///
    /// # Safety
    /// No other borrow of the same value may be live, and the borrow must be
    /// dropped before any call that can switch threads.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }
}
