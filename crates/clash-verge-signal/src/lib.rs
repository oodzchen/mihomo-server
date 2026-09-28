use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
pub mod unix;

#[cfg(unix)]
pub use unix::{UnixSignals, register};

pub static SHUTDOWN_LATCH: ShutdownLatch = ShutdownLatch::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownOutcome {
    Committed,
    Canceled,
}

pub struct ShutdownLatch {
    is_cleaning_up: AtomicBool,
}

impl ShutdownLatch {
    pub const fn new() -> Self {
        Self {
            is_cleaning_up: AtomicBool::new(false),
        }
    }

    pub fn try_begin(&self) -> bool {
        self.is_cleaning_up
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn finish(&self, outcome: ShutdownOutcome) {
        if matches!(outcome, ShutdownOutcome::Canceled) {
            self.is_cleaning_up.store(false, Ordering::Release);
        }
    }

    pub fn is_shutting_down(&self) -> bool {
        self.is_cleaning_up.load(Ordering::Acquire)
    }

    pub fn reset(&self) {
        self.is_cleaning_up.store(false, Ordering::Release);
    }
}

impl Default for ShutdownLatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ShutdownLatch, ShutdownOutcome};

    #[test]
    fn canceled_shutdown_releases_latch_and_permits_next_attempt() {
        let latch = ShutdownLatch::new();

        assert!(latch.try_begin());
        assert!(!latch.try_begin());
        assert!(latch.is_shutting_down());

        latch.finish(ShutdownOutcome::Canceled);

        assert!(!latch.is_shutting_down());
        assert!(latch.try_begin());
    }

    #[test]
    fn committed_shutdown_retains_latch_and_suppresses_repeats() {
        let latch = ShutdownLatch::new();

        assert!(latch.try_begin());
        assert!(latch.is_shutting_down());
        latch.finish(ShutdownOutcome::Committed);

        assert!(latch.is_shutting_down());
        assert!(!latch.try_begin());
    }

    #[test]
    fn manual_reset_clears_latch() {
        let latch = ShutdownLatch::new();
        assert!(latch.try_begin());
        assert!(latch.is_shutting_down());
        latch.reset();
        assert!(!latch.is_shutting_down());
        assert!(latch.try_begin());
    }
}
